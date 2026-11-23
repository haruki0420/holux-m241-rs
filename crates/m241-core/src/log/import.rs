//! Importing GPX 1.0/1.1 and NMEA 0183 logs back into [`Track`].
//!
//! Reads `<trkpt>` elements (with `<ele>`, `<time>`, `<speed>`) and `$GPRMC` /
//! `$GPGGA` sentences, and nothing else. `<wpt>` waypoints are **skipped**: they
//! are a different kind of object (a saved place, not a recorded position), and
//! [`crate::log::export::to_gpx`] writing them is a one-way export until
//! something decides what a waypoint should become in a track. No external XML
//! parser is used, so the GPX reader is a string walk over the tags it needs.
//!
//! `<speed_kmh>` is this toolkit's own extension — [`crate::log::export::to_gpx`]
//! writes it beside the standard `<speed>` so a round trip does not lose the
//! km/h value it stores. It is read first, before the standard `<speed>` (GPX
//! 1.0, m/s), so a file that carries both keeps the exact value rather than a
//! converted one. A GPX 1.1 `<speed>` has no unit in the specification; it is
//! read as m/s, which is what GPX 1.0 defines and what most writers emit.

use crate::error::{Error, Result};
use crate::log::exif::days_from_civil;
use crate::protocol::pmtk::parse_sentence;
use std::io::Read as _;

use crate::types::{Track, TrackPoint};

/// Parse a GPX 1.0 or 1.1 XML document into a [`Track`].
///
/// Reads every `<trkpt lat="..." lon="...">` element in document order along
/// with optional `<ele>`, `<time>` (ISO-8601 UTC `YYYY-MM-DDTHH:MM:SSZ`, plus
/// optional fractional seconds or explicit `+HH:MM`/`-HH:MM` offset), `<speed>`
/// (GPX 1.0 m/s, converted to km/h), and `<speed_kmh>` (written by
/// [`crate::log::export::to_gpx`]).
///
/// `<wpt>` waypoints and `<name>` elements are ignored; comments are removed
/// before anything is read.
///
/// # Errors
///
/// Returns [`Error::Log`] when the text does not contain a `<gpx` root element,
/// has an unclosed `<trkpt>` tag, has an `<!--` that is never closed, or carries
/// invalid coordinates. An unclosed comment is an error rather than "the rest
/// of the file is gone": the points after it would otherwise vanish silently
/// and the result would look like a track that simply ended.
pub fn import_gpx(text: &str) -> Result<Track> {
    let cleaned = strip_xml_comments(text)?;
    if !cleaned.contains("<gpx") {
        return Err(Error::Log {
            message: String::from("input is not a GPX document (missing <gpx element)"),
            offset: None,
        });
    }

    let mut points = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = cleaned
        .get(cursor..)
        .and_then(|s| find_tag_start(s, "trkpt"))
    {
        let tag_start = cursor + rel_start;
        let Some(after_tag) = cleaned.get(tag_start..) else {
            break;
        };
        let Some(open_end_rel) = find_attr_tag_end(after_tag) else {
            return Err(Error::Log {
                message: String::from("unclosed <trkpt> start tag"),
                offset: Some(tag_start),
            });
        };
        let Some(open_tag) = after_tag.get(..=open_end_rel) else {
            return Err(Error::Log {
                message: String::from("unclosed <trkpt> start tag"),
                offset: Some(tag_start),
            });
        };
        let self_closing = open_tag.ends_with("/>");
        let (lat, lon) = parse_lat_lon_attrs(open_tag, tag_start)?;

        let body_start = tag_start + open_end_rel + 1;
        let (body, next_cursor) = if self_closing {
            ("", body_start)
        } else {
            let Some(close_rel) = cleaned.get(body_start..).and_then(|s| s.find("</trkpt>")) else {
                return Err(Error::Log {
                    message: String::from("missing closing </trkpt> tag"),
                    offset: Some(tag_start),
                });
            };
            let body_end = body_start + close_rel;
            let body = cleaned.get(body_start..body_end).unwrap_or_default();
            (body, body_end + "</trkpt>".len())
        };

        let height_m = extract_element(body, "ele")
            .map(|raw| {
                raw.trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::Log {
                        message: format!("invalid <ele> value {raw:?}"),
                        offset: Some(tag_start),
                    })
            })
            .transpose()?;

        let utc = extract_element(body, "time")
            .map(|raw| {
                parse_iso8601_utc(raw.trim()).ok_or_else(|| Error::Log {
                    message: format!("invalid <time> value {raw:?}"),
                    offset: Some(tag_start),
                })
            })
            .transpose()?;

        let speed_kmh = if let Some(raw) = extract_element(body, "speed_kmh") {
            Some(parse_finite_f32(raw.trim(), "speed_kmh", tag_start)?)
        } else if let Some(raw) = extract_element(body, "speed") {
            // GPX 1.0 <speed> is in m/s.
            let mps = parse_finite_f32(raw.trim(), "speed", tag_start)?;
            Some(mps * 3.6)
        } else {
            None
        };

        points.push(TrackPoint {
            utc,
            latitude: Some(lat),
            longitude: Some(lon),
            height_m,
            speed_kmh,
            rcr: None,
        });
        cursor = next_cursor;
    }

    Ok(Track {
        points,
        damaged_blocks: Vec::new(),
    })
}

/// Parse a KML 2.0 or 2.2 document into a [`Track`].
///
/// Reads the `<coordinates>` of every `<Point>` and `<LineString>` in document
/// order: each `lon,lat[,alt]` tuple becomes a point with a position and, when
/// the tuple has a third field, a height. KML records a shape rather than a
/// time series, so `<TimeStamp>`, speed and placemark names are not read, and a
/// `<LinearRing>` (a Polygon outline) is skipped: an area outline is not a
/// recorded track.
///
/// Namespace prefixes are ignored — `<kml:coordinates>` reads the same as
/// `<coordinates>` — so the 2.0 namespace
/// (`http://earth.google.com/kml/2.0`), the 2.2 one
/// (`http://www.opengis.net/kml/2.2`) and a prefixed variant all load. Nothing
/// depends on element order beyond "a geometry contains its own
/// `<coordinates>`", which is the one order KML's schema fixes.
///
/// # Errors
///
/// Returns [`Error::Log`] when the text has no `<kml` element, carries an
/// XML comment that is never closed (the points after it would otherwise vanish
/// without a trace), leaves a geometry or `<coordinates>` element unclosed, or
/// holds a tuple that is not a finite longitude/latitude pair.
pub fn import_kml(text: &str) -> Result<Track> {
    let cleaned = strip_xml_comments(text)?;
    if find_open_tag_local(&cleaned, "kml", 0).is_none() {
        return Err(Error::Log {
            message: String::from("input is not a KML document (missing <kml element)"),
            offset: None,
        });
    }

    let mut points = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, local)) = next_kml_geometry(&cleaned, cursor) {
        let (body, after_element) = element_body(&cleaned, local, start)?;
        if let Some(coordinates_start) = find_open_tag_local(body, "coordinates", 0) {
            let (raw, _) = element_body(body, "coordinates", coordinates_start)?;
            points.extend(parse_kml_coordinates(raw, start)?);
        }
        cursor = after_element;
    }

    Ok(Track {
        points,
        damaged_blocks: Vec::new(),
    })
}

/// Parse a KMZ archive into a [`Track`].
///
/// A KMZ is a ZIP whose `doc.kml` is the document; when that name is absent the
/// first `.kml` entry is used, which is the fallback the format's readers
/// expect. Stored (method 0) and deflated (method 8) entries are read. A
/// compressed KMZ is the normal case, so refusing deflate would accept only this
/// toolkit's own archives.
///
/// The 64 MiB cap is enforced **while reading**, not after: the deflated entry
/// is read through [`std::io::Read::take`] with one byte to spare, and a stored
/// entry is measured before it is copied. A central-directory
/// `uncompressed_size` is only a claim — a 0.16 MiB archive can declare zero and
/// expand to 160 MiB — so the declared value is an early refusal, never the
/// bound itself.
///
/// The entry's CRC is **not** verified. A stored entry corrupted in transit can
/// therefore reach [`import_kml`] with plausible-looking coordinates; the KML
/// structure checks catch a broken document but not a single flipped digit. CRC
/// checking would need the CRC32 of the decompressed bytes compared against the
/// central directory's, which is not done here.
///
/// # Errors
///
/// Returns [`Error::Log`] when the bytes are not a ZIP archive, the archive has
/// no `.kml` entry, the entry uses ZIP64, encryption or a compression method
/// other than store/deflate, the entry is larger than 64 MiB uncompressed (or
/// would decompress past it), or the KML inside fails [`import_kml`]'s checks.
pub fn import_kmz(bytes: &[u8]) -> Result<Track> {
    let document = kmz_document(bytes)?;
    let text = std::str::from_utf8(&document).map_err(|_| Error::Log {
        message: String::from("KMZ doc.kml is not valid UTF-8"),
        offset: None,
    })?;
    import_kml(text)
}

