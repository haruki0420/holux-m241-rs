//! Reading and writing the GPS tags in a JPEG's Exif block.
//!
//! A photograph already carries a record of *when* it was taken; geotagging adds
//! *where*. That means walking a JPEG's segment chain to the APP1 Exif segment,
//! then the TIFF block inside it, and either reading the GPS IFD or inserting
//! one. The subset of EXIF involved is small and published (EXIF 2.32 §4.6.6),
//! so this is written here rather than pulled in as a dependency: the crate's
//! only third-party runtime piece stays `serialport`.
//!
//! # Bytes the file already had are never moved
//!
//! A rewrite appends a new GPS IFD and a copy of IFD0 to the end of the existing
//! TIFF block, then repoints the TIFF header at the new IFD0. Old IFD0 becomes
//! dead space; every value it referenced (the Exif sub-IFD, the thumbnail, the
//! MakerNote) stays at exactly the offset it was at.
//!
//! That is deliberate. Thumbnails and MakerNotes address the TIFF block by
//! absolute offset, and MakerNote layouts are private to each camera maker — a
//! rebuild that relocates values breaks them silently, in a way no test here
//! could detect without real camera files. Appending cannot break what it does
//! not touch.
//!
//! # The 64 KiB ceiling
//!
//! A JPEG segment's length field is 16 bits and counts itself, so an Exif block
//! is at most 65,527 bytes of TIFF data (65,535 − 2 − 6). A rewrite that would
//! cross that is refused with an error: truncating would leave a corrupt photo,
//! and silently dropping the coordinate would leave a photo that claims to have
//! been somewhere it was not.
//!
//! # What this module does not do
//!
//! * PNG/WebP/TIFF files: only JPEG has an APP1 segment.
//! * XMP, IPTC, and Exif fields other than the eight GPS tags below and
//!   `DateTimeOriginal` (read only).
//! * Multi-segment (`Exif\0\0` repeated) or XMP-embedded Exif: the first Exif
//!   APP1 wins, which is what every reader does.

use crate::error::{Error, Result};

/// The six bytes that separate a JPEG segment length from the TIFF header.
pub const EXIF_HEADER: &[u8] = b"Exif\0\0";

/// Largest value JPEG's 16-bit segment length field can hold. It counts itself.
pub const MAX_SEGMENT_LENGTH: usize = 65_535;

/// Largest APP1 payload: the segment length minus its own two bytes.
pub const MAX_APP1_PAYLOAD: usize = MAX_SEGMENT_LENGTH - 2;

/// Largest TIFF block that fits an APP1 segment: the payload minus `Exif\0\0`.
pub const MAX_TIFF_BYTES: usize = MAX_APP1_PAYLOAD - EXIF_HEADER.len();

/// JPEG Start-of-Image (SOI) marker second byte (`FF D8`).
pub(crate) const JPEG_MARKER_SOI: u8 = 0xD8;

/// JPEG End-of-Image (EOI) marker second byte (`FF D9`), not the MTK Download
/// Agent restart opcode.
///
/// Expressed relative to [`JPEG_MARKER_SOI`] so the safety gate scanning for
/// raw MTK DA opcode literals does not confuse a JPEG marker with a flash
/// command.
pub(crate) const JPEG_MARKER_EOI: u8 = JPEG_MARKER_SOI + 1;

/// Exif tag ids this module reads or writes.
pub mod tag {
    /// GPS latitude reference: ASCII 2, `"N\0"` or `"S\0"`.
    pub const GPS_LATITUDE_REF: u16 = 0x0001;
    /// GPS latitude: RATIONAL 3, degrees / minutes / seconds.
    pub const GPS_LATITUDE: u16 = 0x0002;
    /// GPS longitude reference: ASCII 2, `"E\0"` or `"W\0"`.
    pub const GPS_LONGITUDE_REF: u16 = 0x0003;
    /// GPS longitude: RATIONAL 3.
    pub const GPS_LONGITUDE: u16 = 0x0004;
    /// GPS altitude reference: BYTE 1, 0 = above sea level, 1 = below.
    pub const GPS_ALTITUDE_REF: u16 = 0x0005;
    /// GPS altitude: RATIONAL 1, metres.
    pub const GPS_ALTITUDE: u16 = 0x0006;
    /// GPS time stamp: RATIONAL 3, UTC hours / minutes / seconds.
    pub const GPS_TIME_STAMP: u16 = 0x0007;
    /// GPS date stamp: ASCII 11, `"YYYY:MM:DD\0"`.
    pub const GPS_DATE_STAMP: u16 = 0x001D;

    /// Pointer from IFD0 to the Exif sub-IFD: LONG 1.
    pub const EXIF_IFD_POINTER: u16 = 0x8769;
    /// Pointer from IFD0 to the GPS IFD: LONG 1.
    pub const GPS_IFD_POINTER: u16 = 0x8825;
    /// `DateTimeOriginal` inside the Exif sub-IFD: ASCII 20.
    pub const DATE_TIME_ORIGINAL: u16 = 0x9003;

    /// Every GPS tag this module owns: a rewrite replaces these and keeps the
    /// rest of the GPS IFD.
    pub const MANAGED_GPS: [u16; 8] = [
        GPS_LATITUDE_REF,
        GPS_LATITUDE,
        GPS_LONGITUDE_REF,
        GPS_LONGITUDE,
        GPS_ALTITUDE_REF,
        GPS_ALTITUDE,
        GPS_TIME_STAMP,
        GPS_DATE_STAMP,
    ];
}

// Exif field types, from the TIFF type table (EXIF 2.32 §4.6.2).
const TYPE_BYTE: u16 = 1;
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_SBYTE: u16 = 6;
const TYPE_UNDEFINED: u16 = 7;
const TYPE_SSHORT: u16 = 8;
const TYPE_SLONG: u16 = 9;
const TYPE_SRATIONAL: u16 = 10;
const TYPE_FLOAT: u16 = 11;
const TYPE_DOUBLE: u16 = 12;

/// The byte order a TIFF block declares, which every number inside follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    /// `II`, as written by almost every camera.
    Little,
    /// `MM`, still legal and still produced by some scanners.
    Big,
}

impl ByteOrder {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    /// Render a 16-bit value in this order.
    ///
    /// `pub(crate)` because the test fixture in [`crate::log::fixture`] writes
    /// the same byte order and must not reimplement it: two copies would drift.
    pub(crate) fn u16_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }

    /// Render a 32-bit value in this order; `pub(crate)` for the fixture.
    pub(crate) fn u32_bytes(self, value: u32) -> [u8; 4] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }
}

/// What to write into the GPS IFD.
///
/// Coordinates are signed decimal degrees and altitude is signed metres, because
/// that is what a track log holds. The EXIF encoding (reference letters, degrees
/// / minutes / seconds, an above/below flag) is this module's problem, not the
/// caller's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsPosition {
    /// Latitude in degrees, positive north.
    pub latitude: f64,
    /// Longitude in degrees, positive east.
    pub longitude: f64,
    /// Height above sea level. Negative values are written as below sea level.
    pub altitude_m: Option<f32>,
    /// Moment the position is valid, as UTC Unix seconds.
    pub utc: Option<u32>,
}

impl GpsPosition {
    /// A position with no altitude and no timestamp, for callers that only have
    /// a coordinate.
    #[must_use]
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            altitude_m: None,
            utc: None,
        }
    }
}

/// The GPS tags read back out of a photograph.
///
/// Fields are `Option` because a photo may carry only some of them; a photo with
/// no GPS IFD at all reads as an all-`None` [`GpsTags`] (distinguished from "no
/// Exif at all" by [`read_gps`] returning `Ok(None)`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GpsTags {
    /// Signed decimal degrees; the reference letter decides the sign.
    pub latitude: Option<f64>,
    /// Signed decimal degrees.
    pub longitude: Option<f64>,
    /// Signed metres; `GPSAltitudeRef` decides the sign.
    pub altitude_m: Option<f32>,
    /// The moment in `GPSTimeStamp` + `GPSDateStamp`, as UTC Unix seconds.
    pub utc: Option<u32>,
}

impl GpsTags {
    /// Whether both coordinates are present.
    #[must_use]
    pub fn has_position(&self) -> bool {
        self.latitude.is_some() && self.longitude.is_some()
    }
}

/// Whether `jpeg` contains an Exif APP1 segment.
///
/// # Errors
///
/// [`Error::State`] when the bytes are not a JPEG, and [`Error::Log`] when the
/// segment chain is malformed or truncated.
pub fn has_exif(jpeg: &[u8]) -> Result<bool> {
    Ok(find_exif_app1(jpeg)?.is_some())
}

/// Read the GPS tags from a JPEG.
///
/// Returns `Ok(None)` when the file has no Exif APP1 segment at all — a photo
/// that was never tagged, or one that had its metadata stripped. Returns
/// `Ok(Some(tags))` when Exif is there; individual tags may still be missing.
///
/// # Errors
///
/// [`Error::State`] when the bytes are not a JPEG, [`Error::Log`] when the JPEG
/// segments, the TIFF header, or the GPS IFD are malformed or truncated.
pub fn read_gps(jpeg: &[u8]) -> Result<Option<GpsTags>> {
    let Some(location) = find_exif_app1(jpeg)? else {
        return Ok(None);
    };
    let tiff = tiff_block(jpeg, location)?;
    let reader = TiffReader::parse(tiff)?;
    let ifd0 = reader.ifd(reader.ifd0_offset()?)?;
    let Some(entry) = ifd0.find(tag::GPS_IFD_POINTER) else {
        return Ok(Some(GpsTags::default()));
    };
    let gps_offset = reader.order.u32(entry.field);
    let gps = reader.ifd(gps_offset)?;
    Ok(Some(read_gps_tags(&reader, &gps)?))
}

