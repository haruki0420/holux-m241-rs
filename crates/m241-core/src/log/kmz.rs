//! KMZ export: bundling KML 2.0 track geometry and photos into a Store ZIP.
//!
//! A KMZ archive is a standard PKZIP container with `doc.kml` at its root and
//! referenced photos under `photo/`. Using uncompressed Store (`method = 0`)
//! keeps this crate free of external compression dependencies. The archive is
//! checked here by a reader written against the PKZIP field layout and by a CRC
//! check; **Google Earth and `unzip` have never been run on the output**, so
//! "a standard tool opens this" is unverified.
//!
//! # Photo size
//!
//! `07-eztour-spec.ja.md` §6.3 asks for a long-edge limit (`kmzMaxPhotoEdgeSize`)
//! on the embedded photos. [`KmzPhoto::max_edge_pixels`] carries that limit and
//! [`export_kmz`] reduces the photo to it:
//!
//! * a photo whose long edge is **at or under** the limit, or one with no limit
//!   (`None`), is stored **byte for byte** — it is never re-encoded, so its EXIF
//!   and every other metadata block survive untouched;
//! * only a photo **over** the limit is decoded, scaled to `long edge = limit`
//!   with the aspect ratio kept, and re-encoded as JPEG.
//!
//! The re-encode **may drop metadata**: `image` writes a bare JFIF frame, so
//! EXIF (camera time, GPS tags), ICC profiles and comments in a reduced photo
//! are not carried over. A photo read back out of a reduced KMZ will therefore
//! look like one with no EXIF. The KML `<TimeStamp>` and `<Point>` are written
//! from [`KmzPhoto`] before the reduction and are unaffected.
//!
//! The quality of that re-encode is fixed at [`RESIZE_JPEG_QUALITY`] = 85,
//! the usual "visually near-lossless, much smaller" point for a thumbnail-sized
//! image; a KMZ photo is displayed on a map at well under its pixel size, so
//! lower quality would lose detail nobody sees and higher would mostly cost
//! bytes.
//!
//! A photo over the limit that **cannot be decoded** is an [`Error::State`]
//! naming the photo and the decoder's reason: the bytes it holds are not a JPEG
//! this build can read, and embedding them at full size would ignore the limit
//! the caller asked for.
//!
//! # `icon/` thumbnails
//!
//! `07-eztour-spec.ja.md` §6.3 says a KMZ carries the photos under `photo/` and
//! their thumbnails under `icon/` (or as reduced copies). It does not record how
//! large a thumbnail is, whether the "Photo size in KMZ" limit
//! (`kmzMaxPhotoEdgeSize`) bounds the `photo/` entry or the `icon/` one, what
//! the thumbnails are named, or how the KML refers to them. Those points are
//! **unconfirmed**, so this module does not guess at them:
//!
//! * [`export_kmz`] keeps the behaviour that predates the thumbnails and writes
//!   no `icon/` entries;
//! * [`export_kmz_with_icons`] writes one `icon/<name>` per photo, using the
//!   same sanitised, de-duplicated name as the `photo/` entry, and takes the
//!   thumbnail's long-edge limit from [`KmzIconOptions`] — a size the caller
//!   chose is a decision, where a size invented here would be a guess;
//! * a thumbnail is produced by the same reduction path as the photo entry:
//!   over the limit it is decoded, scaled with the aspect ratio kept and
//!   re-encoded as JPEG; at or under the limit, or when its frame header cannot
//!   be read, its bytes are stored unchanged;
//! * `doc.kml` is unchanged. It still shows the `photo/` copy in the balloon,
//!   and no `<IconStyle>` is invented for a thumbnail the spec does not say is a
//!   marker.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;

use crate::error::{Error, Result};
use crate::log::exif::{JPEG_MARKER_EOI, JPEG_MARKER_SOI};
use crate::log::export::{DOCUMENT_NAME, escape_xml, utc_date, utc_iso};
use crate::log::geotag::CaptureTime;
use crate::types::Track;

/// ZIP local file header signature (`PK\x03\x04`).
const LFH_SIGNATURE: u32 = 0x0403_4B50;
/// ZIP central directory file header signature (`PK\x01\x02`).
const CDFH_SIGNATURE: u32 = 0x0201_4B50;
/// ZIP end of central directory record signature (`PK\x05\x06`).
const EOCD_SIGNATURE: u32 = 0x0605_4B50;
/// Compression method 0: uncompressed Store.
const METHOD_STORE: u16 = 0;
/// PKZIP version 2.0 ("made by" and "needed to extract").
const ZIP_VERSION_20: u16 = 20;
/// General purpose bit 11: filename and comment are UTF-8 encoded.
const FLAG_UTF8: u16 = 0x0800;

/// Default track style colour as KML's `<color>` string: `aabbggrr`.
///
/// Read right to left in byte pairs: `ff` is red, `80` green, `00` blue — a
/// purple — and the leading `b3` is alpha, 179/255 ≈ 0.7 opacity. Width 5 and
/// this alpha come from `07-eztour-spec.ja.md` §5, which records what ezTour's
/// map drew (purple, 5 px, 0.7 opacity).
///
/// The hex value is **this project's choice**, not ezTour's: the spec names the
/// colours in words and says they were carried as `#rrggbb` strings, and
/// nothing in this repository records the exact bytes ezTour emitted. So this
/// matches the *description*, and a side-by-side against real ezTour output is
/// unverified. A single colour is all one track needs; the ten-colour palette is
/// a multi-track feature (§5) that this exporter has no caller for yet, so
/// adding the other nine now would be untested colour data.
const DEFAULT_LINE_COLOR: &str = "b3ff0080";
/// Default track polyline width in pixels, from the same §5 measurement.
const DEFAULT_LINE_WIDTH: u8 = 5;

/// JPEG quality used when a photo over the long-edge limit is re-encoded.
///
/// Fixed rather than configurable: the spec gives one setting
/// (`kmzMaxPhotoEdgeSize`) and no quality knob. 85 is the point where the
/// artefacts of a further compression of an already-JPEG photo stay below what
/// a map-sized view shows, while the file is a small fraction of the original.
/// See the module doc.
const RESIZE_JPEG_QUALITY: u8 = 85;

/// A photo to embed in a KMZ archive under `photo/`.
#[derive(Debug, Clone, PartialEq)]
pub struct KmzPhoto {
    /// Filename to use in the archive and as the Placemark label (sanitised
    /// before being placed in `photo/`).
    pub name: String,
    /// Latitude in signed decimal degrees.
    pub lat: f64,
    /// Longitude in signed decimal degrees.
    pub lon: f64,
    /// Altitude in metres. `None` is written as `0` in the KML because the
    /// `<coordinates>` triple has no "unknown" value; see [`export_kmz`].
    pub altitude_m: Option<f32>,
    /// Capture time and where it came from; `None` writes no `<TimeStamp>`.
    ///
    /// The provenance is kept rather than flattened to a second count so a
    /// reader can tell a shutter time from a file's modification time. See
    /// [`crate::log::geotag::GeotagPlan::to_kmz_photo`] for the conversion
    /// from a geotag plan.
    pub utc: Option<CaptureTime>,
    /// Largest long edge the caller will accept for this photo, in pixels
    /// (`kmzMaxPhotoEdgeSize`). `None` accepts the bytes as they are.
    ///
    /// A photo over the limit is decoded, scaled down to it and re-encoded;
    /// one at or under it keeps its exact bytes. See the module doc for what a
    /// re-encode costs.
    pub max_edge_pixels: Option<u32>,
    /// Raw JPEG bytes to store in the archive.
    pub jpeg_bytes: Vec<u8>,
}