/// The uncompressed `doc.kml` (or first `.kml` entry) of a KMZ archive.
fn kmz_document(bytes: &[u8]) -> Result<Vec<u8>> {
    let entries = zip_entries(bytes)?;
    let entry = entries
        .iter()
        .find(|entry| {
            entry
                .name
                .trim_start_matches("./")
                .eq_ignore_ascii_case("doc.kml")
        })
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.name.to_ascii_lowercase().trim_end().ends_with(".kml"))
        })
        .ok_or_else(|| Error::Log {
            message: String::from("KMZ archive contains no .kml document"),
            offset: None,
        })?;
    zip_entry_bytes(bytes, entry)
}

/// ZIP local-file-header signature (`PK\x03\x04`).
const ZIP_LOCAL_SIGNATURE: u32 = 0x0403_4B50;
/// ZIP central-directory-header signature (`PK\x01\x02`).
const ZIP_CENTRAL_SIGNATURE: u32 = 0x0201_4B50;
/// ZIP end-of-central-directory signature (`PK\x05\x06`).
const ZIP_EOCD_SIGNATURE: u32 = 0x0605_4B50;
/// The ZIP32 "value is in the ZIP64 record" markers.
const ZIP64_COUNT: u16 = 0xFFFF;
const ZIP64_OFFSET: u32 = 0xFFFF_FFFF;
/// Refuse a `doc.kml` larger than this once decompressed.
const MAX_KMZ_DOCUMENT_BYTES: u32 = 64 * 1024 * 1024;

/// One central-directory entry, reduced to what reading `doc.kml` needs.
struct ZipEntry {
    name: String,
    /// The general-purpose bit flag (bit 0 = encrypted).
    flags: u16,
    method: u16,
    compressed_size: u32,
    uncompressed_size: u32,
    local_offset: u32,
}

/// The general-purpose flag's "this entry is encrypted" bit.
const ZIP_FLAG_ENCRYPTED: u16 = 1 << 0;

/// Find the end-of-central-directory record, scanning back over its comment.
fn find_eocd(bytes: &[u8]) -> Option<usize> {
    let signature = ZIP_EOCD_SIGNATURE.to_le_bytes();
    let tail_len = bytes.len().min(66_000);
    let tail_start = bytes.len().checked_sub(tail_len)?;
    let tail = bytes.get(tail_start..)?;
    let mut index = tail.len().checked_sub(4)?;
    loop {
        if tail.get(index..index.checked_add(4)?) == Some(signature.as_slice()) {
            return tail_start.checked_add(index);
        }
        index = index.checked_sub(1)?;
    }
}

fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let field = bytes
        .get(offset..offset.checked_add(2).ok_or_else(zip_truncated)?)
        .ok_or_else(zip_truncated)?;
    let low = *field.first().ok_or_else(zip_truncated)?;
    let high = *field.get(1).ok_or_else(zip_truncated)?;
    Ok(u16::from_le_bytes([low, high]))
}

fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let field = bytes
        .get(offset..offset.checked_add(4).ok_or_else(zip_truncated)?)
        .ok_or_else(zip_truncated)?;
    let mut raw = [0u8; 4];
    for (slot, byte) in raw.iter_mut().zip(field.iter()) {
        *slot = *byte;
    }
    Ok(u32::from_le_bytes(raw))
}

fn zip_truncated() -> Error {
    Error::Log {
        message: String::from("KMZ archive is truncated"),
        offset: None,
    }
}

/// Read the central directory into the entries worth looking at.
fn zip_entries(bytes: &[u8]) -> Result<Vec<ZipEntry>> {
    let eocd = find_eocd(bytes).ok_or_else(|| Error::Log {
        message: String::from("not a ZIP archive (no end-of-central-directory record)"),
        offset: None,
    })?;
    let count = zip_u16(bytes, eocd.checked_add(10).ok_or_else(zip_truncated)?)?;
    let directory_offset = zip_u32(bytes, eocd.checked_add(16).ok_or_else(zip_truncated)?)?;
    if count == ZIP64_COUNT || directory_offset == ZIP64_OFFSET {
        return Err(Error::Log {
            message: String::from("KMZ archive uses ZIP64, which is not supported"),
            offset: None,
        });
    }

    let mut entries = Vec::new();
    let mut cursor = usize::try_from(directory_offset).map_err(|_| zip_truncated())?;
    for _ in 0..count {
        if zip_u32(bytes, cursor)? != ZIP_CENTRAL_SIGNATURE {
            return Err(Error::Log {
                message: String::from("KMZ central directory is malformed"),
                offset: None,
            });
        }
        let flags = zip_u16(bytes, cursor.checked_add(8).ok_or_else(zip_truncated)?)?;
        let method = zip_u16(bytes, cursor.checked_add(10).ok_or_else(zip_truncated)?)?;
        let compressed_size = zip_u32(bytes, cursor.checked_add(20).ok_or_else(zip_truncated)?)?;
        let uncompressed_size = zip_u32(bytes, cursor.checked_add(24).ok_or_else(zip_truncated)?)?;
        let name_length = usize::from(zip_u16(
            bytes,
            cursor.checked_add(28).ok_or_else(zip_truncated)?,
        )?);
        let extra_length = usize::from(zip_u16(
            bytes,
            cursor.checked_add(30).ok_or_else(zip_truncated)?,
        )?);
        let comment_length = usize::from(zip_u16(
            bytes,
            cursor.checked_add(32).ok_or_else(zip_truncated)?,
        )?);
        let local_offset = zip_u32(bytes, cursor.checked_add(42).ok_or_else(zip_truncated)?)?;
        let name_start = cursor.checked_add(46).ok_or_else(zip_truncated)?;
        let name_end = name_start
            .checked_add(name_length)
            .ok_or_else(zip_truncated)?;
        let name = bytes
            .get(name_start..name_end)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .ok_or_else(|| Error::Log {
                message: String::from("KMZ entry name is not valid UTF-8"),
                offset: None,
            })?;
        // An entry can carry ZIP64's "the real value is in the extra field"
        // markers even when the EOCD is plain ZIP32. Those values are not read,
        // so the size or offset would be a sentinel rather than a length; name
        // the real cause here instead of failing later on an absurd size.
        if compressed_size == ZIP64_OFFSET
            || uncompressed_size == ZIP64_OFFSET
            || local_offset == ZIP64_OFFSET
        {
            return Err(Error::Log {
                message: format!("KMZ entry {name:?} uses ZIP64, which is not supported"),
                offset: None,
            });
        }
        entries.push(ZipEntry {
            name: name.to_string(),
            flags,
            method,
            compressed_size,
            uncompressed_size,
            local_offset,
        });
        cursor = name_end
            .checked_add(extra_length)
            .and_then(|offset| offset.checked_add(comment_length))
            .ok_or_else(zip_truncated)?;
    }
    Ok(entries)
}

/// The uncompressed bytes of one central-directory entry.
///
/// The 64 MiB limit is applied to the bytes actually produced, not to the
/// central directory's claim: the claim is written by whoever built the
/// archive, and a small one does not make the bytes small. A stored entry is
/// measured before it is copied; a deflated one is read through
/// [`std::io::Read::take`] so the allocation cannot run past the cap.
fn zip_entry_bytes(bytes: &[u8], entry: &ZipEntry) -> Result<Vec<u8>> {
    if entry.flags & ZIP_FLAG_ENCRYPTED != 0 {
        return Err(encrypted_entry(&entry.name));
    }
    if entry.method != 0 && entry.method != 8 {
        return Err(Error::Log {
            message: format!(
                "KMZ entry {:?} uses compression method {}, which is not supported",
                entry.name, entry.method
            ),
            offset: None,
        });
    }
    if entry.uncompressed_size > MAX_KMZ_DOCUMENT_BYTES {
        return Err(Error::Log {
            message: format!(
                "KMZ entry {:?} is {} bytes uncompressed, larger than the {} byte limit",
                entry.name, entry.uncompressed_size, MAX_KMZ_DOCUMENT_BYTES
            ),
            offset: None,
        });
    }
    let local = usize::try_from(entry.local_offset).map_err(|_| zip_truncated())?;
    if zip_u32(bytes, local)? != ZIP_LOCAL_SIGNATURE {
        return Err(Error::Log {
            message: String::from("KMZ local file header is malformed"),
            offset: None,
        });
    }
    // The central directory and the local header each carry the flag; a
    // disagreement is refused rather than resolved, because an entry that is
    // encrypted according to either header cannot be read here.
    let local_flags = zip_u16(bytes, local.checked_add(6).ok_or_else(zip_truncated)?)?;
    if local_flags & ZIP_FLAG_ENCRYPTED != 0 {
        return Err(encrypted_entry(&entry.name));
    }
    let name_length = usize::from(zip_u16(
        bytes,
        local.checked_add(26).ok_or_else(zip_truncated)?,
    )?);
    let extra_length = usize::from(zip_u16(
        bytes,
        local.checked_add(28).ok_or_else(zip_truncated)?,
    )?);
    let data_start = local
        .checked_add(30)
        .and_then(|offset| offset.checked_add(name_length))
        .and_then(|offset| offset.checked_add(extra_length))
        .ok_or_else(zip_truncated)?;
    let data_end = data_start
        .checked_add(usize::try_from(entry.compressed_size).map_err(|_| zip_truncated())?)
        .ok_or_else(zip_truncated)?;
    let data = bytes.get(data_start..data_end).ok_or_else(zip_truncated)?;

    match entry.method {
        0 => {
            if data.len() > MAX_KMZ_DOCUMENT_BYTES as usize {
                return Err(too_large(&entry.name, data.len()));
            }
            Ok(data.to_vec())
        }
        _ => {
            // The extra byte makes "exactly at the limit" distinguishable from
            // "past it": `take` stops there, the decoder never allocates more.
            let capped = u64::from(MAX_KMZ_DOCUMENT_BYTES) + 1;
            let mut decoder = flate2::read::DeflateDecoder::new(data).take(capped);
            // Reserve the cap once instead of letting `read_to_end` double a
            // `Vec` up to the limit: the last doubling for a 64 MiB + 1 byte
            // document would ask for 128 MiB, and the transient growth is what
            // the cap is supposed to prevent. Untouched pages of a reservation
            // are not resident, so a small document still costs little.
            let mut document = Vec::with_capacity(MAX_KMZ_DOCUMENT_BYTES as usize + 1);
            decoder
                .read_to_end(&mut document)
                .map_err(|error| Error::Log {
                    message: format!("KMZ doc.kml could not be decompressed: {error}"),
                    offset: None,
                })?;
            if document.len() > MAX_KMZ_DOCUMENT_BYTES as usize {
                return Err(too_large(&entry.name, document.len()));
            }
            Ok(document)
        }
    }
}