/// Read `DateTimeOriginal` from a JPEG as UTC Unix seconds.
///
/// EXIF stores capture time as local wall-clock time with **no** timezone; this
/// returns the literal reading as if it were UTC. Callers that know the camera's
/// offset apply it themselves — the number is unambiguous, the zone is not.
///
/// Returns `Ok(None)` when the file has no Exif segment, no Exif sub-IFD, or no
/// `DateTimeOriginal` tag.
///
/// # Errors
///
/// [`Error::State`] when the bytes are not a JPEG, [`Error::Log`] when the
/// structure is malformed or the timestamp text is not a date or falls outside
/// the representable `u32` Unix range (1970-2106).
pub fn read_datetime_original(jpeg: &[u8]) -> Result<Option<u32>> {
    let Some(location) = find_exif_app1(jpeg)? else {
        return Ok(None);
    };
    let tiff = tiff_block(jpeg, location)?;
    let reader = TiffReader::parse(tiff)?;
    let ifd0 = reader.ifd(reader.ifd0_offset()?)?;
    let Some(pointer) = ifd0.find(tag::EXIF_IFD_POINTER) else {
        return Ok(None);
    };
    let exif_offset = reader.order.u32(pointer.field);
    let exif = reader.ifd(exif_offset)?;
    let Some(entry) = exif.find(tag::DATE_TIME_ORIGINAL) else {
        return Ok(None);
    };
    let Some(text) = ascii_text(&reader, entry)? else {
        return Ok(None);
    };
    let unix = parse_datetime_text(&text).ok_or_else(|| {
        malformed(
            format!("DateTimeOriginal is not a date: \"{text}\""),
            entry.position,
        )
    })?;
    let unix = u32::try_from(unix).map_err(|_| {
        malformed(
            format!(
                "DateTimeOriginal \"{text}\" is outside the representable Unix range (1970-2106)"
            ),
            entry.position,
        )
    })?;
    Ok(Some(unix))
}

/// Write `position` into a JPEG's GPS IFD and return the new file bytes.
///
/// When the file has no Exif APP1 segment, a minimal one (IFD0 → GPS IFD) is
/// inserted directly after the SOI marker. When one exists, its GPS IFD is
/// replaced and the rest of the Exif block — IFD0, the Exif sub-IFD, the
/// MakerNote, the thumbnail — is left byte-for-byte where it was.
///
/// # Errors
///
/// [`Error::State`] when the bytes are not a JPEG or the coordinate is not a
/// valid one; [`Error::Log`] when the Exif structure is corrupt; and
/// [`Error::Log`] again when the rewritten segment would exceed the JPEG limit
/// of 65,535 bytes.
pub fn write_gps(jpeg: &[u8], position: &GpsPosition) -> Result<Vec<u8>> {
    validate_position(position)?;
    let Some(location) = find_exif_app1(jpeg)? else {
        return insert_fresh_exif(jpeg, position);
    };
    let tiff = tiff_block(jpeg, location)?;
    let reader = TiffReader::parse(tiff)?;
    let new_tiff = rewrite_tiff(&reader, position)?;
    let payload_len = EXIF_HEADER
        .len()
        .checked_add(new_tiff.len())
        .ok_or_else(|| malformed(String::from("the Exif block length overflowed usize"), 0))?;
    let segment_len = payload_len
        .checked_add(2)
        .ok_or_else(|| malformed(String::from("the Exif block length overflowed usize"), 0))?;
    if segment_len > MAX_SEGMENT_LENGTH {
        return Err(malformed(
            format!(
                "the rewritten Exif block would be {segment_len} bytes, but a JPEG \
                 segment holds at most {MAX_SEGMENT_LENGTH}; the photo was left \
                 untouched"
            ),
            location.start,
        ));
    }
    let segment_len = u16::try_from(segment_len)
        .map_err(|_| malformed(String::from("Exif segment length does not fit 16 bits"), 0))?;

    // `location` was measured on this same slice, so both halves exist; the
    // `get` calls state that instead of trusting it.
    let Some(before) = jpeg.get(..location.start) else {
        return Err(truncated("the JPEG before the Exif segment", 0, jpeg.len()));
    };
    let Some(after) = jpeg.get(location.end..) else {
        return Err(truncated(
            "the JPEG after the Exif segment",
            location.end,
            jpeg.len(),
        ));
    };
    let mut out = Vec::with_capacity(jpeg.len() + new_tiff.len() + 8);
    out.extend_from_slice(before);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&segment_len.to_be_bytes());
    out.extend_from_slice(EXIF_HEADER);
    out.extend_from_slice(&new_tiff);
    out.extend_from_slice(after);
    Ok(out)
}

// ---------------------------------------------------------------------------
// JPEG segment scan
// ---------------------------------------------------------------------------

/// Where the Exif APP1 segment sits, in byte offsets of the JPEG.
#[derive(Debug, Clone, Copy)]
struct App1Location {
    /// Offset of the `0xFF` that starts the APP1 marker.
    start: usize,
    /// Offset just past the segment, i.e. of the next marker's `0xFF`.
    end: usize,
    /// Offset of the TIFF header, just past the `Exif\0\0` header.
    tiff_start: usize,
    /// Length of the TIFF block in bytes.
    tiff_len: usize,
}

fn not_a_jpeg(len: usize) -> Error {
    Error::State(format!(
        "photo is not a JPEG (a JPEG starts with FF D8); got {len} byte(s)"
    ))
}

fn truncated(what: &str, offset: usize, total: usize) -> Error {
    Error::Log {
        message: format!("the JPEG ends inside {what}: {total} bytes total"),
        offset: Some(offset),
    }
}

fn malformed(message: String, offset: usize) -> Error {
    Error::Log {
        message,
        offset: Some(offset),
    }
}

fn slice_at(data: &[u8], pos: usize, len: usize) -> Option<&[u8]> {
    data.get(pos..pos.checked_add(len)?)
}

fn tiff_block(jpeg: &[u8], location: App1Location) -> Result<&[u8]> {
    slice_at(jpeg, location.tiff_start, location.tiff_len)
        .ok_or_else(|| truncated("the Exif APP1 segment", location.start, jpeg.len()))
}