impl KmzPhoto {
    /// The ZIP and `<TimeStamp>` timestamp: Unix seconds when the capture time
    /// came from the camera, `None` when it came from the file system.
    ///
    /// A modification time says when the file was written, and putting it in
    /// the KML would place the photo on the map at a moment it was not taken.
    /// The ZIP entry keeps it, because a ZIP entry always has *some* timestamp
    /// and the file's own is the honest one there.
    #[must_use]
    fn exif_capture_utc(&self) -> Option<u32> {
        match self.utc {
            Some(CaptureTime::Exif(utc)) => Some(utc),
            Some(CaptureTime::FileModified(_)) | None => None,
        }
    }

    /// The ZIP entry timestamp: either capture time's seconds.
    #[must_use]
    fn zip_utc(&self) -> Option<u32> {
        self.utc.map(CaptureTime::utc)
    }
}

/// How [`export_kmz_with_icons`] writes the `icon/` thumbnails.
///
/// `07-eztour-spec.ja.md` §6.3 names one size knob, `kmzMaxPhotoEdgeSize`, and
/// does not say whether it bounds the `photo/` entry, the `icon/` entry or both,
/// so the thumbnail size is a value the caller supplies rather than one this
/// module invents. See the module doc's "`icon/` thumbnails" section for the
/// remaining unconfirmed points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KmzIconOptions {
    /// Largest long edge of an `icon/` thumbnail, in pixels. Must be at least
    /// one; zero is refused because a thumbnail of no pixels is not a thumbnail.
    pub max_edge_pixels: u32,
}

/// Bundle `track` and `photos` into an uncompressed (Store) KMZ archive.
///
/// The resulting ZIP contains `doc.kml` (KML 2.0 with styled `<LineString>` and
/// a `<Folder><name>Photos</name>` of photo `<Placemark>` entries) and each
/// photo under `photo/<sanitised_name>`. No `icon/` entry is written; use
/// [`export_kmz_with_icons`] when thumbnails are wanted (see the module doc).
///
/// A photo with no altitude is written with altitude `0`, because KML's
/// `<coordinates>` has no "unknown" value and a two-value pair would clamp to
/// the ground; the Placemark already carries `<Point>` so the horizontal
/// position is right, and only the height is a placeholder.
///
/// # Errors
///
/// Returns [`Error::State`] if a photo coordinate or altitude is non-finite or
/// out of range, or if a photo exceeds [`KmzPhoto::max_edge_pixels`] and its
/// bytes cannot be decoded as a JPEG (see the module doc), and [`Error::Log`]
/// if the archive exceeds ZIP32 limits.
pub fn export_kmz(track: &Track, photos: &[KmzPhoto]) -> Result<Vec<u8>> {
    export_kmz_inner(track, photos, None)
}

/// Bundle `track` and `photos` into an uncompressed (Store) KMZ archive with an
/// `icon/` thumbnail beside each photo.
///
/// The archive is the one [`export_kmz`] writes plus one
/// `icon/<sanitised_name>` entry per photo, named exactly like its `photo/`
/// entry (so `photo/holiday.jpg` is paired with `icon/holiday.jpg`). The
/// thumbnail's long edge is [`KmzIconOptions::max_edge_pixels`]; a photo already
/// within that limit, or one whose frame header cannot be read, contributes its
/// own bytes, so a KMZ of small photos does not grow beyond the duplicated
/// bytes.
///
/// The `photo/` entry keeps the behaviour [`export_kmz`] documents, including
/// [`KmzPhoto::max_edge_pixels`]; the two limits are independent.
///
/// # Errors
///
/// The same errors as [`export_kmz`], with one addition: a thumbnail over the
/// icon limit that cannot be decoded is an [`Error::State`] naming the photo and
/// the icon limit, because storing the full-size bytes would be a silent
/// duplication of the `photo/` entry rather than a thumbnail. An icon limit of
/// zero is also an [`Error::State`].
pub fn export_kmz_with_icons(
    track: &Track,
    photos: &[KmzPhoto],
    icons: &KmzIconOptions,
) -> Result<Vec<u8>> {
    if icons.max_edge_pixels == 0 {
        return Err(Error::State(String::from(
            "the icon/ thumbnail long edge must be at least one pixel",
        )));
    }
    export_kmz_inner(track, photos, Some(icons.max_edge_pixels))
}

fn export_kmz_inner(
    track: &Track,
    photos: &[KmzPhoto],
    icon_max_edge: Option<u32>,
) -> Result<Vec<u8>> {
    let mut used_names = HashSet::with_capacity(photos.len());
    let mut resolved: Vec<(String, &KmzPhoto)> = Vec::with_capacity(photos.len());

    for photo in photos {
        validate_photo(photo)?;
        let sanitised = sanitize_photo_name(&photo.name);
        let unique = deduplicate_name(&sanitised, &mut used_names);
        resolved.push((unique, photo));
    }

    let kml = build_kml(track, &resolved);
    let kml_utc = track.time_span().map(|(_, end)| end);

    // Reduced bytes live here, not beside the photo, so the entries can borrow
    // either the caller's slice or this buffer for the life of the write.
    let mut reduced: Vec<Option<Vec<u8>>> = Vec::with_capacity(resolved.len());
    for (_, photo) in &resolved {
        reduced.push(reduce_to_edge_limit(photo)?);
    }

    // A thumbnail is its own reduction, not a copy of the photo entry: the two
    // limits are independent, so an archive may keep an original under `photo/`
    // and a reduced copy under `icon/`.
    let mut thumbnails: Vec<Option<Vec<u8>>> = Vec::with_capacity(resolved.len());
    if let Some(max_edge) = icon_max_edge {
        for (_, photo) in &resolved {
            let label = format!("the icon/ thumbnail of photo {:?}", photo.name);
            thumbnails.push(reduce_bytes_to_edge_limit(
                &label,
                &photo.jpeg_bytes,
                Some(max_edge),
            )?);
        }
    }

    let entries_per_photo = 1 + usize::from(icon_max_edge.is_some());
    let mut entries: Vec<ZipInput<'_>> = Vec::with_capacity(1 + resolved.len() * entries_per_photo);
    entries.push(ZipInput {
        path: String::from("doc.kml"),
        data: kml.as_bytes(),
        utc: kml_utc,
    });
    for ((name, photo), reduced_bytes) in resolved.iter().zip(&reduced) {
        entries.push(ZipInput {
            path: format!("photo/{name}"),
            data: reduced_bytes.as_deref().unwrap_or(&photo.jpeg_bytes),
            utc: photo.zip_utc(),
        });
    }
    if icon_max_edge.is_some() {
        for ((name, photo), thumbnail) in resolved.iter().zip(&thumbnails) {
            entries.push(ZipInput {
                path: format!("icon/{name}"),
                data: thumbnail.as_deref().unwrap_or(&photo.jpeg_bytes),
                utc: photo.zip_utc(),
            });
        }
    }

    write_zip_store(&entries)
}

fn validate_photo(photo: &KmzPhoto) -> Result<()> {
    if !photo.lat.is_finite() || photo.lat.abs() > 90.0 {
        return Err(Error::State(format!(
            "photo {:?} has invalid latitude {}",
            photo.name, photo.lat
        )));
    }
    if !photo.lon.is_finite() || photo.lon.abs() > 180.0 {
        return Err(Error::State(format!(
            "photo {:?} has invalid longitude {}",
            photo.name, photo.lon
        )));
    }
    if photo.altitude_m.is_some_and(|alt| !alt.is_finite()) {
        return Err(Error::State(format!(
            "photo {:?} has non-finite altitude",
            photo.name
        )));
    }
    Ok(())
}