fn encrypted_entry(name: &str) -> Error {
    Error::Log {
        message: format!("KMZ entry {name:?} is encrypted, which is not supported"),
        offset: None,
    }
}

fn too_large(name: &str, bytes: usize) -> Error {
    Error::Log {
        message: format!(
            "KMZ entry {name:?} is {bytes} bytes uncompressed, larger than the {} byte limit",
            MAX_KMZ_DOCUMENT_BYTES
        ),
        offset: None,
    }
}

/// The namespace-less local name of the tag whose `<` is at `start`.
///
/// Returns the index just past the name, the name after its last `:` and
/// whether the tag is an end tag. `None` when `start` does not begin a tag with
/// a name (a comment, a processing instruction, or the end of the text).
fn tag_local_name(text: &str, start: usize) -> Option<(usize, &str, bool)> {
    let rest = text.get(start..)?;
    let (offset, is_end) = if rest.starts_with("</") {
        (2usize, true)
    } else if rest.starts_with('<') {
        (1usize, false)
    } else {
        return None;
    };
    let mut cursor = start.checked_add(offset)?;
    let name_start = cursor;
    while let Some(character) = text.get(cursor..)?.chars().next() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':' | '.') {
            cursor = cursor.checked_add(character.len_utf8())?;
        } else {
            break;
        }
    }
    let name = text.get(name_start..cursor)?;
    if name.is_empty() {
        return None;
    }
    Some((cursor, name.rsplit(':').next()?, is_end))
}

fn find_open_tag_local(text: &str, local: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while let Some(relative) = text.get(cursor..)?.find('<') {
        let start = cursor.checked_add(relative)?;
        if let Some((_, name, is_end)) = tag_local_name(text, start)
            && !is_end
            && name.eq_ignore_ascii_case(local)
        {
            return Some(start);
        }
        cursor = start.checked_add(1)?;
    }
    None
}

fn find_close_tag_local(text: &str, local: &str, from: usize) -> Option<(usize, usize)> {
    let mut cursor = from;
    while let Some(relative) = text.get(cursor..)?.find("</") {
        let start = cursor.checked_add(relative)?;
        if let Some((_, name, is_end)) = tag_local_name(text, start)
            && is_end
            && name.eq_ignore_ascii_case(local)
        {
            let end = start
                .checked_add(text.get(start..)?.find('>')?)?
                .checked_add(1)?;
            return Some((start, end));
        }
        cursor = start.checked_add(2)?;
    }
    None
}

/// The text between `<local ...>` and `</...local>`, and the index after it.
///
/// A self-closing element yields an empty body. The close tag may carry a
/// namespace prefix of its own.
fn element_body<'a>(text: &'a str, local: &str, tag_start: usize) -> Result<(&'a str, usize)> {
    let tail = text.get(tag_start..).ok_or_else(|| Error::Log {
        message: format!("missing <{local}> element"),
        offset: Some(tag_start),
    })?;
    let open_end = tail.find('>').ok_or_else(|| Error::Log {
        message: format!("unclosed <{local}> start tag"),
        offset: Some(tag_start),
    })?;
    let after_open = tag_start
        .checked_add(open_end)
        .and_then(|offset| offset.checked_add(1))
        .ok_or_else(|| Error::Log {
            message: String::from("KML document is truncated"),
            offset: Some(tag_start),
        })?;
    let open_tag = text
        .get(tag_start..tag_start.checked_add(open_end).unwrap_or(tag_start))
        .unwrap_or_default();
    if open_tag.trim_end().ends_with('/') {
        return Ok(("", after_open));
    }
    let (close_start, after_close) =
        find_close_tag_local(text, local, after_open).ok_or_else(|| Error::Log {
            message: format!("unclosed <{local}> element"),
            offset: Some(tag_start),
        })?;
    let body = text
        .get(after_open..close_start)
        .ok_or_else(|| Error::Log {
            message: format!("unclosed <{local}> element"),
            offset: Some(tag_start),
        })?;
    Ok((body, after_close))
}

/// The next `<Point>` or `<LineString>` in document order, and its local name.
///
/// One scan from `from`, not one scan per candidate: looking for both tags
/// separately makes a Point-only document quadratic, because every point pays
/// for a full remaining-text search for the LineString that is not there.
fn next_kml_geometry(text: &str, from: usize) -> Option<(usize, &'static str)> {
    let mut cursor = from;
    while let Some(relative) = text.get(cursor..)?.find('<') {
        let start = cursor.checked_add(relative)?;
        if let Some((_, name, is_end)) = tag_local_name(text, start)
            && !is_end
        {
            if name.eq_ignore_ascii_case("point") {
                return Some((start, "point"));
            }
            if name.eq_ignore_ascii_case("linestring") {
                return Some((start, "linestring"));
            }
        }
        cursor = start.checked_add(1)?;
    }
    None
}

fn parse_kml_coordinates(raw: &str, offset: usize) -> Result<Vec<TrackPoint>> {
    let mut points = Vec::new();
    for tuple in raw.split_whitespace() {
        let mut fields = tuple.split(',');
        let longitude = parse_kml_number(fields.next(), "longitude", offset)?;
        let latitude = parse_kml_number(fields.next(), "latitude", offset)?;
        let height_m = match fields.next() {
            Some(field) if !field.is_empty() => {
                let height = parse_kml_number(Some(field), "altitude", offset)? as f32;
                if !height.is_finite() {
                    return Err(kml_coordinate_error(tuple, offset));
                }
                Some(height)
            }
            _ => None,
        };
        if fields.next().is_some() {
            return Err(kml_coordinate_error(tuple, offset));
        }
        if !(-180.0..=180.0).contains(&longitude) || !(-90.0..=90.0).contains(&latitude) {
            return Err(kml_coordinate_error(tuple, offset));
        }
        points.push(TrackPoint {
            utc: None,
            latitude: Some(latitude),
            longitude: Some(longitude),
            height_m,
            speed_kmh: None,
            rcr: None,
        });
    }
    Ok(points)
}

fn parse_kml_number(field: Option<&str>, name: &str, offset: usize) -> Result<f64> {
    let raw = field.ok_or_else(|| Error::Log {
        message: format!("KML coordinate tuple is missing its {name}"),
        offset: Some(offset),
    })?;
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| Error::Log {
            message: format!("invalid KML {name} {raw:?}"),
            offset: Some(offset),
        })
}

fn kml_coordinate_error(tuple: &str, offset: usize) -> Error {
    Error::Log {
        message: format!("invalid KML coordinate tuple {tuple:?}"),
        offset: Some(offset),
    }
}

/// What an NMEA import read, kept and dropped.
///
/// The counts separate an empty capture from a non-empty one that produced no
/// point. `dropped` deliberately mixes two causes — a sentence that could not be
/// read (broken checksum, missing fields) and a well-formed sentence that
/// carried no fix — because both mean "this line gave us no position"; the
/// message that quotes the counts names both possibilities.
#[derive(Debug, Clone)]
pub struct NmeaImport {
    pub track: Track,
    /// `$GPRMC` / `$GPGGA` lines whose fields parsed, including a merged pair.
    pub imported: usize,
    /// `$GPRMC` / `$GPGGA` lines that produced no point: a broken checksum, a
    /// no-fix sentence, or missing coordinates.
    pub dropped: usize,
    /// Lines never turned into a point: blank, free text, another kind.
    ///
    /// The address is read before the checksum, so a `$GPGSA` line with a broken
    /// checksum is still `ignored`: this reader never reads that sentence, and
    /// its checksum is not its concern.
    pub ignored: usize,
}