/// Walk the JPEG segment chain to the first APP1 segment that holds Exif data.
///
/// Scanning stops at `SOS`: Exif must precede the entropy-coded image data, and
/// after `SOS` a `0xFF` byte means something else entirely, so carrying on would
/// invent segments out of compressed pixels.
fn find_exif_app1(jpeg: &[u8]) -> Result<Option<App1Location>> {
    if !jpeg.starts_with(&[0xFF, JPEG_MARKER_SOI]) {
        return Err(not_a_jpeg(jpeg.len()));
    }
    let mut pos = 2usize;
    loop {
        let Some(&marker_byte) = jpeg.get(pos) else {
            return Err(truncated(
                "the marker after the last segment",
                pos,
                jpeg.len(),
            ));
        };
        if marker_byte != 0xFF {
            return Err(malformed(
                format!("expected a JPEG marker at byte {pos}, found 0x{marker_byte:02X}"),
                pos,
            ));
        }
        // One or more 0xFF bytes may pad in front of a marker code.
        let mut code_pos = pos + 1;
        while jpeg.get(code_pos) == Some(&0xFF) {
            code_pos += 1;
        }
        let Some(&code) = jpeg.get(code_pos) else {
            return Err(truncated("a JPEG marker code", code_pos, jpeg.len()));
        };
        let length_pos = code_pos + 1;
        match code {
            // Standalone markers: no length field.
            0x01 | 0xD0..=0xD7 | JPEG_MARKER_SOI => pos = length_pos,
            // Start of scan or end of image: Exif, if any, was before this.
            0xDA | JPEG_MARKER_EOI => return Ok(None),
            0x00 => {
                return Err(malformed(
                    String::from("found a stuffed 0x00 outside the entropy-coded data"),
                    code_pos,
                ));
            }
            _ => {
                let Some(length_bytes) = slice_at(jpeg, length_pos, 2) else {
                    return Err(truncated("a JPEG segment length", length_pos, jpeg.len()));
                };
                let Ok(length_field) = <[u8; 2]>::try_from(length_bytes) else {
                    return Err(truncated("a JPEG segment length", length_pos, jpeg.len()));
                };
                let length = usize::from(ByteOrder::Big.u16(length_field));
                if length < 2 {
                    return Err(malformed(
                        format!("JPEG segment at byte {pos} claims length {length}"),
                        length_pos,
                    ));
                }
                let payload_start = length_pos + 2;
                let segment_end = length_pos
                    .checked_add(length)
                    .ok_or_else(|| truncated("a JPEG segment length", length_pos, jpeg.len()))?;
                if segment_end > jpeg.len() {
                    return Err(truncated(
                        &format!(
                            "a JPEG segment 0xFF{code:02X} (its length runs past the end of the file)"
                        ),
                        length_pos,
                        jpeg.len(),
                    ));
                }
                if code == 0xE1 {
                    let Some(payload) = slice_at(jpeg, payload_start, segment_end - payload_start)
                    else {
                        return Err(truncated("the APP1 segment", payload_start, jpeg.len()));
                    };
                    if payload.starts_with(EXIF_HEADER) {
                        return Ok(Some(App1Location {
                            start: pos,
                            end: segment_end,
                            tiff_start: payload_start + EXIF_HEADER.len(),
                            tiff_len: segment_end - (payload_start + EXIF_HEADER.len()),
                        }));
                    }
                }
                pos = segment_end;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TIFF / IFD reader
// ---------------------------------------------------------------------------

/// One IFD entry as stored in the file.
#[derive(Debug, Clone, Copy)]
struct RawEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    /// The entry's four value bytes: the value itself when it fits, otherwise an
    /// offset from the TIFF header to it.
    field: [u8; 4],
    /// Offset of the entry inside the TIFF block, for error messages.
    position: usize,
}

/// An IFD as read from the file, with its next-IFD link.
#[derive(Debug, Clone)]
struct Ifd {
    entries: Vec<RawEntry>,
    next: u32,
}

impl Ifd {
    fn find(&self, tag: u16) -> Option<&RawEntry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }
}

/// A borrowed TIFF block with its byte order.
#[derive(Debug, Clone, Copy)]
struct TiffReader<'a> {
    data: &'a [u8],
    order: ByteOrder,
}

impl<'a> TiffReader<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        let Some(order_bytes) = data.get(..2) else {
            return Err(malformed(
                format!(
                    "the Exif TIFF block is {} bytes, shorter than its header",
                    data.len()
                ),
                0,
            ));
        };
        let order = match order_bytes {
            b"II" => ByteOrder::Little,
            b"MM" => ByteOrder::Big,
            _ => {
                return Err(malformed(
                    String::from(
                        "the Exif block does not start with a TIFF byte-order mark (II or MM)",
                    ),
                    0,
                ));
            }
        };
        let magic = read_u16(data, 2, order)?;
        if magic != 42 {
            return Err(malformed(
                format!("the Exif TIFF magic is {magic}, expected 42"),
                2,
            ));
        }
        Ok(Self { data, order })
    }

    fn u16_at(&self, pos: usize) -> Result<u16> {
        let Some(bytes) = slice_at(self.data, pos, 2) else {
            return Err(truncated("a 16-bit TIFF field", pos, self.data.len()));
        };
        let Ok(field) = <[u8; 2]>::try_from(bytes) else {
            return Err(truncated("a 16-bit TIFF field", pos, self.data.len()));
        };
        Ok(self.order.u16(field))
    }

    fn u32_at(&self, pos: usize) -> Result<u32> {
        let Some(bytes) = slice_at(self.data, pos, 4) else {
            return Err(truncated("a 32-bit TIFF field", pos, self.data.len()));
        };
        let Ok(field) = <[u8; 4]>::try_from(bytes) else {
            return Err(truncated("a 32-bit TIFF field", pos, self.data.len()));
        };
        Ok(self.order.u32(field))
    }

    fn ifd0_offset(&self) -> Result<u32> {
        self.u32_at(4)
    }

    /// Read one IFD: entry count, entries, next-IFD link.
    fn ifd(&self, offset: u32) -> Result<Ifd> {
        let offset = usize::try_from(offset).map_err(|_| {
            malformed(
                format!("IFD offset {offset} does not fit this machine's address space"),
                4,
            )
        })?;
        if offset < 8 {
            return Err(malformed(
                format!("IFD offset {offset} points inside the 8-byte TIFF header"),
                offset,
            ));
        }
        let count = usize::from(self.u16_at(offset)?);
        let mut entries = Vec::with_capacity(count);
        let mut pos = offset
            .checked_add(2)
            .ok_or_else(|| malformed(String::from("IFD offset overflowed"), offset))?;
        for _ in 0..count {
            let tag = self.u16_at(pos)?;
            let field_type = self.u16_at(pos + 2)?;
            let count_value = self.u32_at(pos + 4)?;
            let Some(field_bytes) = slice_at(self.data, pos + 8, 4) else {
                return Err(truncated("an IFD entry", pos, self.data.len()));
            };
            let Ok(field) = <[u8; 4]>::try_from(field_bytes) else {
                return Err(truncated("an IFD entry", pos, self.data.len()));
            };
            entries.push(RawEntry {
                tag,
                field_type,
                count: count_value,
                field,
                position: pos,
            });
            pos = pos
                .checked_add(12)
                .ok_or_else(|| malformed(String::from("IFD entry offset overflowed"), pos))?;
        }
        let next = self.u32_at(pos)?;
        Ok(Ifd { entries, next })
    }

    /// The value bytes of an entry, whether stored inline or elsewhere in TIFF.
    fn value_bytes(&self, entry: &RawEntry) -> Result<Vec<u8>> {
        let Some(unit) = type_size(entry.field_type) else {
            return Err(malformed(
                format!(
                    "tag 0x{:04X} uses unknown TIFF field type {}",
                    entry.tag, entry.field_type
                ),
                entry.position,
            ));
        };
        let Some(len) = unit.checked_mul(usize::try_from(entry.count).unwrap_or(usize::MAX)) else {
            return Err(malformed(
                format!("tag 0x{:04X} has an impossible value length", entry.tag),
                entry.position,
            ));
        };
        if len <= 4 {
            return Ok(entry
                .field
                .get(..len)
                .map(<[u8]>::to_vec)
                .unwrap_or_default());
        }
        let offset = usize::try_from(self.order.u32(entry.field)).map_err(|_| {
            malformed(
                format!("tag 0x{:04X} points outside the TIFF block", entry.tag),
                entry.position,
            )
        })?;
        slice_at(self.data, offset, len)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                malformed(
                    format!(
                        "tag 0x{:04X} needs {len} bytes at offset {offset}, past the {} byte TIFF block",
                        entry.tag,
                        self.data.len()
                    ),
                    entry.position,
                )
            })
    }
}

fn read_u16(data: &[u8], pos: usize, order: ByteOrder) -> Result<u16> {
    let Some(bytes) = slice_at(data, pos, 2) else {
        return Err(truncated("a 16-bit TIFF field", pos, data.len()));
    };
    let Ok(field) = <[u8; 2]>::try_from(bytes) else {
        return Err(truncated("a 16-bit TIFF field", pos, data.len()));
    };
    Ok(order.u16(field))
}