/// The bytes to store for `photo`: its own when the limit is absent or met, a
/// re-encoded reduction when the long edge is over it.
///
/// `Ok(None)` means "store [`KmzPhoto::jpeg_bytes`] unchanged". The size is read
/// from the frame header first, so a photo within the limit is never decoded.
fn reduce_to_edge_limit(photo: &KmzPhoto) -> Result<Option<Vec<u8>>> {
    let label = format!("photo {:?}", photo.name);
    reduce_bytes_to_edge_limit(&label, &photo.jpeg_bytes, photo.max_edge_pixels)
}

/// [`reduce_to_edge_limit`] over raw bytes, shared with the `icon/` thumbnails.
///
/// `label` names what is being reduced in an error message: `photo "a.jpg"` for
/// a `photo/` entry, `the icon/ thumbnail of photo "a.jpg"` for an `icon/` one.
/// An `icon/` thumbnail is the same reduction under another limit, so the two
/// callers must not carry two copies of it.
fn reduce_bytes_to_edge_limit(
    label: &str,
    bytes: &[u8],
    max_edge: Option<u32>,
) -> Result<Option<Vec<u8>>> {
    let Some(max_edge) = max_edge.filter(|limit| *limit > 0) else {
        return Ok(None);
    };
    // A byte stream with no frame header cannot be measured; it is stored as it
    // is, as before the reduction existed. Every decodable JPEG carries a SOF.
    let Some((width, height)) = jpeg_dimensions(bytes) else {
        return Ok(None);
    };
    let long_edge = width.max(height);
    if long_edge <= max_edge {
        return Ok(None);
    }

    let decoded =
        image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).map_err(|err| {
            Error::State(format!(
                "{label} is {width} pixels wide and {height} high, and its long edge \
                 {long_edge} exceeds the {max_edge} pixel limit, but it could not be decoded \
                 to be reduced: {err}"
            ))
        })?;

    // Keep the aspect ratio: scale the long edge to the limit and round the
    // short edge to the nearest proportional value, never below one pixel.
    let (target_width, target_height) = if width >= height {
        (max_edge, scale_short_edge(height, width, max_edge))
    } else {
        (scale_short_edge(width, height, max_edge), max_edge)
    };
    let resized = decoded.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::Lanczos3,
    );

    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        Cursor::new(&mut out),
        RESIZE_JPEG_QUALITY,
    );
    encoder
        .encode_image(&resized)
        .map_err(|err| Error::State(format!("{label} could not be re-encoded: {err}")))?;
    Ok(Some(out))
}

/// The short edge after scaling `long` down to `limit`, rounded to nearest and
/// held at one pixel.
fn scale_short_edge(short: u32, long: u32, limit: u32) -> u32 {
    if long == 0 {
        return 1;
    }
    // Rounded half up without a float: (short * limit + long / 2) / long.
    let rounded = (u64::from(short) * u64::from(limit) + u64::from(long) / 2) / u64::from(long);
    u32::try_from(rounded).unwrap_or(1).max(1)
}

/// Read a JPEG's pixel dimensions from its first Start-Of-Frame segment.
///
/// Returns `None` when the bytes are not a JPEG, or are a JPEG whose headers do
/// not hold a frame size this function recognises (the arithmetic-coded and
/// lossless SOF variants, or a truncated header). JPEG has no other place the
/// size is recorded, so `None` is "unknown", not "small".
///
/// Only the size matters here, so the walk stops at the first frame header and
/// never looks at the entropy-coded data, where `FF` bytes are stuffed and a
/// marker scan would be meaningless.
#[must_use]
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Segments after SOI are `FF <marker> <len hi> <len lo> <payload>`, where
    // `len` includes its own two bytes. `FF` padding before a marker is legal.
    let mut cursor = bytes.strip_prefix(&[0xFF, 0xD8])?;
    loop {
        // Skip any `FF` fill bytes before the marker, then consume the marker.
        let marker = loop {
            let next = *cursor.first()?;
            cursor = cursor.get(1..)?;
            if next != 0xFF {
                break next;
            }
        };
        match marker {
            // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC) are frame
            // headers; after the length comes precision, height, then width.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let height = u32::from(u16::from_be_bytes([*cursor.get(3)?, *cursor.get(4)?]));
                let width = u32::from(u16::from_be_bytes([*cursor.get(5)?, *cursor.get(6)?]));
                return (width > 0 && height > 0).then_some((width, height));
            }
            // Start of scan or end of image: no frame header came before, so
            // the size is not where it should be. The EOI byte is named by its
            // JPEG meaning, not by its MTK meaning (see exif.rs).
            0xDA | JPEG_MARKER_EOI => return None,
            // Restart markers and TEM stand alone, without a length.
            0xD0..=JPEG_MARKER_SOI | 0x01 => {}
            _ => {
                let len = usize::from(u16::from_be_bytes([*cursor.first()?, *cursor.get(1)?]));
                // `cursor` is at the length field: `len` counts the two length
                // bytes plus the payload, so skip the payload after them.
                cursor = cursor.get(2..)?;
                cursor = cursor.get(len.checked_sub(2)?..)?;
            }
        }
    }
}

/// Sanitise a photo filename so it cannot escape `photo/` or break XML/ZIP.
fn sanitize_photo_name(raw: &str) -> String {
    let candidate = raw
        .split(['/', '\\'])
        .rfind(|part| !part.is_empty() && *part != "." && *part != "..")
        .unwrap_or("photo.jpg");

    let mut cleaned = String::with_capacity(candidate.len());
    for ch in candidate.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '"' | '\'' | '&' | ']') {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }
    while cleaned.contains("..") {
        cleaned = cleaned.replace("..", "_");
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        String::from("photo.jpg")
    } else {
        trimmed.to_owned()
    }
}