/// Parse NMEA 0183 text (`$GPRMC` and `$GPGGA` sentences) into a [`NmeaImport`].
///
/// Malformed lines, broken checksums, unsupported kinds and no-fix sentences are
/// skipped and counted; sentences sharing a `HHMMSS` are merged so date/speed
/// (RMC) and altitude (GGA) survive together. Only `$GPRMC` / `$GPGGA` lines can
/// be `imported` or `dropped`: the address is read first, so a line of another
/// kind is `ignored` whatever its checksum says.
///
/// # Errors
///
/// Never fails on text input: a malformed line is counted, not returned. The
/// `Result` mirrors [`import_gpx`] so a caller can handle both importers the
/// same way.
pub fn import_nmea(text: &str) -> Result<NmeaImport> {
    let mut points: Vec<TrackPoint> = Vec::new();
    let mut last_bucket: Option<String> = None;
    let mut imported = 0usize;
    let mut dropped = 0usize;
    let mut ignored = 0usize;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if !line.starts_with('$') {
            ignored += 1;
            continue;
        }
        // The address decides the counter before `parse_sentence` checks the
        // checksum: a sentence this reader does not read is `ignored` even when
        // its checksum is wrong, and only RMC/GGA lines can be `dropped`.
        if !is_position_sentence(line) {
            ignored += 1;
            continue;
        }
        let Ok(sentence) = parse_sentence(line) else {
            dropped += 1;
            continue;
        };
        let parsed = match sentence.kind.as_str() {
            // A refused sentence (no fix, bad fields) is dropped by the merge
            // and, deliberately, does not touch `last_bucket`: a `V` line
            // between two good ones must not break their pairing.
            "GPRMC" => parse_gprmc(&sentence.fields)
                .map(|(bucket, point)| Merged::Rmc(Rmc { bucket, point })),
            "GPGGA" => parse_gpgga(&sentence.fields)
                .map(|(bucket, point)| Merged::Gga(Gga { bucket, point })),
            // A sentence of another kind is not one this reader failed to read;
            // it carries no position this importer could use.
            _ => {
                ignored += 1;
                continue;
            }
        };
        if parsed.is_some() {
            imported += 1;
        } else {
            dropped += 1;
        }
        merge_sentence(&mut points, &mut last_bucket, parsed);
    }

    Ok(NmeaImport {
        track: Track {
            points,
            damaged_blocks: Vec::new(),
        },
        imported,
        dropped,
        ignored,
    })
}

/// Is `line`'s address `$GPRMC` or `$GPGGA`?
///
/// Reads only the address field, before the checksum is verified: this decides
/// which counter a line belongs to, and a sentence of another kind must count as
/// ignored whether or not its checksum is valid.
fn is_position_sentence(line: &str) -> bool {
    line.strip_prefix('$')
        .and_then(|rest| rest.split(',').next())
        .is_some_and(|kind| matches!(kind, "GPRMC" | "GPGGA"))
}

/// One parsed RMC sentence: its `HHMMSS` bucket and the point it carries.
struct Rmc {
    bucket: String,
    point: TrackPoint,
}

/// One parsed GGA sentence.
struct Gga {
    bucket: String,
    point: TrackPoint,
}

/// A sentence that parsed, of either kind.
enum Merged {
    Rmc(Rmc),
    Gga(Gga),
}

/// Add `sentence` to `points`, merging it into the previous point when both
/// share the `HHMMSS` bucket.
///
/// `last_bucket` is the bucket of the point at the end of `points`, and it is
/// the whole key: the RMC date is not part of it, so two sentences with the
/// same `HHMMSS` on different days (a log that spans midnight) would merge. NMEA
/// writes `HHMMSS` for both sentence kinds and the date only in RMC, so this is
/// the finest key the two kinds share.
///
/// Only the tags each kind owns are copied — `utc` and `speed_kmh` from RMC,
/// `height_m` from GGA — so the order the kinds arrive in does not decide which
/// fields survive.
fn merge_sentence(
    points: &mut Vec<TrackPoint>,
    last_bucket: &mut Option<String>,
    parsed: Option<Merged>,
) {
    let Some(merged) = parsed else {
        return;
    };
    let (bucket, point, height_only) = match merged {
        Merged::Rmc(rmc) => (rmc.bucket, rmc.point, false),
        Merged::Gga(gga) => (gga.bucket, gga.point, true),
    };
    let same_bucket = last_bucket.as_deref() == Some(bucket.as_str());
    if same_bucket && let Some(prev) = points.last_mut() {
        if height_only {
            if prev.height_m.is_none() {
                prev.height_m = point.height_m;
            }
        } else {
            if prev.utc.is_none() {
                prev.utc = point.utc;
            }
            if prev.speed_kmh.is_none() {
                prev.speed_kmh = point.speed_kmh;
            }
        }
        return;
    }
    *last_bucket = Some(bucket);
    points.push(point);
}

fn parse_gprmc(fields: &[String]) -> Option<(String, TrackPoint)> {
    let time_str = fields.first()?.trim();
    let status = fields.get(1)?.trim();
    if status != "A" {
        return None;
    }
    let lat_str = fields.get(2)?.trim();
    let lat_hemi = fields.get(3)?.trim();
    let lon_str = fields.get(4)?.trim();
    let lon_hemi = fields.get(5)?.trim();
    let speed_str = fields.get(6).map(String::as_str).unwrap_or("").trim();
    let date_str = fields.get(8)?.trim();

    let (hhmmss, hour, minute, second) = parse_nmea_hhmmss(time_str)?;
    let (day, month, year) = parse_nmea_ddmmyy(date_str)?;
    let days = days_from_civil(year, month, day)?;
    let unix_i64 = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let utc = u32::try_from(unix_i64).ok()?;

    let lat = parse_nmea_coord(lat_str, lat_hemi, 'N', 'S', 90.0)?;
    let lon = parse_nmea_coord(lon_str, lon_hemi, 'E', 'W', 180.0)?;
    let speed_kmh = if speed_str.is_empty() {
        None
    } else {
        let knots: f64 = speed_str.parse().ok()?;
        knots.is_finite().then_some((knots * 1.852) as f32)
    };

    Some((
        hhmmss,
        TrackPoint {
            utc: Some(utc),
            latitude: Some(lat),
            longitude: Some(lon),
            height_m: None,
            speed_kmh,
            rcr: None,
        },
    ))
}

fn parse_gpgga(fields: &[String]) -> Option<(String, TrackPoint)> {
    let time_str = fields.first()?.trim();
    let lat_str = fields.get(1)?.trim();
    let lat_hemi = fields.get(2)?.trim();
    let lon_str = fields.get(3)?.trim();
    let lon_hemi = fields.get(4)?.trim();
    let quality = fields.get(5)?.trim();
    if quality.is_empty() || quality == "0" {
        return None;
    }

    let (hhmmss, _, _, _) = parse_nmea_hhmmss(time_str)?;
    let lat = parse_nmea_coord(lat_str, lat_hemi, 'N', 'S', 90.0)?;
    let lon = parse_nmea_coord(lon_str, lon_hemi, 'E', 'W', 180.0)?;
    let height_m = fields
        .get(8)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite());

    Some((
        hhmmss,
        TrackPoint {
            utc: None,
            latitude: Some(lat),
            longitude: Some(lon),
            height_m,
            speed_kmh: None,
            rcr: None,
        },
    ))
}

fn parse_nmea_hhmmss(text: &str) -> Option<(String, u32, u32, u32)> {
    let base = text.split('.').next()?;
    if base.len() < 6 || !base.is_ascii() {
        return None;
    }
    let hh: u32 = base.get(0..2)?.parse().ok()?;
    let mm: u32 = base.get(2..4)?.parse().ok()?;
    let ss: u32 = base.get(4..6)?.parse().ok()?;
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some((base.get(0..6)?.to_owned(), hh, mm, ss))
}

fn parse_nmea_ddmmyy(text: &str) -> Option<(u32, u32, i64)> {
    if text.len() != 6 || !text.is_ascii() {
        return None;
    }
    let day: u32 = text.get(0..2)?.parse().ok()?;
    let month: u32 = text.get(2..4)?.parse().ok()?;
    let yy: i64 = text.get(4..6)?.parse().ok()?;
    // NMEA keeps two digits of year; it has no century of its own. 80 is where
    // the century rolls, so a logger's log stays in 1980-2079.
    let year = if yy >= 80 { 1900 + yy } else { 2000 + yy };
    Some((day, month, year))
}