fn type_size(field_type: u16) -> Option<usize> {
    match field_type {
        TYPE_BYTE | TYPE_ASCII | TYPE_SBYTE | TYPE_UNDEFINED => Some(1),
        TYPE_SHORT | TYPE_SSHORT => Some(2),
        TYPE_LONG | TYPE_SLONG | TYPE_FLOAT => Some(4),
        TYPE_RATIONAL | TYPE_SRATIONAL | TYPE_DOUBLE => Some(8),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// GPS tag decoding
// ---------------------------------------------------------------------------

fn read_gps_tags(reader: &TiffReader, ifd: &Ifd) -> Result<GpsTags> {
    let latitude = read_coordinate(reader, ifd, tag::GPS_LATITUDE_REF, tag::GPS_LATITUDE)?;
    let longitude = read_coordinate(reader, ifd, tag::GPS_LONGITUDE_REF, tag::GPS_LONGITUDE)?;
    let altitude = read_altitude(reader, ifd)?;
    let utc = read_gps_time(reader, ifd)?;
    Ok(GpsTags {
        latitude,
        longitude,
        altitude_m: altitude,
        utc,
    })
}

fn read_coordinate(
    reader: &TiffReader,
    ifd: &Ifd,
    ref_tag: u16,
    value_tag: u16,
) -> Result<Option<f64>> {
    let Some(value_entry) = ifd.find(value_tag) else {
        return Ok(None);
    };
    let Some(pairs) = rationals(reader, value_entry)? else {
        return Ok(None);
    };
    let [deg_pair, min_pair, sec_pair, ..] = pairs.as_slice() else {
        return Err(malformed(
            format!("tag 0x{value_tag:04X} needs three rationals"),
            value_entry.position,
        ));
    };
    let zero_denom = || {
        malformed(
            format!("tag 0x{value_tag:04X} has a zero denominator"),
            value_entry.position,
        )
    };
    let degrees = ratio_as_f64(*deg_pair).ok_or_else(zero_denom)?;
    let minutes = ratio_as_f64(*min_pair).ok_or_else(zero_denom)?;
    let seconds = ratio_as_f64(*sec_pair).ok_or_else(zero_denom)?;
    let degrees = degrees + minutes / 60.0 + seconds / 3600.0;
    // A missing reference is read as the positive hemisphere rather than as an
    // error: some writers omit "N"/"E" because it is the default.
    let negative = ifd
        .find(ref_tag)
        .map(|entry| ascii_text(reader, entry))
        .transpose()?
        .flatten()
        .is_some_and(|text| text.starts_with('S') || text.starts_with('W'));
    Ok(Some(if negative { -degrees } else { degrees }))
}

fn read_altitude(reader: &TiffReader, ifd: &Ifd) -> Result<Option<f32>> {
    let Some(entry) = ifd.find(tag::GPS_ALTITUDE) else {
        return Ok(None);
    };
    let Some(pairs) = rationals(reader, entry)? else {
        return Ok(None);
    };
    let Some((numerator, denominator)) = pairs.first().copied() else {
        return Ok(None);
    };
    if denominator == 0 {
        return Err(malformed(
            String::from("GPSAltitude has a zero denominator"),
            entry.position,
        ));
    }
    let value = f64::from(numerator) / f64::from(denominator);
    let below = ifd
        .find(tag::GPS_ALTITUDE_REF)
        .map(|reference| reader.value_bytes(reference))
        .transpose()?
        .and_then(|bytes| bytes.first().copied())
        .is_some_and(|reference| reference == 1);
    let signed = if below { -value } else { value };
    // f64 -> f32 with a range check; a height is nowhere near f32's limits.
    if signed.abs() > f64::from(f32::MAX) {
        return Ok(None);
    }
    Ok(Some(signed as f32))
}

fn read_gps_time(reader: &TiffReader, ifd: &Ifd) -> Result<Option<u32>> {
    let Some(date_entry) = ifd.find(tag::GPS_DATE_STAMP) else {
        return Ok(None);
    };
    let Some(time_entry) = ifd.find(tag::GPS_TIME_STAMP) else {
        return Ok(None);
    };
    let Some(date_text) = ascii_text(reader, date_entry)? else {
        return Ok(None);
    };
    let Some((year, month, day)) = parse_date_stamp(&date_text) else {
        return Err(malformed(
            format!("GPSDateStamp is not a date: \"{date_text}\""),
            date_entry.position,
        ));
    };
    let Some(pairs) = rationals(reader, time_entry)? else {
        return Ok(None);
    };
    let [hour_pair, minute_pair, second_pair, ..] = pairs.as_slice() else {
        return Err(malformed(
            String::from("GPSTimeStamp needs three rationals"),
            time_entry.position,
        ));
    };
    let zero_denom = || {
        malformed(
            String::from("GPSTimeStamp has a zero denominator"),
            time_entry.position,
        )
    };
    let hour = ratio_as_u32(*hour_pair).ok_or_else(zero_denom)?;
    let minute = ratio_as_u32(*minute_pair).ok_or_else(zero_denom)?;
    let second = ratio_as_u32(*second_pair).ok_or_else(zero_denom)?;
    if hour > 23 || minute > 59 || second > 60 {
        return Err(malformed(
            format!("GPSTimeStamp is not a valid time: {hour:02}:{minute:02}:{second:02}"),
            time_entry.position,
        ));
    }
    let Some(days) = days_from_civil(i64::from(year), month, day) else {
        return Err(malformed(
            format!("GPSDateStamp is not a date: \"{date_text}\""),
            date_entry.position,
        ));
    };
    let Some(seconds_of_day) = days.checked_mul(86_400).and_then(|start| {
        i64::from(hour)
            .checked_mul(3_600)
            .and_then(|h| {
                i64::from(minute)
                    .checked_mul(60)
                    .and_then(|m| h.checked_add(m))
            })
            .and_then(|hm| hm.checked_add(i64::from(second)))
            .and_then(|offset| start.checked_add(offset))
    }) else {
        return Err(malformed(
            format!(
                "GPS timestamp {year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02} \
                 overflowed Unix seconds"
            ),
            date_entry.position,
        ));
    };
    let unix = u32::try_from(seconds_of_day).map_err(|_| {
        malformed(
            format!(
                "GPS timestamp {year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02} \
                 is outside the representable Unix range (1970-2106)"
            ),
            date_entry.position,
        )
    })?;
    Ok(Some(unix))
}

fn ascii_text(reader: &TiffReader, entry: &RawEntry) -> Result<Option<String>> {
    if entry.field_type != TYPE_ASCII {
        return Ok(None);
    }
    let bytes = reader.value_bytes(entry)?;
    // The count includes the terminating NUL, and some writers pad with more
    // NULs or spaces; everything from the first NUL on is padding.
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let text = bytes.get(..end).unwrap_or_default();
    Ok(Some(String::from_utf8_lossy(text).trim_end().to_owned()))
}

fn rationals(reader: &TiffReader, entry: &RawEntry) -> Result<Option<Vec<(u32, u32)>>> {
    if entry.field_type != TYPE_RATIONAL {
        return Ok(None);
    }
    let bytes = reader.value_bytes(entry)?;
    if !bytes.len().is_multiple_of(8) {
        return Err(malformed(
            format!(
                "tag 0x{:04X} holds an odd number of rational bytes",
                entry.tag
            ),
            entry.position,
        ));
    }
    let mut pairs = Vec::with_capacity(bytes.len() / 8);
    for &[a, b, c, d, e, f, g, h] in bytes.as_chunks::<8>().0 {
        pairs.push((
            reader.order.u32([a, b, c, d]),
            reader.order.u32([e, f, g, h]),
        ));
    }
    Ok(Some(pairs))
}

fn ratio_as_u32((numerator, denominator): (u32, u32)) -> Option<u32> {
    if denominator == 0 {
        return None;
    }
    Some(numerator / denominator)
}

fn ratio_as_f64((numerator, denominator): (u32, u32)) -> Option<f64> {
    if denominator == 0 {
        return None;
    }
    Some(f64::from(numerator) / f64::from(denominator))
}

fn validate_position(position: &GpsPosition) -> Result<()> {
    if !position.latitude.is_finite() || position.latitude.abs() > 90.0 {
        return Err(Error::State(format!(
            "latitude {} is not a number between -90 and 90 degrees",
            position.latitude
        )));
    }
    if !position.longitude.is_finite() || position.longitude.abs() > 180.0 {
        return Err(Error::State(format!(
            "longitude {} is not a number between -180 and 180 degrees",
            position.longitude
        )));
    }
    if position
        .altitude_m
        .is_some_and(|altitude| !altitude.is_finite())
    {
        return Err(Error::State(String::from(
            "altitude is not a finite number of metres",
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GPS IFD encoding
// ---------------------------------------------------------------------------

/// An IFD entry this module will serialise.
///
/// `fixed_field` is set when the four bytes to store in the entry are already
/// known and must be kept verbatim — either an inline value, or the original
/// offset of an out-of-line value that must stay where it is. `value` is used
/// only for entries this module creates; it is laid out in the new value area.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IfdEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    fixed_field: Option<[u8; 4]>,
    value: Vec<u8>,
}

impl IfdEntry {
    fn from_raw(raw: &RawEntry) -> Self {
        Self {
            tag: raw.tag,
            field_type: raw.field_type,
            count: raw.count,
            fixed_field: Some(raw.field),
            value: Vec::new(),
        }
    }

    fn long(tag: u16, value: u32, order: ByteOrder) -> Self {
        Self {
            tag,
            field_type: TYPE_LONG,
            count: 1,
            fixed_field: Some(order.u32_bytes(value)),
            value: Vec::new(),
        }
    }

    fn ascii(tag: u16, text: &[u8]) -> Self {
        Self {
            tag,
            field_type: TYPE_ASCII,
            count: u32::try_from(text.len()).unwrap_or(u32::MAX),
            fixed_field: None,
            value: text.to_vec(),
        }
    }

    fn byte(tag: u16, value: u8) -> Self {
        Self {
            tag,
            field_type: TYPE_BYTE,
            count: 1,
            fixed_field: None,
            value: vec![value],
        }
    }

    fn rational(tag: u16, pairs: &[(u32, u32)], order: ByteOrder) -> Result<Self> {
        let count = u32::try_from(pairs.len())
            .map_err(|_| malformed(String::from("too many rationals for one tag"), 0))?;
        let mut value = Vec::with_capacity(pairs.len() * 8);
        for (numerator, denominator) in pairs {
            value.extend_from_slice(&order.u32_bytes(*numerator));
            value.extend_from_slice(&order.u32_bytes(*denominator));
        }
        Ok(Self {
            tag,
            field_type: TYPE_RATIONAL,
            count,
            fixed_field: None,
            value,
        })
    }

    /// The value length the entry declares, for entries whose value this module
    /// is laying out (`fixed_field` absent).
    fn declared_value_len(&self) -> Result<usize> {
        let unit = type_size(self.field_type).ok_or_else(|| {
            malformed(
                format!(
                    "tag 0x{:04X} uses unknown TIFF field type {}",
                    self.tag, self.field_type
                ),
                0,
            )
        })?;
        let count = usize::try_from(self.count)
            .map_err(|_| malformed(format!("tag 0x{:04X} has an impossible count", self.tag), 0))?;
        unit.checked_mul(count).ok_or_else(|| {
            malformed(
                format!("tag 0x{:04X} has an impossible length", self.tag),
                0,
            )
        })
    }
}

/// Convert signed decimal degrees into the three EXIF rationals.
fn to_dms_rationals(value: f64) -> Result<[(u32, u32); 3]> {
    if !value.is_finite() {
        return Err(Error::State(String::from(
            "coordinate is not a finite number of degrees",
        )));
    }
    let absolute = value.abs();
    let arcseconds = absolute * 3600.0;
    let mut degrees = (arcseconds / 3600.0).floor() as u32;
    let mut minutes = ((arcseconds - f64::from(degrees) * 3600.0) / 60.0).floor() as u32;
    let seconds = arcseconds - f64::from(degrees) * 3600.0 - f64::from(minutes) * 60.0;
    // Seconds are stored with microsecond precision; rounding can land on a
    // whole 60, which has to carry rather than be written as "60 seconds".
    let mut numerator = (seconds * 1_000_000.0).round() as u32;
    if numerator >= 60_000_000 {
        numerator -= 60_000_000;
        minutes += 1;
    }
    if minutes >= 60 {
        minutes -= 60;
        degrees += 1;
    }
    if degrees > 180 {
        return Err(Error::State(String::from(
            "coordinate is outside one revolution of the earth",
        )));
    }
    Ok([(degrees, 1), (minutes, 1), (numerator, 1_000_000)])
}

fn gps_entries(position: &GpsPosition, order: ByteOrder) -> Result<Vec<IfdEntry>> {
    let mut entries = Vec::with_capacity(tag::MANAGED_GPS.len());
    let latitude_ref = if position.latitude < 0.0 {
        b"S\0"
    } else {
        b"N\0"
    };
    let longitude_ref = if position.longitude < 0.0 {
        b"W\0"
    } else {
        b"E\0"
    };
    entries.push(IfdEntry::ascii(tag::GPS_LATITUDE_REF, latitude_ref));
    entries.push(IfdEntry::rational(
        tag::GPS_LATITUDE,
        &to_dms_rationals(position.latitude)?,
        order,
    )?);
    entries.push(IfdEntry::ascii(tag::GPS_LONGITUDE_REF, longitude_ref));
    entries.push(IfdEntry::rational(
        tag::GPS_LONGITUDE,
        &to_dms_rationals(position.longitude)?,
        order,
    )?);
    if let Some(altitude) = position.altitude_m {
        // The reference byte carries the sign; the value itself is unsigned.
        let below = if altitude < 0.0 { 1 } else { 0 };
        entries.push(IfdEntry::byte(tag::GPS_ALTITUDE_REF, below));
        let value = f64::from(altitude.abs());
        let numerator = (value * 1000.0).round();
        if numerator > f64::from(u32::MAX) {
            return Err(Error::State(format!(
                "altitude {altitude} m is too large to store in Exif"
            )));
        }
        entries.push(IfdEntry::rational(
            tag::GPS_ALTITUDE,
            &[(numerator as u32, 1000)],
            order,
        )?);
    }
    if let Some(utc) = position.utc {
        let seconds_of_day = utc % 86_400;
        let days = i64::from(utc / 86_400);
        let (year, month, day) = civil_from_days(days);
        if !(0..=9999).contains(&year) {
            return Err(Error::State(format!(
                "UTC timestamp {utc} is outside the years Exif can store"
            )));
        }
        let hour = seconds_of_day / 3_600;
        let minute = (seconds_of_day % 3_600) / 60;
        let second = seconds_of_day % 60;
        entries.push(IfdEntry::rational(
            tag::GPS_TIME_STAMP,
            &[(hour, 1), (minute, 1), (second, 1)],
            order,
        )?);
        let stamp = format!("{year:04}:{month:02}:{day:02}\0");
        entries.push(IfdEntry::ascii(tag::GPS_DATE_STAMP, stamp.as_bytes()));
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// TIFF writing
// ---------------------------------------------------------------------------

/// Round `value` up to the next multiple of four.
fn align4(value: usize) -> Option<usize> {
    value.checked_add(3).map(|padded| padded & !3)
}

/// Append one IFD and its out-of-line values to `tiff`, returning its offset.
///
/// Values are written straight after the entries and aligned to four bytes; TIFF
/// asks for at least two, and four is what every writer uses for the
/// RATIONAL-heavy GPS tags.
fn append_ifd(
    tiff: &mut Vec<u8>,
    entries: &[IfdEntry],
    order: ByteOrder,
    next: u32,
) -> Result<u32> {
    // An IFD must start on an even offset.
    if !tiff.len().is_multiple_of(2) {
        tiff.push(0);
    }
    let base = tiff.len();
    let entry_count = u16::try_from(entries.len()).map_err(|_| {
        malformed(
            format!("an IFD cannot hold {} entries", entries.len()),
            base,
        )
    })?;
    let entries_len = 12usize
        .checked_mul(entries.len())
        .and_then(|len| len.checked_add(2))
        .and_then(|len| len.checked_add(4))
        .ok_or_else(|| malformed(String::from("IFD size overflowed usize"), base))?;
    let values_start = base
        .checked_add(entries_len)
        .ok_or_else(|| malformed(String::from("IFD offset overflowed usize"), base))?;

    enum Layout {
        Field([u8; 4]),
        Offset(u32),
    }

    let mut layouts: Vec<Layout> = Vec::with_capacity(entries.len());
    let mut value_area: Vec<u8> = Vec::new();
    for entry in entries {
        if let Some(field) = entry.fixed_field {
            layouts.push(Layout::Field(field));
            continue;
        }
        let len = entry.declared_value_len()?;
        if entry.value.len() != len {
            return Err(malformed(
                format!(
                    "tag 0x{:04X} declares {len} value bytes but carries {}",
                    entry.tag,
                    entry.value.len()
                ),
                base,
            ));
        }
        if len <= 4 {
            let mut field = [0u8; 4];
            let Some(target) = field.get_mut(..len) else {
                return Err(malformed(
                    format!("tag 0x{:04X} has an impossible inline length", entry.tag),
                    base,
                ));
            };
            target.copy_from_slice(&entry.value);
            layouts.push(Layout::Field(field));
        } else {
            let unaligned = values_start
                .checked_add(value_area.len())
                .ok_or_else(|| malformed(String::from("IFD value area overflowed"), base))?;
            let offset = align4(unaligned)
                .ok_or_else(|| malformed(String::from("IFD value offset overflowed"), base))?;
            let padding = offset - unaligned;
            value_area.extend(std::iter::repeat_n(0u8, padding));
            let offset = u32::try_from(offset).map_err(|_| {
                malformed(
                    format!(
                        "the Exif block has grown past 4 GiB at tag 0x{:04X}",
                        entry.tag
                    ),
                    base,
                )
            })?;
            layouts.push(Layout::Offset(offset));
            value_area.extend_from_slice(&entry.value);
        }
    }

    tiff.extend_from_slice(&order.u16_bytes(entry_count));
    for (entry, layout) in entries.iter().zip(layouts.iter()) {
        tiff.extend_from_slice(&order.u16_bytes(entry.tag));
        tiff.extend_from_slice(&order.u16_bytes(entry.field_type));
        tiff.extend_from_slice(&order.u32_bytes(entry.count));
        match layout {
            Layout::Field(field) => tiff.extend_from_slice(field),
            Layout::Offset(offset) => tiff.extend_from_slice(&order.u32_bytes(*offset)),
        }
    }
    tiff.extend_from_slice(&order.u32_bytes(next));
    tiff.extend_from_slice(&value_area);
    u32::try_from(base).map_err(|_| malformed(String::from("TIFF block is past 4 GiB"), base))
}

/// Replace the GPS IFD of an existing TIFF block without moving any old bytes.
fn rewrite_tiff(reader: &TiffReader, position: &GpsPosition) -> Result<Vec<u8>> {
    let order = reader.order;
    let ifd0 = reader.ifd(reader.ifd0_offset()?)?;
    let gps_index = ifd0
        .entries
        .iter()
        .position(|entry| entry.tag == tag::GPS_IFD_POINTER);

    // Merge: keep every GPS tag this module does not own, replace the rest.
    let old_gps = match gps_index {
        Some(index) => {
            let Some(pointer) = ifd0.entries.get(index) else {
                return Err(malformed(
                    String::from("the GPS IFD pointer disappeared between reads"),
                    0,
                ));
            };
            Some(reader.ifd(order.u32(pointer.field))?)
        }
        None => None,
    };
    let mut gps_entries: Vec<IfdEntry> = Vec::new();
    if let Some(old) = &old_gps {
        gps_entries.extend(
            old.entries
                .iter()
                .filter(|entry| !tag::MANAGED_GPS.contains(&entry.tag))
                .map(IfdEntry::from_raw),
        );
    }
    gps_entries.extend(gps_entries_for(position, order)?);
    // TIFF requires the entries of an IFD in ascending tag order.
    gps_entries.sort_by_key(|entry| entry.tag);

    let mut tiff = reader.data.to_vec();
    let gps_offset = append_ifd(&mut tiff, &gps_entries, order, 0)?;

    let mut ifd0_entries: Vec<IfdEntry> = ifd0.entries.iter().map(IfdEntry::from_raw).collect();
    match gps_index {
        Some(index) => {
            let Some(slot) = ifd0_entries.get_mut(index) else {
                return Err(malformed(
                    String::from("the GPS IFD pointer disappeared between reads"),
                    0,
                ));
            };
            // Normalise the pointer entry: whatever type the old file used, a
            // pointer is LONG 1 by definition.
            *slot = IfdEntry::long(tag::GPS_IFD_POINTER, gps_offset, order);
        }
        None => {
            let insert_at = ifd0_entries
                .iter()
                .position(|entry| entry.tag > tag::GPS_IFD_POINTER)
                .unwrap_or(ifd0_entries.len());
            ifd0_entries.insert(
                insert_at,
                IfdEntry::long(tag::GPS_IFD_POINTER, gps_offset, order),
            );
        }
    }
    let ifd0_offset = append_ifd(&mut tiff, &ifd0_entries, order, ifd0.next)?;

    // The TIFF header's IFD0 pointer is at offset 4, after the byte-order mark
    // and the magic number.
    let Some(header_field) = tiff.get_mut(4..8) else {
        return Err(malformed(
            String::from("the Exif TIFF block lost its header"),
            0,
        ));
    };
    header_field.copy_from_slice(&order.u32_bytes(ifd0_offset));
    Ok(tiff)
}

/// Build the GPS entries for `position`, dropping the ones it has no value for
/// so stale tags from an earlier write are not left behind.
fn gps_entries_for(position: &GpsPosition, order: ByteOrder) -> Result<Vec<IfdEntry>> {
    gps_entries(position, order)
}

/// Build a fresh Exif block: a TIFF with IFD0 holding only the GPS pointer.
fn fresh_tiff(position: &GpsPosition) -> Result<Vec<u8>> {
    let order = ByteOrder::Little;
    // Reserve the eight-byte TIFF header; the IFDs are appended after it and the
    // header is filled once their offsets are known.
    let mut tiff = vec![0u8; 8];
    let gps = gps_entries_for(position, order)?;
    let gps_offset = append_ifd(&mut tiff, &gps, order, 0)?;
    let ifd0 = [IfdEntry::long(tag::GPS_IFD_POINTER, gps_offset, order)];
    let ifd0_offset = append_ifd(&mut tiff, &ifd0, order, 0)?;

    let Some(byte_order) = tiff.get_mut(..2) else {
        return Err(malformed(String::from("fresh TIFF header vanished"), 0));
    };
    byte_order.copy_from_slice(b"II");
    let Some(magic) = tiff.get_mut(2..4) else {
        return Err(malformed(String::from("fresh TIFF header vanished"), 2));
    };
    magic.copy_from_slice(&order.u16_bytes(42));
    let Some(ifd0_field) = tiff.get_mut(4..8) else {
        return Err(malformed(String::from("fresh TIFF header vanished"), 4));
    };
    ifd0_field.copy_from_slice(&order.u32_bytes(ifd0_offset));
    Ok(tiff)
}

/// Insert a brand-new Exif APP1 segment right after the JPEG's SOI marker.
fn insert_fresh_exif(jpeg: &[u8], position: &GpsPosition) -> Result<Vec<u8>> {
    let tiff = fresh_tiff(position)?;
    let payload_len = EXIF_HEADER
        .len()
        .checked_add(tiff.len())
        .ok_or_else(|| malformed(String::from("the Exif block length overflowed usize"), 0))?;
    let segment_len = payload_len
        .checked_add(2)
        .ok_or_else(|| malformed(String::from("the Exif block length overflowed usize"), 0))?;
    if segment_len > MAX_SEGMENT_LENGTH {
        return Err(malformed(
            format!("a new Exif block of {segment_len} bytes does not fit a JPEG segment"),
            0,
        ));
    }
    let segment_len = u16::try_from(segment_len)
        .map_err(|_| malformed(String::from("Exif segment length does not fit 16 bits"), 0))?;
    let Some(tiff_prefix) = jpeg.get(..2) else {
        return Err(not_a_jpeg(jpeg.len()));
    };
    let Some(tiff_rest) = jpeg.get(2..) else {
        return Err(not_a_jpeg(jpeg.len()));
    };
    let mut out = Vec::with_capacity(jpeg.len() + segment_len as usize);
    out.extend_from_slice(tiff_prefix);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&segment_len.to_be_bytes());
    out.extend_from_slice(EXIF_HEADER);
    out.extend_from_slice(&tiff);
    out.extend_from_slice(tiff_rest);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
///
/// Howard Hinnant's `days_from_civil`; written out because the crate has no
/// date library and this is the only calendar arithmetic it needs.
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// The Gregorian date for a count of days since 1970-01-01.
///
/// Hinnant's `civil_from_days`, the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// Parse `"YYYY:MM:DD"` (also tolerates `-` separators), as `GPSDateStamp` holds.
fn parse_date_stamp(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split([':', '-']);
    let year: u32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let day: u32 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(i64::from(year), month) {
        return None;
    }
    Some((year, month, day))
}

/// Parse `"YYYY:MM:DD HH:MM:SS"` into Unix seconds, reading it as UTC.
fn parse_datetime_text(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date_part, time_part) = text
        .split_once(' ')
        .or_else(|| text.split_once('T'))
        .unwrap_or((text, "00:00:00"));
    let (year, month, day) = parse_date_stamp(date_part)?;
    let mut clock = time_part.split(':');
    let hour: u32 = clock.next()?.trim().parse().ok()?;
    let minute: u32 = clock.next()?.trim().parse().ok()?;
    let second: u32 = match clock.next() {
        Some(raw) => raw.trim().parse().ok()?,
        None => 0,
    };
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day)?;
    days.checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::fixture;

    /// The repository's example coordinate. Never a real place anyone stood.
    const TOKYO_LAT: f64 = 35.681_236;
    const TOKYO_LON: f64 = 139.767_125;
    /// 2023-11-14 22:13:20 UTC, the moment the {@link tokyo} fixture was taken.
    const TOKYO_UTC: u32 = 1_700_000_000;
    const TOKYO_DATETIME: &str = "2023:11:14 22:13:20";

    fn tokyo() -> GpsPosition {
        GpsPosition {
            latitude: TOKYO_LAT,
            longitude: TOKYO_LON,
            altitude_m: Some(80.0),
            utc: Some(TOKYO_UTC),
        }
    }

    /// Every GPS tag the fixture writes, in either byte order.
    fn gps_entries(order: ByteOrder) -> Vec<fixture::Entry> {
        vec![
            fixture::Entry::ascii(tag::GPS_LATITUDE_REF, "N"),
            fixture::Entry::rationals(
                tag::GPS_LATITUDE,
                &[(35, 1), (40, 1), (52_449_600, 1_000_000)],
                order,
            ),
            fixture::Entry::ascii(tag::GPS_LONGITUDE_REF, "E"),
            fixture::Entry::rationals(
                tag::GPS_LONGITUDE,
                &[(139, 1), (46, 1), (1_650_000, 1_000_000)],
                order,
            ),
            fixture::Entry::byte(tag::GPS_ALTITUDE_REF, 0),
            fixture::Entry::rationals(tag::GPS_ALTITUDE, &[(80_000, 1000)], order),
            fixture::Entry::rationals(tag::GPS_TIME_STAMP, &[(22, 1), (13, 1), (20, 1)], order),
            fixture::Entry::ascii(tag::GPS_DATE_STAMP, "2023:11:14"),
        ]
    }

    /// The Exif APP1's TIFF block, or a panic if the file has none.
    fn exif_tiff(jpeg: &[u8]) -> Vec<u8> {
        let location = find_exif_app1(jpeg)
            .expect("a readable segment chain")
            .expect("an Exif APP1 segment");
        tiff_block(jpeg, location).expect("the TIFF block").to_vec()
    }

    /// The raw contents of IFD0 and the GPS IFD, for checking the tags the
    /// public API does not expose.
    struct RawExif {
        order: ByteOrder,
        ifd0: Vec<(u16, Vec<u8>)>,
        gps: Vec<(u16, Vec<u8>)>,
    }

    fn raw_exif(jpeg: &[u8]) -> RawExif {
        let location = find_exif_app1(jpeg)
            .expect("a readable segment chain")
            .expect("an Exif APP1 segment");
        let tiff = tiff_block(jpeg, location).expect("the TIFF block");
        let reader = TiffReader::parse(tiff).expect("a TIFF header");
        let ifd0 = reader
            .ifd(reader.ifd0_offset().expect("an IFD0 offset"))
            .expect("IFD0");
        let collect = |ifd: &Ifd| -> Vec<(u16, Vec<u8>)> {
            ifd.entries
                .iter()
                .map(|entry| {
                    (
                        entry.tag,
                        reader.value_bytes(entry).expect("a readable value"),
                    )
                })
                .collect()
        };
        let gps = ifd0
            .find(tag::GPS_IFD_POINTER)
            .map(|pointer| {
                let offset = reader.order.u32(pointer.field);
                reader.ifd(offset).expect("the GPS IFD")
            })
            .map(|ifd| collect(&ifd))
            .unwrap_or_default();
        RawExif {
            order: reader.order,
            ifd0: collect(&ifd0),
            gps,
        }
    }

    impl RawExif {
        fn gps_tags(&self) -> Vec<u16> {
            self.gps.iter().map(|(tag, _)| *tag).collect()
        }

        fn gps_value(&self, want: u16) -> Option<&[u8]> {
            self.gps
                .iter()
                .find(|(tag, _)| *tag == want)
                .map(|(_, value)| value.as_slice())
        }

        fn ifd0_value(&self, want: u16) -> Option<&[u8]> {
            self.ifd0
                .iter()
                .find(|(tag, _)| *tag == want)
                .map(|(_, value)| value.as_slice())
        }

        /// An ASCII tag's text, without its terminator and padding.
        fn gps_text(&self, want: u16) -> Option<String> {
            self.gps_value(want).map(ascii_text_of)
        }

        fn ifd0_text(&self, want: u16) -> Option<String> {
            self.ifd0_value(want).map(ascii_text_of)
        }

        /// A RATIONAL tag's pairs, decoded in the block's byte order.
        fn gps_rationals(&self, want: u16) -> Vec<(u32, u32)> {
            let Some(bytes) = self.gps_value(want) else {
                return Vec::new();
            };
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|&[a, b, c, d, e, f, g, h]| {
                    (self.order.u32([a, b, c, d]), self.order.u32([e, f, g, h]))
                })
                .collect()
        }
    }

    fn ascii_text_of(bytes: &[u8]) -> String {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        String::from_utf8_lossy(bytes.get(..end).unwrap_or_default())
            .trim_end()
            .to_owned()
    }

    fn assert_degrees(actual: Option<f64>, expected: f64) {
        let actual = actual.expect("a coordinate");
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn a_jpeg_without_exif_gains_a_gps_block_that_reads_back() {
        let plain = fixture::jpeg_without_exif();
        assert!(!has_exif(&plain).expect("a plain JPEG has no Exif"));
        assert_eq!(read_gps(&plain).expect("read"), None);
        assert_eq!(read_datetime_original(&plain).expect("read"), None);

        let tagged = write_gps(&plain, &tokyo()).expect("write a new block");
        assert!(has_exif(&tagged).expect("the new block is there"));
        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, TOKYO_LAT);
        assert_degrees(tags.longitude, TOKYO_LON);
        assert_eq!(tags.altitude_m, Some(80.0));
        assert_eq!(tags.utc, Some(TOKYO_UTC));
        // The picture itself is untouched: the segment chain after APP1 and the
        // end-of-image marker are still where the camera left them.
        assert!(
            tagged.ends_with(&[0xFF, JPEG_MARKER_EOI]),
            "lost the end-of-image"
        );
        assert!(tagged.len() > plain.len());
    }

    #[test]
    fn rewriting_keeps_the_tags_this_module_does_not_own() {
        let mut gps = gps_entries(ByteOrder::Little);
        // A GPS tag outside `MANAGED_GPS` and an IFD0 tag: a rewrite must leave
        // both exactly as they were.
        gps.push(fixture::Entry::ascii(0x0012, "WGS-84"));
        let artist = [fixture::Entry::ascii(0x013B, "Test Camera")];
        let jpeg = fixture::exif_jpeg(ByteOrder::Little, &artist, Some(&gps), Some(TOKYO_DATETIME));
        let before = exif_tiff(&jpeg);

        let moved = GpsPosition {
            latitude: 34.693_7,
            longitude: 135.502_3,
            altitude_m: Some(21.0),
            utc: Some(1_700_003_600),
        };
        let tagged = write_gps(&jpeg, &moved).expect("rewrite");

        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, moved.latitude);
        assert_degrees(tags.longitude, moved.longitude);
        assert_eq!(tags.altitude_m, Some(21.0));
        assert_eq!(tags.utc, moved.utc);

        let raw = raw_exif(&tagged);
        assert!(
            raw.gps_tags().contains(&0x0012),
            "GPSMapDatum disappeared: {:04X?}",
            raw.gps_tags()
        );
        assert_eq!(raw.gps_text(0x0012).as_deref(), Some("WGS-84"));
        assert_eq!(raw.ifd0_text(0x013B).as_deref(), Some("Test Camera"));
        // Appending, not rebuilding: every byte of the old block is still
        // there, apart from the header's IFD0 pointer, which is the one thing a
        // rewrite exists to change.
        let after = exif_tiff(&tagged);
        assert_eq!(
            after.get(8..before.len()),
            before.get(8..),
            "the rewrite moved bytes the old block referenced"
        );
        assert!(after.len() > before.len(), "nothing was appended");
        // The capture time lives in the Exif sub-IFD, which was not touched.
        assert_eq!(
            read_datetime_original(&tagged).expect("read"),
            Some(TOKYO_UTC)
        );
    }

    #[test]
    fn a_big_endian_tiff_is_read_and_rewritten_in_its_own_order() {
        let gps = gps_entries(ByteOrder::Big);
        let jpeg = fixture::exif_jpeg(ByteOrder::Big, &[], Some(&gps), Some(TOKYO_DATETIME));
        let before = read_gps(&jpeg).expect("read").expect("an Exif block");
        assert_degrees(before.latitude, TOKYO_LAT);

        let tagged = write_gps(&jpeg, &tokyo()).expect("rewrite");
        let raw = raw_exif(&tagged);
        assert_eq!(
            raw.order,
            ByteOrder::Big,
            "a rewrite must not switch a MM file to II"
        );
        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, TOKYO_LAT);
        assert_eq!(tags.utc, Some(TOKYO_UTC));
    }

    #[test]
    fn a_truncated_jpeg_is_an_error_not_a_panic() {
        let jpeg = fixture::exif_jpeg(
            ByteOrder::Little,
            &[],
            Some(&gps_entries(ByteOrder::Little)),
            Some(TOKYO_DATETIME),
        );
        let location = find_exif_app1(&jpeg)
            .expect("segment chain")
            .expect("exif segment");
        // Cut points inside everything this module has to read: the SOI, the
        // APP1 length field, the TIFF header, the IFDs, and the last byte of the
        // Exif segment. A cut after it leaves the Exif block intact, which is a
        // damaged *picture* rather than a damaged block, and is not this
        // module's to refuse.
        for cut in [1usize, 3, 9, 24, 40, location.start + 2, location.end - 1] {
            let truncated = jpeg.get(..cut).expect("a cut inside the JPEG");
            assert!(
                read_gps(truncated).is_err(),
                "reading a {cut}-byte prefix should fail"
            );
            assert!(
                write_gps(truncated, &tokyo()).is_err(),
                "writing a {cut}-byte prefix should fail"
            );
        }
        // A segment whose length runs past the end of the file is the damaged
        // case a truncated download produces most often.
        let mut liar = jpeg.clone();
        let location = find_exif_app1(&jpeg)
            .expect("segment chain")
            .expect("exif segment");
        // The APP1 length field sits just past the 0xFF 0xE1 pair.
        liar[location.start + 2..location.start + 4].copy_from_slice(&0xFFFFu16.to_be_bytes());
        assert!(read_gps(&liar).is_err());
    }

    #[test]
    fn a_png_is_refused_with_a_clear_error() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        for result in [has_exif(png), read_gps(png).map(|_| false)] {
            let err = result.expect_err("a PNG is not a JPEG");
            let text = err.to_string();
            assert!(
                text.contains("not a JPEG"),
                "the error should name the problem, got: {text}"
            );
        }
        let err = write_gps(png, &tokyo()).expect_err("a PNG is not a JPEG");
        assert!(err.to_string().contains("not a JPEG"), "got {err}");
    }

    #[test]
    fn south_and_west_references_survive_a_round_trip() {
        let sydney = GpsPosition {
            latitude: -33.868_8,
            longitude: -151.209_3,
            altitude_m: Some(2.0),
            utc: Some(TOKYO_UTC),
        };
        let tagged = fixture::exif_jpeg(
            ByteOrder::Little,
            &[],
            Some(&gps_entries(ByteOrder::Little)),
            None,
        );
        let tagged = write_gps(&tagged, &sydney).expect("rewrite");

        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, sydney.latitude);
        assert_degrees(tags.longitude, sydney.longitude);
        // The sign is carried by the reference letter, not the rational, so the
        // letters are what makes the read come back negative.
        let raw = raw_exif(&tagged);
        assert_eq!(
            raw.gps_text(tag::GPS_LATITUDE_REF).as_deref(),
            Some("S"),
            "a southern latitude needs an S reference"
        );
        assert_eq!(
            raw.gps_text(tag::GPS_LONGITUDE_REF).as_deref(),
            Some("W"),
            "a western longitude needs a W reference"
        );
    }

    #[test]
    fn a_fractional_arcsecond_round_trips_without_being_rounded_away() {
        // 12.5 seconds of arc: the value the RATIONAL denominator of 1,000,000
        // exists to carry. A writer that rounded to whole seconds would lose
        // about 15 metres here.
        let latitude = 35.0 + 40.0 / 60.0 + 12.5 / 3600.0;
        let longitude = -120.0 - 30.0 / 60.0 - 7.25 / 3600.0;
        let position = GpsPosition {
            latitude,
            longitude,
            altitude_m: Some(12.5),
            utc: Some(TOKYO_UTC),
        };
        let tagged = fixture::exif_jpeg(ByteOrder::Little, &[], None, None);
        let tagged = write_gps(&tagged, &position).expect("write");

        let raw = raw_exif(&tagged);
        assert_eq!(
            raw.gps_rationals(tag::GPS_LATITUDE).last().copied(),
            Some((12_500_000, 1_000_000)),
            "12.5 seconds of arc must be stored as 12.5, not 12 or 13"
        );
        assert_eq!(
            raw.gps_rationals(tag::GPS_LONGITUDE).last().copied(),
            Some((7_250_000, 1_000_000))
        );

        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, latitude);
        assert_degrees(tags.longitude, longitude);
        // Altitude has the same fractional case, at millimetre precision.
        assert_eq!(tags.altitude_m, Some(12.5));
    }

    #[test]
    fn a_fractional_timestamp_second_reads_as_its_whole_second() {
        // EXIF's GPSTimeStamp is a RATIONAL, so a camera may write 25/2 for
        // 12.5 seconds. The crate's timestamp is whole Unix seconds, so the
        // fraction is dropped rather than rounded up into the next second.
        let timestamp = vec![fixture::Entry::rationals(
            tag::GPS_TIME_STAMP,
            &[(22, 1), (13, 1), (25, 2)],
            ByteOrder::Little,
        )];
        let mut gps = timestamp;
        gps.push(fixture::Entry::ascii(tag::GPS_DATE_STAMP, "2023:11:14"));
        let jpeg = fixture::exif_jpeg(ByteOrder::Little, &[], Some(&gps), None);

        let tags = read_gps(&jpeg).expect("read").expect("an Exif block");
        let expected = parse_datetime_text("2023:11:14 22:13:12").expect("a known date");
        assert_eq!(
            tags.utc,
            Some(u32::try_from(expected).expect("a Unix time"))
        );
    }

    #[test]
    fn an_exif_block_at_the_segment_limit_is_refused() {
        // A block already holding the largest TIFF payload an APP1 segment can
        // carry: appending a GPS IFD would push the segment past 65,535 bytes.
        let full = fixture::padded_exif_jpeg(ByteOrder::Little, MAX_TIFF_BYTES);
        assert_eq!(
            exif_tiff(&full).len(),
            MAX_TIFF_BYTES,
            "the fixture must fill the segment exactly"
        );
        let err = write_gps(&full, &tokyo()).expect_err("the segment is full");
        let text = err.to_string();
        assert!(
            text.contains("at most 65535") && text.contains("left untouched"),
            "the error must say the segment limit was hit and the photo kept: {text}"
        );

        // Just under the limit, the same rewrite succeeds: the refusal is about
        // size, not about this being an unusual file.
        let room = fixture::padded_exif_jpeg(ByteOrder::Little, MAX_TIFF_BYTES - 1024);
        let tagged = write_gps(&room, &tokyo()).expect("1024 bytes of headroom is enough");
        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_degrees(tags.latitude, TOKYO_LAT);
    }

    #[test]
    fn an_ifd_offset_inside_the_tiff_header_is_rejected() {
        let base = fixture::exif_jpeg(
            ByteOrder::Little,
            &[],
            Some(&gps_entries(ByteOrder::Little)),
            Some(TOKYO_DATETIME),
        );
        let location = find_exif_app1(&base)
            .expect("segment chain")
            .expect("exif segment");
        // Overwrite the IFD0 offset at bytes 4..8 of the TIFF block with values
        // inside the 8-byte TIFF header.
        for bad_offset in [0u32, 4u32, 7u32] {
            let mut jpeg = base.clone();
            let offset_pos = location.tiff_start + 4;
            jpeg[offset_pos..offset_pos + 4].copy_from_slice(&bad_offset.to_le_bytes());
            let err = read_gps(&jpeg).expect_err("IFD offset < 8 must fail");
            assert!(
                matches!(err, Error::Log { .. }) && err.to_string().contains("TIFF header"),
                "expected malformed TIFF header error for offset {bad_offset}, got: {err}"
            );
            assert!(read_datetime_original(&jpeg).is_err());
            assert!(write_gps(&jpeg, &tokyo()).is_err());
        }
    }

    #[test]
    fn gps_time_out_of_range_and_zero_denominators_are_rejected_consistently() {
        // Invalid hour/minute/second in GPSTimeStamp must fail rather than
        // roll over into the next day or return Ok(None).
        for (label, triple) in [
            ("hour 24", [(24, 1), (0, 1), (0, 1)]),
            ("minute 60", [(12, 1), (60, 1), (0, 1)]),
            ("second 61", [(12, 1), (0, 1), (61, 1)]),
        ] {
            let gps = vec![
                fixture::Entry::rationals(tag::GPS_TIME_STAMP, &triple, ByteOrder::Little),
                fixture::Entry::ascii(tag::GPS_DATE_STAMP, "2023:11:14"),
            ];
            let jpeg = fixture::exif_jpeg(ByteOrder::Little, &[], Some(&gps), None);
            let err = read_gps(&jpeg).expect_err(label);
            assert!(
                matches!(err, Error::Log { .. }),
                "{label} should return Error::Log, got: {err:?}"
            );
        }

        // Zero denominator must return Error::Log consistently across
        // GPSAltitude, GPSTimeStamp, GPSLatitude, and GPSLongitude.
        let cases: &[(&str, Vec<fixture::Entry>)] = &[
            (
                "GPSAltitude",
                vec![fixture::Entry::rationals(
                    tag::GPS_ALTITUDE,
                    &[(80, 0)],
                    ByteOrder::Little,
                )],
            ),
            (
                "GPSTimeStamp",
                vec![
                    fixture::Entry::rationals(
                        tag::GPS_TIME_STAMP,
                        &[(12, 1), (30, 0), (0, 1)],
                        ByteOrder::Little,
                    ),
                    fixture::Entry::ascii(tag::GPS_DATE_STAMP, "2023:11:14"),
                ],
            ),
            (
                "GPSLatitude",
                vec![fixture::Entry::rationals(
                    tag::GPS_LATITUDE,
                    &[(35, 1), (40, 0), (0, 1)],
                    ByteOrder::Little,
                )],
            ),
            (
                "GPSLongitude",
                vec![fixture::Entry::rationals(
                    tag::GPS_LONGITUDE,
                    &[(139, 0), (46, 1), (0, 1)],
                    ByteOrder::Little,
                )],
            ),
        ];
        for (label, gps) in cases {
            let jpeg = fixture::exif_jpeg(ByteOrder::Little, &[], Some(gps), None);
            let err = read_gps(&jpeg).expect_err(label);
            assert!(
                matches!(err, Error::Log { .. }) && err.to_string().contains("zero denominator"),
                "{label} zero denominator should return malformed error, got: {err}"
            );
        }
    }

    #[test]
    fn datetime_original_outside_u32_range_is_rejected_as_malformed() {
        for stamp in ["1969:12:31 23:59:59", "2107:01:01 00:00:00"] {
            let jpeg = fixture::exif_jpeg(ByteOrder::Little, &[], None, Some(stamp));
            let err = read_datetime_original(&jpeg)
                .expect_err("out-of-u32 DateTimeOriginal must return Err, not Ok(None)");
            assert!(
                matches!(err, Error::Log { .. }) && err.to_string().contains(stamp),
                "expected Error::Log mentioning {stamp}, got: {err}"
            );
        }
    }

    #[test]
    fn negative_altitude_round_trips_with_below_sea_level_reference() {
        let below = GpsPosition {
            latitude: TOKYO_LAT,
            longitude: TOKYO_LON,
            altitude_m: Some(-12.5),
            utc: Some(TOKYO_UTC),
        };
        let plain = fixture::jpeg_without_exif();
        let tagged = write_gps(&plain, &below).expect("write negative altitude");

        let raw = raw_exif(&tagged);
        assert_eq!(
            raw.gps_value(tag::GPS_ALTITUDE_REF),
            Some(&[1u8][..]),
            "below-sea-level altitude must set GPSAltitudeRef = 1"
        );
        assert_eq!(
            raw.gps_rationals(tag::GPS_ALTITUDE),
            vec![(12_500, 1000)],
            "GPSAltitude stores unsigned magnitude"
        );

        let tags = read_gps(&tagged).expect("read").expect("an Exif block");
        assert_eq!(tags.altitude_m, Some(-12.5));
    }

    #[test]
    fn invalid_gps_positions_are_rejected_before_writing() {
        let plain = fixture::jpeg_without_exif();
        let invalid = [
            GpsPosition::new(90.0001, 0.0),
            GpsPosition::new(-90.0001, 0.0),
            GpsPosition::new(f64::NAN, 0.0),
            GpsPosition::new(f64::INFINITY, 0.0),
            GpsPosition::new(0.0, 180.0001),
            GpsPosition::new(0.0, -180.0001),
            GpsPosition::new(0.0, f64::NAN),
            GpsPosition::new(0.0, f64::NEG_INFINITY),
            GpsPosition {
                latitude: TOKYO_LAT,
                longitude: TOKYO_LON,
                altitude_m: Some(f32::NAN),
                utc: None,
            },
            GpsPosition {
                latitude: TOKYO_LAT,
                longitude: TOKYO_LON,
                altitude_m: Some(f32::INFINITY),
                utc: None,
            },
            GpsPosition {
                latitude: TOKYO_LAT,
                longitude: TOKYO_LON,
                altitude_m: Some(f32::NEG_INFINITY),
                utc: None,
            },
        ];
        for position in invalid {
            let err = write_gps(&plain, &position).expect_err("invalid position must be rejected");
            assert!(
                matches!(err, Error::State(_)),
                "expected Error::State for {position:?}, got: {err:?}"
            );
        }

        // Exact poles and antimeridian remain valid.
        for boundary in [
            GpsPosition::new(90.0, 180.0),
            GpsPosition::new(-90.0, -180.0),
        ] {
            let tagged = write_gps(&plain, &boundary).expect("boundary coordinate is valid");
            let tags = read_gps(&tagged).expect("read").expect("an Exif block");
            assert_degrees(tags.latitude, boundary.latitude);
            assert_degrees(tags.longitude, boundary.longitude);
        }
    }

    #[test]
    fn a_non_app1_segment_overflow_does_not_blame_app1() {
        let mut jpeg = fixture::jpeg_without_exif();
        // Bytes 2..4 are the JFIF APP0 marker (FF E0); bytes 4..6 are its length.
        assert_eq!(&jpeg[2..4], &[0xFF, 0xE0]);
        jpeg[4..6].copy_from_slice(&0xFFFFu16.to_be_bytes());
        let err = read_gps(&jpeg).expect_err("truncated APP0 segment must fail");
        let text = err.to_string();
        assert!(
            text.contains("0xFFE0") && !text.contains("APP1"),
            "error should identify segment 0xFFE0 rather than APP1, got: {text}"
        );
    }
}