/// Ensure each archive entry under `photo/` is unique (case-insensitively).
fn deduplicate_name(base: &str, used_lower: &mut HashSet<String>) -> String {
    let lower = base.to_ascii_lowercase();
    if used_lower.insert(lower) {
        return base.to_owned();
    }

    let (stem, ext) = match base.rfind('.') {
        Some(dot) if dot > 0 => match (base.get(..dot), base.get(dot..)) {
            (Some(s), Some(e)) => (s, e),
            _ => (base, ""),
        },
        _ => (base, ""),
    };

    let mut counter = 1usize;
    loop {
        let candidate = format!("{stem}_{counter}{ext}");
        let candidate_lower = candidate.to_ascii_lowercase();
        if used_lower.insert(candidate_lower) {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

fn build_kml(track: &Track, photos: &[(String, &KmzPhoto)]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<kml xmlns=\"http://earth.google.com/kml/2.0\">\n");
    out.push_str("  <Document>\n");
    let _ = writeln!(out, "    <name>{}</name>", escape_xml(DOCUMENT_NAME));
    let _ = writeln!(
        out,
        "    <Style id=\"trackStyle\">\n      <LineStyle>\n        \
         <color>{DEFAULT_LINE_COLOR}</color>\n        <width>{DEFAULT_LINE_WIDTH}</width>\n      \
         </LineStyle>\n    </Style>"
    );
    out.push_str("    <Placemark>\n");
    let _ = writeln!(out, "      <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str("      <styleUrl>#trackStyle</styleUrl>\n");
    out.push_str("      <LineString>\n        <tessellate>1</tessellate>\n");
    out.push_str("        <coordinates>\n");
    for point in &track.points {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        let alt = point.height_m.filter(|h| h.is_finite()).unwrap_or(0.0);
        let _ = writeln!(out, "          {lon:.8},{lat:.8},{alt:.2}");
    }
    out.push_str("        </coordinates>\n      </LineString>\n    </Placemark>\n");

    out.push_str("    <Folder>\n      <name>Photos</name>\n");
    for (archive_name, photo) in photos {
        out.push_str("      <Placemark>\n");
        let _ = writeln!(out, "        <name>{}</name>", escape_xml(archive_name));
        if let Some(utc) = photo.exif_capture_utc() {
            let _ = writeln!(
                out,
                "        <TimeStamp><when>{}</when></TimeStamp>",
                utc_iso(utc)
            );
        }
        let _ = writeln!(
            out,
            "        <description><![CDATA[<img src=\"photo/{archive_name}\"/>]]></description>"
        );
        // A `<coordinates>` triple has no "unknown", and a two-value pair is
        // read as terrain-altitude (clamped to the ground). 0 is the honest
        // placeholder for "no altitude was measured"; the `<Point>` still
        // carries the horizontal position, which is what the photo claims.
        let alt = photo.altitude_m.unwrap_or(0.0);
        let _ = writeln!(
            out,
            "        <Point><coordinates>{:.8},{:.8},{alt:.2}</coordinates></Point>",
            photo.lon, photo.lat
        );
        out.push_str("      </Placemark>\n");
    }
    out.push_str("    </Folder>\n");
    out.push_str("  </Document>\n</kml>\n");
    out
}

// ---------------------------------------------------------------------------
// Pure-Rust uncompressed ZIP writer + IEEE CRC-32
// ---------------------------------------------------------------------------

struct ZipInput<'a> {
    path: String,
    data: &'a [u8],
    utc: Option<u32>,
}

struct CentralEntry<'a> {
    name: &'a [u8],
    crc: u32,
    size: u32,
    dos_time: u16,
    dos_date: u16,
    local_header_offset: u32,
}

/// Compute the IEEE 802.3 / PKZIP CRC-32 of `bytes`.
///
/// `pub(crate)`: the ZIP writer here and the tests are the only users, and a
/// public CRC would be a promise to keep a general checksum tool that the crate
/// does not otherwise need.
#[must_use]
pub(crate) fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Convert UTC Unix seconds into `(dos_time, dos_date)` for ZIP headers.
fn dos_datetime(utc: Option<u32>) -> (u16, u16) {
    let Some(secs) = utc else {
        // 1980-01-01 00:00:00 — earliest MS-DOS date.
        return (0, (1 << 5) | 1);
    };
    let (day, month, year) = utc_date(secs);
    if !(1980..=2107).contains(&year) {
        return (0, (1 << 5) | 1);
    }
    let sod = secs % 86_400;
    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second_half = (sod % 60) / 2;
    let dos_time = ((hour as u16) << 11) | ((minute as u16) << 5) | (second_half as u16);
    let dos_date = (((year - 1980) as u16) << 9) | ((month as u16) << 5) | (day as u16);
    (dos_time, dos_date)
}

fn write_zip_store(entries: &[ZipInput<'_>]) -> Result<Vec<u8>> {
    let entry_count = u16::try_from(entries.len()).map_err(|_| Error::Log {
        message: format!("KMZ has {} entries, exceeding ZIP32 limit", entries.len()),
        offset: None,
    })?;

    let mut out = Vec::new();
    let mut central = Vec::with_capacity(entries.len());

    for entry in entries {
        let name = entry.path.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| Error::Log {
            message: format!("KMZ entry name {:?} exceeds 65535 bytes", entry.path),
            offset: None,
        })?;
        let data_size = u32::try_from(entry.data.len()).map_err(|_| Error::Log {
            message: format!("KMZ entry {:?} exceeds 4 GiB ZIP32 limit", entry.path),
            offset: None,
        })?;
        let local_offset = u32::try_from(out.len()).map_err(|_| Error::Log {
            message: String::from("KMZ archive exceeds 4 GiB ZIP32 offset limit"),
            offset: None,
        })?;

        let crc = crc32_ieee(entry.data);
        let (dos_time, dos_date) = dos_datetime(entry.utc);

        // Local File Header (30 bytes + name)
        out.extend_from_slice(&LFH_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&data_size.to_le_bytes());
        out.extend_from_slice(&data_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name);
        out.extend_from_slice(entry.data);

        central.push(CentralEntry {
            name,
            crc,
            size: data_size,
            dos_time,
            dos_date,
            local_header_offset: local_offset,
        });
    }

    let cd_offset = u32::try_from(out.len()).map_err(|_| Error::Log {
        message: String::from("KMZ central directory exceeds 4 GiB ZIP32 offset limit"),
        offset: None,
    })?;

    for item in &central {
        let name_len = u16::try_from(item.name.len()).map_err(|_| Error::Log {
            message: String::from("KMZ entry name exceeds 65535 bytes"),
            offset: None,
        })?;
        // Central Directory File Header (46 bytes + name)
        out.extend_from_slice(&CDFH_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes()); // version made by
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes()); // version needed
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&item.dos_time.to_le_bytes());
        out.extend_from_slice(&item.dos_date.to_le_bytes());
        out.extend_from_slice(&item.crc.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal file attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external file attrs
        out.extend_from_slice(&item.local_header_offset.to_le_bytes());
        out.extend_from_slice(item.name);
    }

    let cd_size =
        u32::try_from(out.len().saturating_sub(cd_offset as usize)).map_err(|_| Error::Log {
            message: String::from("KMZ central directory size exceeds 4 GiB ZIP32 limit"),
            offset: None,
        })?;

    // End of Central Directory Record (22 bytes)
    out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // start disk
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrackPoint;

    const TOKYO_LAT: f64 = 35.681_236;
    const TOKYO_LON: f64 = 139.767_125;
    const TOKYO_UTC: u32 = 1_700_000_000;

    fn sample_track() -> Track {
        Track {
            points: vec![
                TrackPoint {
                    utc: Some(TOKYO_UTC),
                    latitude: Some(TOKYO_LAT),
                    longitude: Some(TOKYO_LON),
                    height_m: Some(40.0),
                    speed_kmh: Some(12.0),
                    rcr: None,
                },
                TrackPoint {
                    utc: Some(TOKYO_UTC + 60),
                    latitude: Some(34.693_7),
                    longitude: Some(135.502_3),
                    height_m: Some(25.0),
                    speed_kmh: Some(18.0),
                    rcr: None,
                },
            ],
            damaged_blocks: Vec::new(),
        }
    }

    /// A photo at the example coordinate, with everything optional left out.
    fn photo(name: &str) -> KmzPhoto {
        KmzPhoto {
            name: String::from(name),
            lat: TOKYO_LAT,
            lon: TOKYO_LON,
            altitude_m: None,
            utc: None,
            max_edge_pixels: None,
            jpeg_bytes: vec![0xFF, 0xD8, 0xFF, 0xE1],
        }
    }

    /// A JPEG-shaped byte stream whose SOF0 declares `width` x `height`.
    ///
    /// Every segment carries the length it declares: an APP0 with a one-byte
    /// payload, then SOF0 with precision, height, width and one component. It
    /// has no scan data, so [`jpeg_dimensions`] reads it but no decoder can.
    fn jpeg_with_size(width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        // APP0, length 6: the two length bytes plus four payload bytes.
        out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00]);
        // SOF0: length 12 = 2 + precision(1) + height(2) + width(2) + count(1)
        // + 4 bytes for one component (id, sampling, quant-table).
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0C, 0x08]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        out.extend_from_slice(&[0xFF, JPEG_MARKER_EOI]);
        out
    }

    /// A real, decodable JPEG of `width` x `height`, built in memory.
    ///
    /// Generated rather than checked in as a fixture: the resize tests need
    /// bytes `image` can decode, and a synthetic image of one pixel value is
    /// small, compresses well and carries no personal data.
    fn synthetic_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([120, 60, 30]));
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
            Cursor::new(&mut out),
            RESIZE_JPEG_QUALITY,
        );
        encoder
            .encode_image(&image)
            .expect("encode a synthetic JPEG");
        out
    }

    /// Decode `bytes` and return `(width, height)` as the decoder sees them.
    ///
    /// The SOF header is written by the encoder, so this is what a viewer will
    /// render; reading it back also proves the reduced bytes are a JPEG.
    fn decoded_dimensions(bytes: &[u8]) -> (u32, u32) {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
            .expect("a stored photo must decode");
        (decoded.width(), decoded.height())
    }

    struct ParsedZipEntry {
        name: String,
        method: u16,
        crc: u32,
        /// MS-DOS `(date, time)` fields the writer stored for the entry.
        dos_date: u16,
        dos_time: u16,
        data: Vec<u8>,
    }

    fn parse_zip_and_verify(bytes: &[u8]) -> Vec<ParsedZipEntry> {
        let eocd = read_eocd(bytes);
        assert_eq!(eocd.entry_count as usize, eocd.total_entries as usize);

        let mut entries = Vec::new();
        let mut cursor = eocd.cd_offset;
        for _ in 0..eocd.entry_count {
            let entry = read_central_entry(bytes, cursor);
            cursor += entry.record_len;
            entries.push(entry.into_parsed(bytes));
        }
        assert_eq!(cursor, eocd.position);
        entries
    }

    struct Eocd {
        position: usize,
        entry_count: u16,
        total_entries: u16,
        cd_offset: usize,
    }

    fn read_eocd(bytes: &[u8]) -> Eocd {
        assert!(bytes.len() >= 22, "ZIP shorter than EOCD");
        let position = bytes.len() - 22;
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(position), EOCD_SIGNATURE);
        let entry_count = u16_at(position + 8);
        let total_entries = u16_at(position + 10);
        let cd_size = u32_at(position + 12) as usize;
        let cd_offset = u32_at(position + 16) as usize;
        assert_eq!(cd_offset + cd_size, position);
        Eocd {
            position,
            entry_count,
            total_entries,
            cd_offset,
        }
    }

    struct CentralRecord {
        name: String,
        method: u16,
        crc: u32,
        size: usize,
        dos_date: u16,
        dos_time: u16,
        lfh_offset: usize,
        record_len: usize,
    }

    fn read_central_entry(bytes: &[u8], cursor: usize) -> CentralRecord {
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(cursor), CDFH_SIGNATURE);
        let method = u16_at(cursor + 10);
        let dos_time = u16_at(cursor + 12);
        let dos_date = u16_at(cursor + 14);
        let crc = u32_at(cursor + 16);
        let comp_size = u32_at(cursor + 20) as usize;
        let uncomp_size = u32_at(cursor + 24) as usize;
        assert_eq!(comp_size, uncomp_size);
        let name_len = u16_at(cursor + 28) as usize;
        let extra_len = u16_at(cursor + 30) as usize;
        let comment_len = u16_at(cursor + 32) as usize;
        let lfh_offset = u32_at(cursor + 42) as usize;
        let name = std::str::from_utf8(&bytes[cursor + 46..cursor + 46 + name_len])
            .expect("UTF-8 name")
            .to_owned();
        CentralRecord {
            name,
            method,
            crc,
            size: comp_size,
            dos_date,
            dos_time,
            lfh_offset,
            record_len: 46 + name_len + extra_len + comment_len,
        }
    }

    impl CentralRecord {
        fn into_parsed(self, bytes: &[u8]) -> ParsedZipEntry {
            let offset = self.lfh_offset;
            let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
            let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            assert_eq!(u32_at(offset), LFH_SIGNATURE);
            let lfh_method = u16_at(offset + 8);
            let lfh_crc = u32_at(offset + 14);
            let name_len = u16_at(offset + 26) as usize;
            let extra_len = u16_at(offset + 28) as usize;
            let name = std::str::from_utf8(&bytes[offset + 30..offset + 30 + name_len])
                .expect("UTF-8 LFH name");
            assert_eq!(name, self.name, "the two headers must name one entry");
            assert_eq!(lfh_method, self.method);
            assert_eq!(lfh_crc, self.crc);
            let data_start = offset + 30 + name_len + extra_len;
            let data = bytes[data_start..data_start + self.size].to_vec();
            assert_eq!(
                crc32_ieee(&data),
                self.crc,
                "CRC-32 mismatch for {}",
                self.name
            );
            ParsedZipEntry {
                name: self.name,
                method: self.method,
                crc: self.crc,
                dos_date: self.dos_date,
                dos_time: self.dos_time,
                data,
            }
        }
    }

    /// `doc.kml` out of an archive built by `export_kmz`.
    fn kml_of(photos: &[KmzPhoto]) -> String {
        let kmz = export_kmz(&sample_track(), photos).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(entries[0].name, "doc.kml");
        String::from_utf8(entries[0].data.clone()).expect("UTF-8 KML")
    }

    #[test]
    fn crc32_matches_standard_check_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0x0000_0000);
    }

    #[test]
    fn export_kmz_produces_a_store_archive_this_reader_accepts() {
        // "Valid" here means: PKZIP field layout as this test reads it, and the
        // standard CRC-32 of every entry. `unzip -t` and Google Earth have not
        // been run on the output; see the module doc.
        let mut photo = photo("tokyo.jpg");
        photo.altitude_m = Some(40.0);
        photo.utc = Some(CaptureTime::Exif(TOKYO_UTC));
        photo.jpeg_bytes = vec![0xFF, JPEG_MARKER_SOI, 0xFF, JPEG_MARKER_EOI];

        let kmz = export_kmz(&sample_track(), &[photo.clone()]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "doc.kml");
        assert_eq!(entries[0].method, METHOD_STORE);
        assert_ne!(entries[0].crc, 0);

        let kml = std::str::from_utf8(&entries[0].data).expect("UTF-8 KML");
        assert!(kml.contains("http://earth.google.com/kml/2.0"));
        assert!(kml.contains("<Document>"));
        assert!(kml.contains("<LineString>"));
        assert!(kml.contains("<tessellate>1</tessellate>"));
        assert!(kml.contains("139.76712500,35.68123600,40.00"));

        assert_eq!(entries[1].name, "photo/tokyo.jpg");
        assert_eq!(entries[1].method, METHOD_STORE);
        assert_eq!(entries[1].data, photo.jpeg_bytes);
    }

    #[test]
    fn the_track_style_carries_the_ez_tour_colour_and_width() {
        let kml = kml_of(&[]);
        assert!(kml.contains("<Style id=\"trackStyle\">"));
        // aabbggrr: 0.7 opacity over purple, width 5 (spec §5).
        assert!(
            kml.contains("<color>b3ff0080</color>"),
            "the KML must carry the colour as aabbggrr: {kml}"
        );
        assert!(kml.contains("<width>5</width>"));
        assert!(kml.contains("<styleUrl>#trackStyle</styleUrl>"));
    }

    #[test]
    fn a_photo_placemark_is_a_point_at_its_coordinates() {
        let mut photo = photo("point.jpg");
        photo.lat = 34.693_7;
        photo.lon = 135.502_3;
        photo.altitude_m = Some(25.5);
        let kml = kml_of(&[photo]);
        assert!(kml.contains("<Placemark>"));
        assert!(
            kml.contains(
                "<Point><coordinates>135.50230000,34.69370000,25.50</coordinates></Point>"
            ),
            "lon,lat,alt in one <Point>: {kml}"
        );
        assert!(
            !kml.contains(",0.00</coordinates></Point>"),
            "the measured altitude must not be replaced by the placeholder"
        );
    }

    #[test]
    fn a_photo_without_an_altitude_is_written_at_zero_metres() {
        // The three-value triple has no "unknown"; a two-value pair would be
        // read as terrain height. The placeholder is fixed by this test so a
        // change to it is a decision, not a drift.
        let kml = kml_of(&[photo("no-alt.jpg")]);
        assert!(
            kml.contains("<Point><coordinates>139.76712500,35.68123600,0.00</coordinates></Point>"),
            "a missing altitude is 0.00 m: {kml}"
        );
    }

    #[test]
    fn only_a_camera_time_becomes_a_timestamp_in_the_kml() {
        let mut from_camera = photo("camera.jpg");
        from_camera.utc = Some(CaptureTime::Exif(TOKYO_UTC));
        let kml = kml_of(&[from_camera]);
        assert!(
            kml.contains("<TimeStamp><when>2023-11-14T22:13:20Z</when></TimeStamp>"),
            "an Exif capture time must be written: {kml}"
        );

        let mut from_mtime = photo("mtime.jpg");
        from_mtime.utc = Some(CaptureTime::FileModified(TOKYO_UTC));
        let kml = kml_of(&[from_mtime]);
        assert!(
            !kml.contains("<TimeStamp>"),
            "a modification time is not when the photo was taken: {kml}"
        );

        let kml = kml_of(&[photo("untimed.jpg")]);
        assert!(!kml.contains("<TimeStamp>"));
    }

    #[test]
    fn a_file_modified_time_still_stamps_the_zip_entry() {
        // The ZIP entry needs some time; the file's own is the honest one even
        // though the KML must not present it as a capture time.
        let mut from_mtime = photo("mtime.jpg");
        from_mtime.utc = Some(CaptureTime::FileModified(TOKYO_UTC));
        let kmz = export_kmz(&sample_track(), &[from_mtime]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let photo_entry = entries.get(1).expect("the photo entry");
        // 2023-11-14 -> DOS date: (2023-1980) << 9 | 11 << 5 | 14.
        assert_eq!(photo_entry.dos_date, (43 << 9) | (11 << 5) | 14);
        assert_ne!(photo_entry.dos_time, 0, "22:13:20 is not midnight");
    }

    #[test]
    fn a_photo_longer_than_the_size_limit_is_reduced_to_it() {
        // kmzMaxPhotoEdgeSize (spec §6.3): the limit is a target, not a filter.
        // Landscape 100x50 with a 40-pixel limit becomes 40x20, aspect kept.
        let original = synthetic_jpeg(100, 50);
        let mut big = photo("big.jpg");
        big.jpeg_bytes = original.clone();
        big.max_edge_pixels = Some(40);
        let kmz = export_kmz(&sample_track(), &[big]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let stored = entries.get(1).expect("the photo entry").data.clone();
        assert_ne!(stored, original, "an oversized photo must be re-encoded");
        assert_eq!(
            jpeg_dimensions(&stored),
            Some((40, 20)),
            "the long edge becomes the limit and the short edge keeps the ratio"
        );
        assert_eq!(decoded_dimensions(&stored), (40, 20));
    }

    #[test]
    fn a_portrait_photo_over_the_limit_is_reduced_through_its_height() {
        // 50x100 with a 40-pixel limit becomes 20x40: the height is the long
        // edge, so scaling the width alone would leave the photo over the limit.
        let original = synthetic_jpeg(50, 100);
        let mut portrait = photo("portrait.jpg");
        portrait.jpeg_bytes = original.clone();
        portrait.max_edge_pixels = Some(40);
        let kmz = export_kmz(&sample_track(), &[portrait]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let stored = entries.get(1).expect("the photo entry").data.clone();
        assert_ne!(stored, original, "an oversized photo must be re-encoded");
        assert_eq!(jpeg_dimensions(&stored), Some((20, 40)));
        assert_eq!(decoded_dimensions(&stored), (20, 40));
    }

    #[test]
    fn a_photo_exactly_at_the_limit_keeps_its_bytes() {
        // The limit is a maximum. A photo already within it must not be
        // re-encoded: that would drop its EXIF and lose quality for nothing.
        let original = synthetic_jpeg(100, 50);
        let mut exact = photo("exact.jpg");
        exact.jpeg_bytes = original.clone();
        exact.max_edge_pixels = Some(100);
        let kmz = export_kmz(&sample_track(), &[exact]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(
            entries.get(1).map(|entry| entry.data.clone()),
            Some(original),
            "a photo exactly at the limit keeps its exact bytes"
        );
    }

    #[test]
    fn a_photo_with_no_limit_keeps_its_bytes() {
        let original = synthetic_jpeg(100, 50);
        let mut unlimited = photo("unlimited.jpg");
        unlimited.jpeg_bytes = original.clone();
        unlimited.max_edge_pixels = None;
        let kmz = export_kmz(&sample_track(), &[unlimited]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(
            entries.get(1).map(|entry| entry.data.clone()),
            Some(original),
            "without a limit the bytes are stored unchanged"
        );
    }

    #[test]
    fn an_oversized_photo_that_cannot_be_decoded_is_reported_not_embedded() {
        // A SOF0 header that claims 4000x3000 but has no scan data: measurable,
        // undecodable. Falling back to the full-size bytes would ignore the
        // limit the caller set, so this must be an error naming the photo.
        let mut broken = photo("broken.jpg");
        broken.jpeg_bytes = jpeg_with_size(4000, 3000);
        broken.max_edge_pixels = Some(1024);
        let err = export_kmz(&sample_track(), &[broken]).expect_err("undecodable must fail");
        let message = err.to_string();
        assert!(
            message.contains("broken.jpg") && message.contains("1024"),
            "the error must name the photo and the limit: {message}"
        );
        assert!(
            message.contains("could not be decoded"),
            "the error must say the decode failed: {message}"
        );
        assert!(matches!(err, Error::State(_)));
    }

    #[test]
    fn only_the_oversized_photo_of_a_batch_is_re_encoded() {
        // One archive, one reduced photo and one untouched photo: the reduction
        // must be decided per photo, not for the whole export.
        let small_bytes = synthetic_jpeg(100, 50);
        let large_bytes = synthetic_jpeg(200, 100);

        let mut small = photo("small.jpg");
        small.jpeg_bytes = small_bytes.clone();
        small.max_edge_pixels = Some(150);
        let mut large = photo("large.jpg");
        large.jpeg_bytes = large_bytes.clone();
        large.max_edge_pixels = Some(150);

        let kmz = export_kmz(&sample_track(), &[small, large]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(
            entries.get(1).map(|entry| entry.data.clone()),
            Some(small_bytes),
            "the photo under the limit keeps its bytes"
        );
        let reduced = entries.get(2).expect("the second photo entry").data.clone();
        assert_ne!(reduced, large_bytes);
        assert_eq!(decoded_dimensions(&reduced), (150, 75));
    }

    /// The archive entry names, in the order the writer emitted them.
    fn entry_names(kmz: &[u8]) -> Vec<String> {
        parse_zip_and_verify(kmz)
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }

    #[test]
    fn asking_for_icons_writes_one_icon_entry_for_each_photo() {
        // Spec §6.3: photos under `photo/`, thumbnails under `icon/`. The spec
        // does not say how the KML refers to a thumbnail, so the balloon keeps
        // pointing at `photo/` and no `<IconStyle>` is invented.
        let mut small = photo("small.jpg");
        small.jpeg_bytes = synthetic_jpeg(120, 60);
        let mut large = photo("large.jpg");
        large.jpeg_bytes = synthetic_jpeg(200, 100);

        let kmz = export_kmz_with_icons(
            &sample_track(),
            &[small, large],
            &KmzIconOptions {
                max_edge_pixels: 64,
            },
        )
        .expect("export kmz with icons");
        let entries = parse_zip_and_verify(&kmz);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "doc.kml",
                "photo/small.jpg",
                "photo/large.jpg",
                "icon/small.jpg",
                "icon/large.jpg",
            ],
            "the icons follow the photos and pair with them by name"
        );

        let kml = std::str::from_utf8(&entries[0].data).expect("UTF-8 KML");
        assert!(kml.contains("<img src=\"photo/small.jpg\"/>"));
        assert!(
            !kml.contains("icon/"),
            "the spec does not say the KML references icon/, so it must not: {kml}"
        );
    }

    #[test]
    fn export_kmz_still_writes_no_icon_entries() {
        // Regression: adding the thumbnail path must not change the archive a
        // caller of the pre-existing function gets.
        let kmz = export_kmz(&sample_track(), &[photo("only.jpg")]).expect("export kmz");
        assert_eq!(entry_names(&kmz), vec!["doc.kml", "photo/only.jpg"]);
    }

    #[test]
    fn an_icon_thumbnail_is_reduced_to_the_icon_long_edge_not_the_photo_limit() {
        // The two limits are independent: the photo entry is reduced to its own
        // limit and the icon to the icon limit, from the same original bytes.
        let original = synthetic_jpeg(200, 100);
        let mut big = photo("big.jpg");
        big.jpeg_bytes = original.clone();
        big.max_edge_pixels = Some(100);

        let kmz = export_kmz_with_icons(
            &sample_track(),
            &[big],
            &KmzIconOptions {
                max_edge_pixels: 50,
            },
        )
        .expect("export kmz with icons");
        let entries = parse_zip_and_verify(&kmz);
        let photo_bytes = entries.get(1).expect("the photo entry").data.clone();
        let icon_bytes = entries.get(2).expect("the icon entry").data.clone();
        assert_eq!(jpeg_dimensions(&photo_bytes), Some((100, 50)));
        assert_eq!(jpeg_dimensions(&icon_bytes), Some((50, 25)));
        assert_eq!(decoded_dimensions(&icon_bytes), (50, 25));
        assert_ne!(
            icon_bytes, photo_bytes,
            "the icon is its own reduction, not a copy of the photo entry"
        );
    }

    #[test]
    fn a_photo_within_the_icon_limit_keeps_its_bytes_in_both_directories() {
        // The limit is a maximum, exactly as for the photo entry: re-encoding a
        // photo already small enough would drop its EXIF for no gain.
        let original = synthetic_jpeg(100, 50);
        let mut fits = photo("fits.jpg");
        fits.jpeg_bytes = original.clone();

        let kmz = export_kmz_with_icons(
            &sample_track(),
            &[fits],
            &KmzIconOptions {
                max_edge_pixels: 100,
            },
        )
        .expect("export kmz with icons");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(
            entries.get(1).map(|entry| entry.data.clone()),
            Some(original.clone()),
            "the photo entry keeps its bytes"
        );
        assert_eq!(
            entries.get(2).map(|entry| entry.data.clone()),
            Some(original),
            "a photo exactly at the icon limit keeps its exact bytes"
        );
    }

    #[test]
    fn a_portrait_photo_icon_is_reduced_through_its_height() {
        // 50x100 with a 40-pixel icon limit becomes 20x40: measuring only the
        // width would leave the thumbnail over the limit.
        let mut portrait = photo("portrait.jpg");
        portrait.jpeg_bytes = synthetic_jpeg(50, 100);
        let kmz = export_kmz_with_icons(
            &sample_track(),
            &[portrait],
            &KmzIconOptions {
                max_edge_pixels: 40,
            },
        )
        .expect("export kmz with icons");
        let entries = parse_zip_and_verify(&kmz);
        let icon_bytes = entries.get(2).expect("the icon entry").data.clone();
        assert_eq!(jpeg_dimensions(&icon_bytes), Some((20, 40)));
        assert_eq!(decoded_dimensions(&icon_bytes), (20, 40));
    }

    #[test]
    fn icon_entries_use_the_same_deduplicated_name_as_their_photo_entries() {
        // Two photos with one name must stay paired: the sanitiser and the
        // de-duplicator run once and both directories use the result.
        let photos = [photo("same.jpg"), photo("same.jpg")];
        let kmz = export_kmz_with_icons(
            &sample_track(),
            &photos,
            &KmzIconOptions {
                max_edge_pixels: 32,
            },
        )
        .expect("export kmz with icons");
        assert_eq!(
            entry_names(&kmz),
            vec![
                "doc.kml",
                "photo/same.jpg",
                "photo/same_1.jpg",
                "icon/same.jpg",
                "icon/same_1.jpg",
            ]
        );
    }

    #[test]
    fn an_undecodable_icon_thumbnail_is_reported_rather_than_copied() {
        // A SOF0 header that claims 4000x3000 but has no scan data. Storing the
        // full-size bytes as the thumbnail would silently ignore the requested
        // limit, so it is an error that names the photo and the icon limit.
        let mut broken = photo("broken.jpg");
        broken.jpeg_bytes = jpeg_with_size(4000, 3000);
        let err = export_kmz_with_icons(
            &sample_track(),
            &[broken],
            &KmzIconOptions {
                max_edge_pixels: 256,
            },
        )
        .expect_err("an undecodable thumbnail must fail");
        let message = err.to_string();
        assert!(
            message.contains("broken.jpg") && message.contains("256"),
            "the error must name the photo and the icon limit: {message}"
        );
        assert!(
            message.contains("icon/") && message.contains("could not be decoded"),
            "the error must say which copy failed and why: {message}"
        );
        assert!(matches!(err, Error::State(_)));
    }

    #[test]
    fn an_icon_limit_of_zero_pixels_is_refused() {
        let err = export_kmz_with_icons(
            &sample_track(),
            &[photo("zero.jpg")],
            &KmzIconOptions { max_edge_pixels: 0 },
        )
        .expect_err("zero pixels is not a thumbnail");
        assert!(matches!(err, Error::State(_)));
        assert!(
            err.to_string().contains("at least one pixel"),
            "the error must say what a usable limit is: {err}"
        );
    }

    /// A colour-ramp JPEG with a two-pixel checker, smooth in both directions.
    ///
    /// A flat-colour image compresses the same at every quality, so it cannot
    /// tell quality 85 from 1. The ramp (red rising, blue falling across x)
    /// and the checker (green alternating every two pixels in x and y) make
    /// both reducers' choices measurable: the checker is finer than the 2x
    /// downscale grid, yet survives Lanczos3 as a mid-grey instead of aliasing.
    fn detailed_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            let ramp = u8::try_from(x * 255 / width.max(1)).unwrap_or(0);
            let checker = if ((x + y) / 2) % 2 == 0 { 255 } else { 0 };
            image::Rgb([ramp, checker, 255 - ramp])
        });
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
            Cursor::new(&mut out),
            RESIZE_JPEG_QUALITY,
        );
        encoder
            .encode_image(&image)
            .expect("encode a detailed JPEG");
        out
    }

    #[test]
    fn the_reduction_keeps_detail_rather_than_flattening_it() {
        // Two settings that are choices, not accidents: JPEG quality 85 (not
        // something far lower) and Lanczos3 resampling (not nearest-neighbour).
        // Measured with this fixture, reduction by Lanczos3 then quality 85
        // keeps a green range of about 128; nearest-neighbour then quality 85
        // flattens it to about 1, and Lanczos3 then quality 1 to about 43.
        // The thresholds below leave headroom on both sides of 128.
        let original = detailed_jpeg(200, 200);
        let mut shot = photo("detail.jpg");
        shot.jpeg_bytes = original;
        shot.max_edge_pixels = Some(100);
        let kmz = export_kmz(&sample_track(), &[shot]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let stored = entries.get(1).expect("the photo entry").data.clone();

        let reduced = image::load_from_memory_with_format(&stored, image::ImageFormat::Jpeg)
            .expect("a reduced photo must decode")
            .to_rgb8();
        assert_eq!(reduced.dimensions(), (100, 100));

        let channel_range = |channel: usize| {
            let values = reduced.pixels().map(|pixel| u32::from(pixel.0[channel]));
            let min = values.clone().min().unwrap_or(u32::MAX);
            let max = values.max().unwrap_or(0);
            max.saturating_sub(min)
        };
        assert!(
            channel_range(0) > 150,
            "the red ramp survived: range {}",
            channel_range(0)
        );
        assert!(
            channel_range(2) > 150,
            "the blue ramp survived: range {}",
            channel_range(2)
        );
        assert!(
            channel_range(1) > 100,
            "the checker survived as texture rather than being flattened: range {}",
            channel_range(1)
        );
        assert!(
            channel_range(1) < 160,
            "the checker was resampled smoothly rather than aliased: range {}",
            channel_range(1)
        );
    }

    #[test]
    fn scale_short_edge_rounds_to_nearest_and_never_reaches_zero() {
        assert_eq!(scale_short_edge(50, 100, 40), 20);
        assert_eq!(scale_short_edge(100, 200, 41), 21); // 20.5 rounds up
        assert_eq!(scale_short_edge(1, 10_000, 1), 1); // not 0
        assert_eq!(scale_short_edge(7, 0, 10), 1); // degenerate guard
    }

    #[test]
    fn a_photo_within_the_size_limit_is_embedded_unchanged() {
        let mut fits = photo("fits.jpg");
        fits.jpeg_bytes = jpeg_with_size(1280, 960);
        fits.max_edge_pixels = Some(1600);
        let kmz = export_kmz(&sample_track(), &[fits.clone()]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(
            entries.get(1).map(|entry| entry.data.clone()),
            Some(fits.jpeg_bytes),
            "a photo under the limit keeps its exact bytes"
        );
    }

    #[test]
    fn the_long_edge_is_the_height_when_the_photo_is_portrait() {
        // A portrait photo breaches the limit through its height; measuring
        // only the width would let it through at full size.
        let mut portrait = photo("portrait.jpg");
        portrait.jpeg_bytes = synthetic_jpeg(1080, 1920);
        portrait.max_edge_pixels = Some(1600);
        let kmz = export_kmz(&sample_track(), &[portrait]).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let stored = entries.get(1).expect("the photo entry").data.clone();
        assert_eq!(jpeg_dimensions(&stored), Some((900, 1600)));
        assert_eq!(decoded_dimensions(&stored), (900, 1600));
    }

    #[test]
    fn a_long_edge_exactly_at_the_limit_is_accepted() {
        // The limit is a maximum, not an exclusive bound: 1024 is allowed.
        let mut exact = photo("exact.jpg");
        exact.jpeg_bytes = jpeg_with_size(1024, 768);
        exact.max_edge_pixels = Some(1024);
        assert!(export_kmz(&sample_track(), &[exact]).is_ok());
    }

    #[test]
    fn a_photo_of_unknown_size_is_checked_but_not_refused() {
        // Bytes without a frame header cannot be measured. Refusing them would
        // reject every non-JPEG merely because a limit was set; the bytes are
        // embedded and the caller's limit simply does not apply.
        let mut unknown = photo("unknown.jpg");
        unknown.jpeg_bytes = vec![0x00, 0x01, 0x02];
        unknown.max_edge_pixels = Some(1024);
        assert!(export_kmz(&sample_track(), &[unknown]).is_ok());
    }

    #[test]
    fn jpeg_dimensions_read_the_frame_header() {
        assert_eq!(
            jpeg_dimensions(&jpeg_with_size(4000, 3000)),
            Some((4000, 3000))
        );
        assert_eq!(jpeg_dimensions(&jpeg_with_size(1, 1)), Some((1, 1)));
        // Not a JPEG: no SOI.
        assert_eq!(jpeg_dimensions(&[0x00, 0xFF, 0xD8]), None);
        // SOI immediately followed by EOI: no frame header.
        assert_eq!(
            jpeg_dimensions(&[0xFF, JPEG_MARKER_SOI, 0xFF, JPEG_MARKER_EOI]),
            None
        );
        // Truncated inside the APP0 segment.
        assert_eq!(jpeg_dimensions(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]), None);
        // A DHT segment is not a frame header; the SOF0 after it is.
        let mut with_dht = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00];
        with_dht.extend_from_slice(&jpeg_with_size(640, 480)[2..]);
        assert_eq!(jpeg_dimensions(&with_dht), Some((640, 480)));
    }

    #[test]
    fn export_kmz_sanitises_traversal_and_deduplicates_names() {
        let names = [
            "../../etc/passwd.jpg",
            "sub\\dir\\passwd.jpg",
            "PASSWD.JPG",
            "..",
        ];
        let photos: Vec<KmzPhoto> = names.iter().map(|name| photo(name)).collect();

        let kmz = export_kmz(&sample_track(), &photos).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "doc.kml",
                "photo/passwd.jpg",
                "photo/passwd_1.jpg",
                "photo/PASSWD_2.JPG",
                "photo/photo.jpg",
            ]
        );
        for name in &names[1..] {
            assert!(!name.contains(".."), "traversal survived in {name}");
            assert!(!name.contains('\\'), "backslash survived in {name}");
        }
        let kml = std::str::from_utf8(&entries[0].data).expect("UTF-8 KML");
        assert!(kml.contains("<img src=\"photo/passwd.jpg\"/>"));
        assert!(kml.contains("<img src=\"photo/passwd_1.jpg\"/>"));
        assert!(kml.contains("<img src=\"photo/PASSWD_2.JPG\"/>"));
        assert!(kml.contains("<img src=\"photo/photo.jpg\"/>"));
    }

    #[test]
    fn a_bracket_in_a_photo_name_cannot_close_the_description_cdata() {
        // `]]>` inside a CDATA section would end it early and make the KML
        // ill-formed. `sanitize_photo_name` replaces every `]` before the name
        // reaches the document, so no filename can close the section; this test
        // is what keeps that true. `[` and `>` survive the sanitiser on their
        // own, which is why `]>` alone is not a terminator and this input uses
        // no `!` either.
        let kml = kml_of(&[photo("a]]>b[c>.jpg")]);
        let start = kml.find("<![CDATA[").expect("a CDATA section") + "<![CDATA[".len();
        let close = kml[start..]
            .find("]]>")
            .expect("the description must close its CDATA")
            + start;
        let inside = &kml[start..close];
        assert!(
            !inside.contains("]]>"),
            "the section must not be closable from a filename: {inside}"
        );
        assert!(
            inside.contains("src=\"photo/a___b[c_.jpg\""),
            "every ] is replaced and the rest of the name is kept: {inside}"
        );
    }

    #[test]
    fn export_kmz_rejects_invalid_photo_coordinates() {
        let mut bad = photo("bad.jpg");
        bad.lat = 95.0;
        let err = export_kmz(&sample_track(), &[bad]).expect_err("invalid lat must fail");
        assert!(matches!(err, Error::State(_)));

        let mut bad_alt = photo("bad-alt.jpg");
        bad_alt.altitude_m = Some(f32::NAN);
        assert!(matches!(
            export_kmz(&sample_track(), &[bad_alt]),
            Err(Error::State(_))
        ));
    }
}