fn parse_nmea_coord(
    raw: &str,
    hemi: &str,
    pos_hemi: char,
    neg_hemi: char,
    max_abs: f64,
) -> Option<f64> {
    if raw.is_empty() {
        return None;
    }
    // The hemisphere is a one-character string; anything else (empty, two
    // letters, lower case) is a sentence this parser cannot sign and is refused.
    let mut letters = hemi.chars();
    let first = letters.next();
    let sign = if first.is_some() && letters.next().is_none() {
        match first {
            Some(f) if f == pos_hemi => 1.0,
            Some(f) if f == neg_hemi => -1.0,
            _ => return None,
        }
    } else {
        return None;
    };

    let split_pos = match raw.find('.') {
        Some(dot) => dot.checked_sub(2)?,
        None => raw.len().checked_sub(2)?,
    };
    let (deg_str, min_str) = raw.split_at_checked(split_pos)?;
    let deg: f64 = deg_str.parse().ok()?;
    let min: f64 = min_str.parse().ok()?;
    if !deg.is_finite() || !min.is_finite() || min < 0.0 || min >= 60.0 {
        return None;
    }
    let decimal = sign * (deg + min / 60.0);
    (decimal.is_finite() && decimal.abs() <= max_abs).then_some(decimal)
}

// ---------------------------------------------------------------------------
// String-based GPX search (no XML parser is pulled in; see the module doc)
// ---------------------------------------------------------------------------

/// Remove `<!-- ... -->` comments, keeping everything else as it was.
///
/// # Errors
///
/// [`Error::Log`] when an `<!--` is never closed. Cutting the input there would
/// drop every point after it and return a short track that looks complete, so
/// the malformed document is refused instead.
fn strip_xml_comments(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + 4..).unwrap_or_default();
        let Some(end) = after.find("-->") else {
            return Err(Error::Log {
                message: String::from("a <!-- comment is never closed with -->"),
                offset: Some(out.len() + start),
            });
        };
        rest = after.get(end + 3..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

fn find_tag_start(haystack: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut offset = 0usize;
    while let Some(found) = haystack.get(offset..).and_then(|s| s.find(&needle)) {
        let pos = offset + found;
        let next_char = haystack.get(pos + needle.len()..)?.chars().next()?;
        if next_char.is_ascii_whitespace() || next_char == '>' || next_char == '/' {
            return Some(pos);
        }
        offset = pos + needle.len();
    }
    None
}

fn find_attr_tag_end(tag_text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (idx, ch) in tag_text.char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '>' => return Some(idx),
            None => {}
        }
    }
    None
}

fn parse_lat_lon_attrs(tag: &str, offset: usize) -> Result<(f64, f64)> {
    let lat_str = find_attr(tag, "lat").ok_or_else(|| Error::Log {
        message: String::from("<trkpt> is missing lat attribute"),
        offset: Some(offset),
    })?;
    let lon_str = find_attr(tag, "lon").ok_or_else(|| Error::Log {
        message: String::from("<trkpt> is missing lon attribute"),
        offset: Some(offset),
    })?;
    let lat: f64 = lat_str.trim().parse().map_err(|_| Error::Log {
        message: format!("invalid <trkpt> lat {lat_str:?}"),
        offset: Some(offset),
    })?;
    let lon: f64 = lon_str.trim().parse().map_err(|_| Error::Log {
        message: format!("invalid <trkpt> lon {lon_str:?}"),
        offset: Some(offset),
    })?;
    if !lat.is_finite() || lat.abs() > 90.0 {
        return Err(Error::Log {
            message: format!("out-of-range <trkpt> lat {lat}"),
            offset: Some(offset),
        });
    }
    if !lon.is_finite() || lon.abs() > 180.0 {
        return Err(Error::Log {
            message: format!("out-of-range <trkpt> lon {lon}"),
            offset: Some(offset),
        });
    }
    Ok((lat, lon))
}

fn find_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(pos) = rest.find(name) {
        let before_ok = pos == 0
            || rest
                .get(..pos)
                .and_then(|s| s.chars().next_back())
                .is_some_and(char::is_whitespace);
        let after = rest.get(pos + name.len()..)?;
        if before_ok {
            let trimmed = after.trim_start();
            if let Some(after_eq) = trimmed.strip_prefix('=') {
                let after_eq = after_eq.trim_start();
                let quote = after_eq.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let inner = after_eq.get(1..)?;
                    let end = inner.find(quote)?;
                    return inner.get(..end);
                }
            }
        }
        rest = after;
    }
    None
}

fn extract_element<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let start = find_tag_start(body, name)?;
    let after_name = body.get(start..)?;
    let open_end = after_name.find('>')?;
    if after_name.get(..open_end)?.ends_with('/') {
        return None;
    }
    let content_start = start + open_end + 1;
    let close_tag = format!("</{name}>");
    let close_rel = body.get(content_start..)?.find(&close_tag)?;
    body.get(content_start..content_start + close_rel)
}

fn parse_finite_f32(raw: &str, name: &str, offset: usize) -> Result<f32> {
    raw.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::Log {
            message: format!("invalid <{name}> value {raw:?}"),
            offset: Some(offset),
        })
}

fn parse_iso8601_utc(text: &str) -> Option<u32> {
    let (date_str, time_and_zone) = text.split_once('T').or_else(|| text.split_once('t'))?;
    let mut date_parts = date_str.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let (time_str, tz_offset_secs) = if let Some(base) = time_and_zone
        .strip_suffix('Z')
        .or_else(|| time_and_zone.strip_suffix('z'))
    {
        (base, 0i64)
    } else if let Some((base, off)) = time_and_zone.rsplit_once('+') {
        (base, parse_tz_offset(off, 1)?)
    } else if let Some((base, off)) = time_and_zone.rsplit_once('-') {
        (base, parse_tz_offset(off, -1)?)
    } else {
        (time_and_zone, 0i64)
    };

    let mut clock = time_str.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let sec_part = clock.next()?;
    if clock.next().is_some() {
        return None;
    }
    let sec_whole = sec_part.split('.').next()?;
    let second: u32 = sec_whole.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day)?;
    let local_secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let utc_secs = local_secs.checked_sub(tz_offset_secs)?;
    u32::try_from(utc_secs).ok()
}

fn parse_tz_offset(text: &str, sign: i64) -> Option<i64> {
    let (hh_str, mm_str) = text.split_once(':')?;
    let hh: i64 = hh_str.parse().ok()?;
    let mm: i64 = mm_str.parse().ok()?;
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) {
        return None;
    }
    Some(sign * (hh * 3_600 + mm * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::export::{to_gpx, to_nmea};

    fn sample_track() -> Track {
        Track {
            points: vec![
                TrackPoint {
                    utc: Some(1_700_000_000),
                    latitude: Some(35.681_236),
                    longitude: Some(139.767_125),
                    height_m: Some(42.5),
                    speed_kmh: Some(18.5),
                    rcr: Some(0x0008),
                },
                TrackPoint {
                    utc: Some(1_700_000_010),
                    latitude: Some(34.693_7),
                    longitude: Some(135.502_3),
                    height_m: Some(15.25),
                    speed_kmh: Some(36.0),
                    rcr: Some(0),
                },
            ],
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn import_gpx_round_trips_with_export_to_gpx() {
        let original = sample_track();
        let gpx = to_gpx(&original);
        let imported = import_gpx(&gpx).expect("import exported gpx");
        assert_eq!(imported.points.len(), 2);

        for (got, want) in imported.points.iter().zip(&original.points) {
            assert_eq!(got.utc, want.utc);
            assert!(
                (got.latitude.unwrap() - want.latitude.unwrap()).abs() < 1e-8,
                "lat mismatch"
            );
            assert!(
                (got.longitude.unwrap() - want.longitude.unwrap()).abs() < 1e-8,
                "lon mismatch"
            );
            assert!((got.height_m.unwrap() - want.height_m.unwrap()).abs() < 0.01);
            assert!((got.speed_kmh.unwrap() - want.speed_kmh.unwrap()).abs() < 0.01);
        }
    }

    #[test]
    fn import_gpx_supports_gpx10_speed_and_self_closing_and_offsets() {
        let gpx10 = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.0" creator="test">
  <trk><trkseg>
    <trkpt lon='-70.25' lat="-35.5">
      <ele>120.5</ele>
      <time>2023-11-15T07:13:20.500+09:00</time>
      <speed>10.0</speed>
    </trkpt>
    <trkpt lat="35.681236" lon="139.767125"/>
  </trkseg></trk>
</gpx>"#;
        let track = import_gpx(gpx10).expect("import gpx 1.0");
        assert_eq!(track.points.len(), 2);
        assert_eq!(track.points[0].utc, Some(1_700_000_000));
        assert!((track.points[0].latitude.unwrap() - (-35.5)).abs() < 1e-9);
        assert!((track.points[0].longitude.unwrap() - (-70.25)).abs() < 1e-9);
        assert_eq!(track.points[0].height_m, Some(120.5));
        assert!((track.points[0].speed_kmh.unwrap() - 36.0).abs() < 1e-4);

        assert_eq!(track.points[1].utc, None);
        assert_eq!(track.points[1].height_m, None);
        assert!((track.points[1].latitude.unwrap() - 35.681_236).abs() < 1e-9);
    }

    #[test]
    fn a_gpx10_speed_in_metres_per_second_arrives_as_km_per_hour() {
        // The conversion is the whole of the GPX 1.0 `<speed>` contract, so it
        // gets its own assertion (and its own test): a file with a single point
        // leaves nothing else for a survivor to hide behind.
        let gpx = r#"<gpx version="1.0"><trk><trkseg>
            <trkpt lat="35.681236" lon="139.767125"><speed>10.0</speed></trkpt>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(gpx).expect("import gpx 1.0");
        let speed = track.points[0].speed_kmh.expect("a speed");
        assert!(
            (speed - 36.0).abs() < 1e-4,
            "10 m/s is 36 km/h, got {speed}"
        );
    }

    #[test]
    fn the_speed_kmh_extension_wins_over_the_standard_speed() {
        // `to_gpx` writes both; reading the extension first keeps the km/h
        // value exact instead of converting the m/s one back.
        let gpx = r#"<gpx version="1.1"><trk><trkseg>
            <trkpt lat="0.0" lon="0.0"><speed>1.0</speed><speed_kmh>42.5</speed_kmh></trkpt>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(gpx).expect("import");
        assert_eq!(track.points[0].speed_kmh, Some(42.5));
    }

    #[test]
    fn an_unclosed_xml_comment_is_an_error_not_a_truncated_track() {
        let doc = r#"<gpx><trk><trkseg>
            <trkpt lat="35.0" lon="139.0"/><!-- never closed
        </trkseg></trk></gpx>"#;
        let err = import_gpx(doc).expect_err("an unclosed comment is malformed");
        assert!(matches!(err, Error::Log { .. }), "got {err:?}");
        assert!(err.to_string().contains("never closed"), "got {err}");
    }

    #[test]
    fn a_closed_comment_is_removed_without_touching_the_points() {
        let doc = r#"<gpx><!-- <trkpt lat="91.0" lon="0.0"/> --><trk><trkseg>
            <trkpt lat="35.0" lon="139.0"/>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(doc).expect("import");
        assert_eq!(track.points.len(), 1, "a commented-out point is not read");
        assert!((track.points[0].latitude.unwrap() - 35.0).abs() < 1e-9);
    }

    #[test]
    fn waypoints_are_not_read_as_track_points() {
        // to_gpx writes `<wpt>` for saved places; import reads `<trkpt>` only.
        // This test pins that choice so a change to either side is visible.
        let doc = r#"<gpx><wpt lat="35.0" lon="139.0"/><trk><trkseg>
            <trkpt lat="35.1" lon="139.1"/>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(doc).expect("import");
        assert_eq!(track.points.len(), 1);
        assert!((track.points[0].latitude.unwrap() - 35.1).abs() < 1e-9);
    }

    #[test]
    fn import_gpx_rejects_malformed_input() {
        assert!(matches!(import_gpx("hello"), Err(Error::Log { .. })));
        let bad_lat = r#"<gpx><trk><trkseg><trkpt lat="91.0" lon="0.0"/></trkseg></trk></gpx>"#;
        assert!(matches!(import_gpx(bad_lat), Err(Error::Log { .. })));
        let unclosed = r#"<gpx><trk><trkseg><trkpt lat="35.0" lon="139.0">"#;
        assert!(matches!(import_gpx(unclosed), Err(Error::Log { .. })));
    }

    #[test]
    fn import_nmea_round_trips_with_export_to_nmea() {
        let original = sample_track();
        let nmea = to_nmea(&original);
        let imported = import_nmea(&nmea).expect("import exported nmea");
        // Two points, each exported as GGA + GSA + RMC: both position sentences
        // of a pair are imported (they merge into one point), and the GSA of each
        // point is a kind this reader never reads, so it is ignored.
        assert_eq!(
            (imported.imported, imported.dropped, imported.ignored),
            (4, 0, 2),
            "two GGA + two RMC imported, two GSA ignored"
        );
        assert_eq!(imported.track.points.len(), 2);

        for (got, want) in imported.track.points.iter().zip(&original.points) {
            assert_eq!(got.utc, want.utc);
            assert!(
                (got.latitude.unwrap() - want.latitude.unwrap()).abs() < 1e-4,
                "lat {:?} vs {:?}",
                got.latitude,
                want.latitude
            );
            assert!(
                (got.longitude.unwrap() - want.longitude.unwrap()).abs() < 1e-4,
                "lon {:?} vs {:?}",
                got.longitude,
                want.longitude
            );
            assert!((got.speed_kmh.unwrap() - want.speed_kmh.unwrap()).abs() < 0.05);
            // GGA writes one decimal of a metre — the resolution a GPS altitude
            // has — so a round trip keeps the height to that precision.
            assert!(
                (got.height_m.unwrap() - want.height_m.unwrap()).abs() < 0.1,
                "height {:?} vs {:?}",
                got.height_m,
                want.height_m
            );
        }
    }

    #[test]
    fn import_nmea_merges_gga_and_rmc_and_skips_bad_or_void_lines() {
        use crate::protocol::pmtk::sentence;
        let lines = [
            // Bad checksum -> skipped
            String::from("$GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,*00"),
            // Void fix -> skipped
            sentence("GPRMC,221320.00,V,,,,,0.00,,141123,,"),
            // GGA no-fix (quality 0) -> skipped
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,0,00,99.9,80.0,M,0.0,M,,"),
            // Valid GGA + RMC for same timestamp -> merged into 1 point with altitude + speed + utc
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            // Unrelated sentence -> ignored
            sentence("GPGSA,A,3,01,02,03,,,,,,,,,1.5,1.0,1.2"),
            String::from("not an nmea line"),
        ];
        let track = import_nmea(&lines.join("\r\n")).expect("import nmea").track;
        assert_eq!(track.points.len(), 1);
        let p = track.points[0];
        assert_eq!(p.utc, Some(1_700_000_000));
        assert_eq!(p.height_m, Some(80.5));
        assert!((p.speed_kmh.unwrap() - 18.52).abs() < 0.01);
        assert!((p.latitude.unwrap() - 35.681_236).abs() < 1e-4);
        assert!((p.longitude.unwrap() - 139.767_125).abs() < 1e-4);
    }

    #[test]
    fn an_rmc_before_its_gga_still_gets_the_altitude() {
        use crate::protocol::pmtk::sentence;
        // NMEA order is not fixed; a receiver that emits RMC first must still
        // yield a point with the date, the speed and the altitude.
        let text = [
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
        ]
        .join("\r\n");
        let track = import_nmea(&text).expect("import nmea").track;
        assert_eq!(track.points.len(), 1);
        let p = track.points[0];
        assert_eq!(p.utc, Some(1_700_000_000), "the RMC supplies the date");
        assert_eq!(
            p.height_m,
            Some(80.5),
            "the later GGA supplies the altitude"
        );
        assert!((p.speed_kmh.unwrap() - 18.52).abs() < 0.01);
    }

    #[test]
    fn a_void_rmc_between_two_good_ones_changes_nothing() {
        use crate::protocol::pmtk::sentence;
        // The `V` sentence repeats the same `HHMMSS` as the good RMC around it
        // and carries different coordinates. If it were read, the merge would
        // patch nothing (a void RMC has no position) and, more importantly, it
        // must not reset the pairing key for the GGA that follows.
        let text = [
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            sentence("GPRMC,221320.00,V,0000.0000,N,00000.0000,E,0.00,,141123,,"),
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
        ]
        .join("\r\n");
        let track = import_nmea(&text).expect("import nmea").track;
        assert_eq!(
            track.points.len(),
            1,
            "one place, not a phantom record for the void fix"
        );
        let p = track.points[0];
        assert!((p.latitude.unwrap() - 35.681_236).abs() < 1e-4);
        assert_eq!(p.height_m, Some(80.5));
        assert_eq!(p.utc, Some(1_700_000_000));
    }

    #[test]
    fn a_void_rmc_with_no_following_sentence_is_not_a_point() {
        use crate::protocol::pmtk::sentence;
        let text = sentence("GPRMC,221320.00,V,3540.8742,N,13946.0275,E,10.00,,141123,,");
        let track = import_nmea(&text).expect("import nmea").track;
        assert!(
            track.points.is_empty(),
            "a no-fix RMC must be dropped, not kept and overwritten: {:?}",
            track.points
        );
    }

    #[test]
    fn a_gga_of_quality_zero_is_not_a_point() {
        use crate::protocol::pmtk::sentence;
        let text = sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,0,00,99.9,80.0,M,0.0,M,,");
        assert!(import_nmea(&text).expect("import").track.points.is_empty());
    }

    #[test]
    fn nmea_hemispheres_must_be_one_of_the_four_letters() {
        // A two-letter or missing hemisphere cannot be signed; the sentence is
        // dropped rather than read as the northern/eastern hemisphere.
        let bad = [
            "$GPRMC,120000.00,A,3540.8742,NN,13946.0275,E,0.00,,141123,,",
            "$GPRMC,120000.00,A,3540.8742,x,13946.0275,E,0.00,,141123,,",
            "$GPRMC,120000.00,A,3540.8742,a,13946.0275,E,0.00,,141123,,",
        ];
        for body in bad {
            let line = crate::protocol::pmtk::sentence(body);
            assert!(
                import_nmea(&line).expect("import").track.points.is_empty(),
                "an unreadable hemisphere must drop the sentence: {body}"
            );
        }
        let south = crate::protocol::pmtk::sentence(
            "GPRMC,120000.00,A,3540.8742,S,13946.0275,W,0.00,,141123,,",
        );
        let track = import_nmea(&south).expect("import").track;
        assert!(track.points[0].latitude.unwrap() < 0.0);
        assert!(track.points[0].longitude.unwrap() < 0.0);
    }

    #[test]
    fn an_nmea_import_reports_imported_dropped_and_ignored_lines() {
        // One line of each outcome, so a reader can tell "nothing arrived" from
        // "everything was rejected" without inspecting the track.
        let lines = [
            crate::protocol::pmtk::sentence(
                "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
            ),
            // Broken checksum: dropped.
            String::from("$GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,*00"),
            // No fix: dropped.
            crate::protocol::pmtk::sentence("GPRMC,221320.00,V,,,,,0.00,,141123,,"),
            // Another sentence kind: not this reader's, counted as ignored.
            crate::protocol::pmtk::sentence("GPGSA,A,3,01,02,03,,,,,,,,,1.5,1.0,1.2"),
            // Blank and free text: ignored.
            String::new(),
            String::from("not an nmea line"),
        ];
        let imported = import_nmea(&lines.join("\r\n")).expect("import nmea");
        assert_eq!(imported.imported, 1);
        assert_eq!(imported.dropped, 2);
        assert_eq!(imported.ignored, 3);
        assert_eq!(imported.imported + imported.dropped + imported.ignored, 6);
        assert_eq!(imported.track.points.len(), 1);
    }

    #[test]
    fn an_nmea_import_counts_a_merged_pair_as_two_imported_sentences() {
        // The pair becomes one point, but both lines were read and accepted;
        // reporting one would make the counts disagree with the input.
        let text = [
            crate::protocol::pmtk::sentence(
                "GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,",
            ),
            crate::protocol::pmtk::sentence(
                "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
            ),
        ]
        .join("\r\n");
        let imported = import_nmea(&text).expect("import nmea");
        assert_eq!(imported.imported, 2);
        assert_eq!(imported.dropped, 0);
        assert_eq!(imported.track.points.len(), 1);
    }

    #[test]
    fn another_sentence_kind_with_a_broken_checksum_is_ignored_not_dropped() {
        // The address decides the counter before the checksum is verified: a
        // sentence this reader never reads is `ignored` even when its checksum
        // is wrong, and only RMC/GGA lines can be `dropped`.
        let text = [
            String::from("$GPGSA,A,3,01,02,03,,,,,,,,,1.5,1.0,1.2*00"),
            String::from("$GPGSV,1,1,00"),
            crate::protocol::pmtk::sentence(
                "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
            ),
        ]
        .join("\r\n");
        let imported = import_nmea(&text).expect("import nmea");
        assert_eq!(imported.imported, 1);
        assert_eq!(imported.dropped, 0, "another kind is not this reader's");
        assert_eq!(imported.ignored, 2);
        assert_eq!(imported.imported + imported.dropped + imported.ignored, 3);
    }

    #[test]
    fn an_empty_nmea_capture_is_zero_lines_not_an_error() {
        let imported = import_nmea("").expect("import empty nmea");
        assert!(imported.track.points.is_empty());
        assert_eq!(imported.imported + imported.dropped + imported.ignored, 0);
    }

    #[test]
    fn import_kml_reads_linestrings_and_placemark_points_with_namespace_prefixes() {
        let kml = r#"<?xml version="1.0" encoding="UTF-8"?>
<kml:kml xmlns:kml="http://www.opengis.net/kml/2.2">
  <kml:Document>
    <kml:Placemark>
      <kml:LineString><kml:coordinates>
        139.767125,35.681236,42.5
        139.780000,35.690000,38.0
      </kml:coordinates></kml:LineString>
    </kml:Placemark>
    <kml:Placemark>
      <kml:Point><kml:coordinates>139.790000,35.695000</kml:coordinates></kml:Point>
    </kml:Placemark>
  </kml:Document>
</kml:kml>"#;
        let track = import_kml(kml).expect("import a prefixed KML 2.2 document");
        assert_eq!(track.points.len(), 3);
        assert!((track.points[0].longitude.unwrap() - 139.767_125).abs() < 1e-9);
        assert!((track.points[0].latitude.unwrap() - 35.681_236).abs() < 1e-9);
        assert_eq!(track.points[0].height_m, Some(42.5));
        assert_eq!(track.points[2].height_m, None);
        assert_eq!(track.points[2].utc, None, "KML carries no timestamp here");
    }

    #[test]
    fn import_kml_accepts_kml_20_and_skips_polygon_rings() {
        let kml = r#"<kml xmlns="http://earth.google.com/kml/2.0"><Document>
  <Placemark><Point><coordinates>-70.25,-35.5,120.5</coordinates></Point></Placemark>
  <Placemark><Polygon><outerBoundaryIs><LinearRing>
    <coordinates>0,0 1,1 2,2 0,0</coordinates>
  </LinearRing></outerBoundaryIs></Polygon></Placemark>
</Document></kml>"#;
        let track = import_kml(kml).expect("import a KML 2.0 document");
        assert_eq!(track.points.len(), 1, "a polygon outline is not a track");
        assert!((track.points[0].latitude.unwrap() + 35.5).abs() < 1e-9);
        assert!((track.points[0].longitude.unwrap() + 70.25).abs() < 1e-9);
        assert_eq!(track.points[0].height_m, Some(120.5));
    }

    #[test]
    fn a_document_that_is_not_kml_is_an_error() {
        let err = import_kml("<gpx version=\"1.1\"></gpx>").expect_err("not a KML document");
        assert!(matches!(err, Error::Log { .. }), "got {err:?}");
    }

    #[test]
    fn a_coordinate_tuple_without_a_latitude_is_an_error() {
        let kml = r#"<kml><Document><Placemark><Point>
            <coordinates>139.767125</coordinates>
        </Point></Placemark></Document></kml>"#;
        let err = import_kml(kml).expect_err("one field is not a coordinate");
        assert!(matches!(err, Error::Log { .. }), "got {err:?}");
    }

    #[test]
    fn import_kmz_round_trips_an_exported_track() {
        let original = sample_track();
        let archive = crate::log::kmz::export_kmz(&original, &[]).expect("export KMZ");
        let imported = import_kmz(&archive).expect("import the exported KMZ");
        assert_eq!(imported.points.len(), original.points.len());
        assert!((imported.points[0].latitude.unwrap() - 35.681_236).abs() < 1e-9);
        assert!((imported.points[1].longitude.unwrap() - 135.502_3).abs() < 1e-9);
        assert!((imported.points[0].height_m.unwrap() - 42.5).abs() < 0.01);
    }

    #[test]
    fn import_kmz_reads_a_deflated_doc_kml() {
        let kml = r#"<kml xmlns="http://www.opengis.net/kml/2.2"><Document><Placemark>
            <Point><coordinates>139.767125,35.681236,0</coordinates></Point>
        </Placemark></Document></kml>"#;
        let archive = zip_with_one_entry("doc.kml", 8, kml.as_bytes());
        let track = import_kmz(&archive).expect("import a deflated KMZ");
        assert_eq!(track.points.len(), 1);
        assert!((track.points[0].latitude.unwrap() - 35.681_236).abs() < 1e-9);
    }

    #[test]
    fn import_kmz_falls_back_to_the_first_kml_entry() {
        let archive = zip_with_one_entry("track/trip.kml", 0, b"<kml><Document/></kml>");
        let track = import_kmz(&archive).expect("import the first .kml entry");
        assert!(track.points.is_empty());
    }

    #[test]
    fn an_archive_without_a_kml_entry_is_an_error() {
        let archive = zip_with_one_entry("readme.txt", 0, b"no track here");
        let err = import_kmz(&archive).expect_err("no .kml entry");
        assert!(err.to_string().contains("no .kml document"), "got {err}");
    }

    #[test]
    fn a_point_only_kml_is_read_in_one_pass_not_once_per_point() {
        // The quadratic reader searched the rest of the document for a
        // `<LineString>` once per `<Point>`: 10,000 points took 7 s in a release
        // build (78 s in debug). The bound is deliberately loose — the point is
        // to fail a return of the old shape, not to time the machine.
        let mut kml = String::from("<kml xmlns=\"http://www.opengis.net/kml/2.2\"><Document>");
        let count = 10_000usize;
        for index in 0..count {
            let latitude = 35.0 + f64::from(u32::try_from(index % 1000).expect("fits")) * 0.0001;
            let longitude = 139.0 + f64::from(u32::try_from(index % 1000).expect("fits")) * 0.0001;
            kml.push_str(&format!(
                "<Placemark><Point><coordinates>{longitude:.6},{latitude:.6},0</coordinates>\
                 </Point></Placemark>"
            ));
        }
        kml.push_str("</Document></kml>");

        let started = std::time::Instant::now();
        let track = import_kml(&kml).expect("import a point-only KML");
        let elapsed = started.elapsed();

        assert_eq!(track.points.len(), count);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "10,000 points took {elapsed:?}; the geometry scan is not linear"
        );
    }

    #[test]
    fn an_archive_without_an_end_of_central_directory_record_is_refused() {
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        archive.truncate(archive.len() - 22);
        let err = import_kmz(&archive).expect_err("no end-of-central-directory record");
        assert!(
            err.to_string().contains("end-of-central-directory"),
            "got {err}"
        );
    }

    #[test]
    fn a_central_directory_header_with_a_bad_signature_is_refused() {
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        let central = central_start(&archive);
        patch_u32(&mut archive, central, 0);
        let err = import_kmz(&archive).expect_err("a broken central directory");
        assert!(
            err.to_string().contains("central directory is malformed"),
            "got {err}"
        );
    }

    #[test]
    fn a_local_file_header_with_a_bad_signature_is_refused() {
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        patch_u32(&mut archive, 0, 0);
        let err = import_kmz(&archive).expect_err("a broken local header");
        assert!(
            err.to_string().contains("local file header is malformed"),
            "got {err}"
        );
    }

    #[test]
    fn a_zip64_archive_is_refused() {
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        let eocd = eocd_start(&archive);
        patch_u16(&mut archive, eocd + 10, ZIP64_COUNT);
        let err = import_kmz(&archive).expect_err("a ZIP64 archive");
        assert!(err.to_string().contains("ZIP64"), "got {err}");
    }

    #[test]
    fn an_entry_with_an_unknown_compression_method_is_refused() {
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        let central = central_start(&archive);
        patch_u16(&mut archive, central + 10, 12);
        let err = import_kmz(&archive).expect_err("bzip2 is not a method this reader knows");
        assert!(
            err.to_string()
                .contains("compression method 12, which is not supported"),
            "got {err}"
        );
    }

    #[test]
    fn an_entry_that_declares_more_than_the_limit_is_refused() {
        let declared = 100 * 1024 * 1024;
        let archive = zip_with_encoded_entry("doc.kml", 0, 0, b"<kml/>", declared);
        let err = import_kmz(&archive).expect_err("a declared size past the limit");
        assert!(
            err.to_string()
                .contains("larger than the 67108864 byte limit"),
            "got {err}"
        );
    }

    #[test]
    fn an_encrypted_entry_is_refused() {
        let archive = zip_with_encoded_entry("doc.kml", 0, ZIP_FLAG_ENCRYPTED, b"<kml/>", 6);
        let err = import_kmz(&archive).expect_err("an encrypted entry");
        assert!(
            err.to_string()
                .contains("is encrypted, which is not supported"),
            "got {err}"
        );
    }

    #[test]
    fn a_local_header_that_says_encrypted_is_refused_even_when_the_central_directory_does_not() {
        // The two headers each carry the flag. A reader that trusts only the
        // central directory would try to inflate ciphertext.
        let mut archive = zip_with_one_entry("doc.kml", 0, b"<kml><Document/></kml>");
        patch_u16(&mut archive, 6, ZIP_FLAG_ENCRYPTED);
        let err = import_kmz(&archive).expect_err("the local header says encrypted");
        assert!(
            err.to_string()
                .contains("is encrypted, which is not supported"),
            "got {err}"
        );
    }

    #[test]
    fn a_doc_kml_that_decompresses_past_the_limit_is_refused_while_reading() {
        // The archive is small (a deflate stream of zeros is ~160 KiB), its
        // headers claim `uncompressed_size = 0`, and it really expands to
        // 160 MiB — the shape that made the old reader allocate everything
        // before checking. The reported length is the assertion that matters:
        // it must be the cap plus the one look-ahead byte, never the 160 MiB
        // the stream actually holds. Removing `Read::take` makes the message
        // say 167772160 and this test fails.
        use std::io::Write as _;

        let total = 160 * 1024 * 1024;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        let block = [0u8; 64 * 1024];
        let mut written = 0usize;
        while written < total {
            let chunk = (total - written).min(block.len());
            encoder
                .write_all(&block[..chunk])
                .expect("feed the deflate stream");
            written += chunk;
        }
        let stored = encoder.finish().expect("finish the deflate stream");
        assert!(
            stored.len() < 1_000_000,
            "the fixture should stay small, got {} bytes",
            stored.len()
        );
        let archive = zip_with_encoded_entry("doc.kml", 8, 0, &stored, 0);
        let err = import_kmz(&archive).expect_err("the reader must stop at the cap");
        let reported = u64::from(MAX_KMZ_DOCUMENT_BYTES) + 1;
        assert!(
            err.to_string()
                .contains(&format!("is {reported} bytes uncompressed")),
            "the reader did not stop at the cap + 1 look-ahead byte; got {err}"
        );
        assert!(
            !err.to_string().contains(&total.to_string()),
            "the reader measured the whole 160 MiB stream instead of the cap: {err}"
        );
    }

    /// Build a one-entry ZIP32 archive, stored (method 0) or deflated (method 8).
    ///
    /// The reader does not verify the CRC, so it is written as zero; the point
    /// of this helper is the deflate path, which the exported KMZ never takes.
    fn zip_with_one_entry(name: &str, method: u16, data: &[u8]) -> Vec<u8> {
        let stored = if method == 8 {
            deflate(data)
        } else {
            data.to_vec()
        };
        zip_with_encoded_entry(
            name,
            method,
            0,
            &stored,
            u32::try_from(data.len()).expect("entry fits ZIP32"),
        )
    }

    /// Deflate `data` into a raw (headerless) deflate stream.
    fn deflate(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;

        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).expect("deflate the entry");
        encoder.finish().expect("finish the deflate stream")
    }

    /// Build a one-entry ZIP32 archive from an already-encoded payload.
    ///
    /// Every header field that decides a refusal is explicit, so a test can
    /// write an archive the reader must turn away: a lying declared size, an
    /// encryption flag, a method the reader does not know.
    fn zip_with_encoded_entry(
        name: &str,
        method: u16,
        flags: u16,
        stored: &[u8],
        uncompressed: u32,
    ) -> Vec<u8> {
        let name = name.as_bytes();
        let size = u32::try_from(stored.len()).expect("entry fits ZIP32");
        let dos_time = 0u16;
        let dos_date = (1u16 << 5) | 1;
        let mut archive = Vec::new();

        // Local file header.
        archive.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        archive.extend_from_slice(&20u16.to_le_bytes());
        archive.extend_from_slice(&flags.to_le_bytes());
        archive.extend_from_slice(&method.to_le_bytes());
        archive.extend_from_slice(&dos_time.to_le_bytes());
        archive.extend_from_slice(&dos_date.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&uncompressed.to_le_bytes());
        archive.extend_from_slice(&(name.len() as u16).to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(name);
        archive.extend_from_slice(stored);

        // Central directory header.
        let directory_offset = u32::try_from(archive.len()).expect("archive fits ZIP32");
        archive.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        archive.extend_from_slice(&20u16.to_le_bytes());
        archive.extend_from_slice(&20u16.to_le_bytes());
        archive.extend_from_slice(&flags.to_le_bytes());
        archive.extend_from_slice(&method.to_le_bytes());
        archive.extend_from_slice(&dos_time.to_le_bytes());
        archive.extend_from_slice(&dos_date.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&uncompressed.to_le_bytes());
        archive.extend_from_slice(&(name.len() as u16).to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(name);

        // End of central directory.
        let directory_size =
            u32::try_from(archive.len()).expect("archive fits ZIP32") - directory_offset;
        archive.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&1u16.to_le_bytes());
        archive.extend_from_slice(&1u16.to_le_bytes());
        archive.extend_from_slice(&directory_size.to_le_bytes());
        archive.extend_from_slice(&directory_offset.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive
    }

    /// The offset of the end-of-central-directory record in a fixture.
    fn eocd_start(archive: &[u8]) -> usize {
        find_eocd(archive).expect("the fixture has an end-of-central-directory record")
    }

    /// The offset of the first central-directory header in a fixture.
    fn central_start(archive: &[u8]) -> usize {
        let eocd = eocd_start(archive);
        let offset = zip_u32(archive, eocd + 16).expect("the fixture records a directory offset");
        usize::try_from(offset).expect("the directory offset fits")
    }

    fn patch_u16(archive: &mut [u8], offset: usize, value: u16) {
        archive[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u32(archive: &mut [u8], offset: usize, value: u32) {
        archive[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
