//! Synthetic JPEGs shared by the Exif and geotag tests.
//!
//! Real photographs are personal data, and this repository must not contain a
//! real track either, so every JPEG a test reads is built here:
//! start-of-image, a JFIF APP0, an optional APP1, a dummy quantisation table and
//! end-of-image. Only the markers the segment scanner walks are present — there
//! is no entropy-coded image data and nothing in this crate decodes a picture.
//!
//! The Exif block is a small but complete TIFF: a header, an IFD0 with the
//! pointers and extra tags the caller asked for, the GPS IFD, and an Exif
//! sub-IFD holding `DateTimeOriginal` when a capture time is wanted. Values that
//! do not fit in an entry's four inline bytes go into a value area after the
//! IFDs, which is how a real camera lays them out and what makes the reader's
//! offset handling testable.
//!
//! This module is compiled only for tests (`#[cfg(test)]` in `log/mod.rs`), so it
//! may index and unwrap freely: a panic here is a broken fixture, not a broken
//! photo.

use crate::log::exif::{ByteOrder, JPEG_MARKER_EOI, JPEG_MARKER_SOI};

/// TIFF field type ids. Written out rather than borrowed from `exif` because a
/// fixture has to be able to build blocks the module's own writers never
/// produce, including deliberately malformed ones.
const TYPE_BYTE: u16 = 1;
const TYPE_ASCII: u16 = 2;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_UNDEFINED: u16 = 7;

/// The pointer tags the fixture writes. The GPS tag ids come from
/// [`crate::log::exif::tag`] so a tag rename is a compile error here.
const EXIF_IFD_POINTER: u16 = 0x8769;
const GPS_IFD_POINTER: u16 = 0x8825;
const DATE_TIME_ORIGINAL: u16 = 0x9003;

/// One IFD entry to be written: its tag, TIFF type and value bytes exactly as
/// they appear in the file (already in the block's byte order where that
/// matters).
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    tag: u16,
    field_type: u16,
    payload: Vec<u8>,
}

impl Entry {
    /// An ASCII value. The count includes the terminating NUL, as EXIF requires.
    pub(crate) fn ascii(tag: u16, text: &str) -> Self {
        let mut payload = text.as_bytes().to_vec();
        payload.push(0);
        Self {
            tag,
            field_type: TYPE_ASCII,
            payload,
        }
    }

    /// One BYTE, which is how `GPSAltitudeRef` stores its sign.
    pub(crate) fn byte(tag: u16, value: u8) -> Self {
        Self {
            tag,
            field_type: TYPE_BYTE,
            payload: vec![value],
        }
    }

    /// A list of unsigned rationals, written in `order`.
    pub(crate) fn rationals(tag: u16, pairs: &[(u32, u32)], order: ByteOrder) -> Self {
        let mut payload = Vec::with_capacity(pairs.len() * 8);
        for (numerator, denominator) in pairs {
            payload.extend_from_slice(&order.u32_bytes(*numerator));
            payload.extend_from_slice(&order.u32_bytes(*denominator));
        }
        Self {
            tag,
            field_type: TYPE_RATIONAL,
            payload,
        }
    }

    /// A blob of UNDEFINED bytes, for a block that is deliberately nearly full.
    pub(crate) fn undefined(tag: u16, payload: Vec<u8>) -> Self {
        Self {
            tag,
            field_type: TYPE_UNDEFINED,
            payload,
        }
    }

    /// `GPSVersionID` with a type other than BYTE, to exercise the reader's
    /// "not the shape the spec allows" path.
    pub(crate) fn undefined_version_id(payload: Vec<u8>) -> Self {
        Self::undefined(crate::log::exif::tag::GPS_VERSION_ID, payload)
    }

    /// `GPSVersionID` as a BYTE tag with `payload.len()` values, which is BYTE
    /// but not the four the spec requires.
    pub(crate) fn version_id_bytes(payload: Vec<u8>) -> Self {
        Self {
            tag: crate::log::exif::tag::GPS_VERSION_ID,
            field_type: TYPE_BYTE,
            payload,
        }
    }
}

/// A JPEG with no Exif at all: SOI, a JFIF APP0, a dummy DQT and EOI.
pub(crate) fn jpeg_without_exif() -> Vec<u8> {
    jpeg_with_payload(None)
}

/// A JPEG whose first APP1 payload is `payload` — Exif when it starts with
/// `Exif\0\0`, something else (XMP, for instance) when it does not.
fn jpeg_with_payload(payload: Option<&[u8]>) -> Vec<u8> {
    let mut out = vec![0xFF, JPEG_MARKER_SOI];
    push_segment(&mut out, 0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    if let Some(payload) = payload {
        push_segment(&mut out, 0xE1, payload);
    }
    // A quantisation table segment, so the chain contains a segment that is
    // neither APP1 nor a marker the scanner treats specially.
    let mut dqt = vec![0x00];
    dqt.extend(std::iter::repeat_n(0x10u8, 64));
    push_segment(&mut out, 0xDB, &dqt);
    out.extend_from_slice(&[0xFF, JPEG_MARKER_EOI]);
    out
}

/// A JPEG carrying a hand-built Exif APP1.
///
/// `ifd0_extra` goes into IFD0 beside the pointers; `gps` is the GPS IFD, or
/// `None` for a block with no GPS IFD at all (a photo from a camera that never
/// had GPS, which is the case a rewrite has to insert a pointer into);
/// `datetime` is `DateTimeOriginal` in EXIF's `"YYYY:MM:DD HH:MM:SS"` form.
pub(crate) fn exif_jpeg(
    order: ByteOrder,
    ifd0_extra: &[Entry],
    gps: Option<&[Entry]>,
    datetime: Option<&str>,
) -> Vec<u8> {
    let block = tiff(order, ifd0_extra, gps, datetime);
    let mut payload = Vec::with_capacity(6 + block.len());
    payload.extend_from_slice(b"Exif\0\0");
    payload.extend_from_slice(&block);
    jpeg_with_payload(Some(&payload))
}

/// A JPEG whose Exif TIFF block is `tiff_bytes` long, padded with one UNDEFINED
/// entry in IFD0.
///
/// This is how the 64 KiB segment limit is reached: the block has to be built
/// large and legal, not corrupted, so the error comes from the writer's size
/// check rather than from the parser.
pub(crate) fn padded_exif_jpeg(order: ByteOrder, tiff_bytes: usize) -> Vec<u8> {
    // header 8 + IFD0 (2 + 12 + 4) = 26; the value area starts after alignment.
    let value_start = align4(8 + 2 + 12 + 4);
    let payload_len = tiff_bytes
        .checked_sub(value_start)
        .expect("the requested size has room for one entry");
    let padding = Entry::undefined(0xEA1C, vec![0u8; payload_len]);
    let block = tiff(order, std::slice::from_ref(&padding), None, None);
    assert_eq!(
        block.len(),
        tiff_bytes,
        "the padding entry must land on the requested TIFF size"
    );
    let mut payload = Vec::with_capacity(6 + block.len());
    payload.extend_from_slice(b"Exif\0\0");
    payload.extend_from_slice(&block);
    jpeg_with_payload(Some(&payload))
}

/// `0xFF`, the marker code, the two-byte big-endian length, then the payload.
fn push_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    let length = u16::try_from(payload.len() + 2).expect("a test segment fits 16 bits");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
}

/// One entry as it will be written: the count and the four field bytes.
#[derive(Debug)]
struct Placed {
    tag: u16,
    field_type: u16,
    count: u32,
    field: [u8; 4],
}

/// Build the TIFF block. Offsets are computed before anything is written, so
/// the IFDs can point at each other and the value area can follow in one pass.
fn tiff(
    order: ByteOrder,
    ifd0_extra: &[Entry],
    gps: Option<&[Entry]>,
    datetime: Option<&str>,
) -> Vec<u8> {
    let datetime_entry = datetime.map(|text| Entry {
        tag: DATE_TIME_ORIGINAL,
        field_type: TYPE_ASCII,
        payload: datetime_payload(text),
    });
    let gps_entries = gps.unwrap_or(&[]);
    let has_gps = gps.is_some();

    let ifd0_count = usize::from(has_gps) + ifd0_extra.len() + usize::from(datetime.is_some());
    let ifd0_offset = 8usize;
    let gps_offset = ifd0_offset + 2 + 12 * ifd0_count + 4;
    let gps_size = if has_gps {
        2 + 12 * gps_entries.len() + 4
    } else {
        0
    };
    let exif_offset = gps_offset + gps_size;
    let exif_size = if datetime.is_some() { 2 + 12 + 4 } else { 0 };
    let mut cursor = align4(exif_offset + exif_size);
    let mut values: Vec<(usize, Vec<u8>)> = Vec::new();

    let mut ifd0 = Vec::with_capacity(ifd0_count);
    if datetime.is_some() {
        ifd0.push(pointer(EXIF_IFD_POINTER, exif_offset, order));
    }
    if has_gps {
        ifd0.push(pointer(GPS_IFD_POINTER, gps_offset, order));
    }
    for entry in ifd0_extra {
        ifd0.push(place(order, entry, &mut cursor, &mut values));
    }

    let mut gps_placed: Vec<Placed> = gps_entries
        .iter()
        .map(|entry| place(order, entry, &mut cursor, &mut values))
        .collect();
    let exif_placed: Vec<Placed> = datetime_entry
        .iter()
        .map(|entry| place(order, entry, &mut cursor, &mut values))
        .collect();

    // TIFF requires an IFD's entries in ascending tag order.
    ifd0.sort_by_key(|entry| entry.tag);
    gps_placed.sort_by_key(|entry| entry.tag);

    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(match order {
        ByteOrder::Little => b"II",
        ByteOrder::Big => b"MM",
    });
    out.extend_from_slice(&order.u16_bytes(42));
    out.extend_from_slice(&order.u32_bytes(u32::try_from(ifd0_offset).expect("fits")));
    emit_ifd(&mut out, order, &ifd0);
    debug_assert_eq!(out.len(), gps_offset, "the GPS IFD offset was miscounted");
    if has_gps {
        emit_ifd(&mut out, order, &gps_placed);
    }
    if datetime.is_some() {
        debug_assert_eq!(out.len(), exif_offset, "the Exif IFD offset was miscounted");
        emit_ifd(&mut out, order, &exif_placed);
    }
    for (offset, payload) in values {
        while out.len() < offset {
            out.push(0);
        }
        out.extend_from_slice(&payload);
    }
    out
}

/// `DateTimeOriginal` is ASCII 20: nineteen characters and one NUL.
fn datetime_payload(text: &str) -> Vec<u8> {
    let mut payload = text.as_bytes().to_vec();
    payload.truncate(19);
    while payload.len() < 19 {
        payload.push(b' ');
    }
    payload.push(0);
    payload
}

/// A LONG 1 entry holding an IFD offset.
fn pointer(tag: u16, offset: usize, order: ByteOrder) -> Placed {
    Placed {
        tag,
        field_type: TYPE_LONG,
        count: 1,
        field: order.u32_bytes(u32::try_from(offset).expect("the fixture fits 4 GiB")),
    }
}

/// Lay one entry out: inline when its value fits in four bytes, otherwise in the
/// value area at a four-byte-aligned offset.
fn place(
    order: ByteOrder,
    entry: &Entry,
    cursor: &mut usize,
    values: &mut Vec<(usize, Vec<u8>)>,
) -> Placed {
    let count = u32::try_from(entry.payload.len() / unit_size(entry.field_type))
        .expect("an entry count fits 32 bits");
    let field = if entry.payload.len() <= 4 {
        let mut field = [0u8; 4];
        let inline = field
            .get_mut(..entry.payload.len())
            .expect("the payload fits inline");
        inline.copy_from_slice(&entry.payload);
        field
    } else {
        let offset = align4(*cursor);
        *cursor = offset + entry.payload.len();
        values.push((offset, entry.payload.clone()));
        order.u32_bytes(u32::try_from(offset).expect("the fixture fits 4 GiB"))
    };
    Placed {
        tag: entry.tag,
        field_type: entry.field_type,
        count,
        field,
    }
}

fn emit_ifd(out: &mut Vec<u8>, order: ByteOrder, entries: &[Placed]) {
    let count = u16::try_from(entries.len()).expect("an IFD count fits 16 bits");
    out.extend_from_slice(&order.u16_bytes(count));
    for entry in entries {
        out.extend_from_slice(&order.u16_bytes(entry.tag));
        out.extend_from_slice(&order.u16_bytes(entry.field_type));
        out.extend_from_slice(&order.u32_bytes(entry.count));
        out.extend_from_slice(&entry.field);
    }
    // No next IFD: the fixture writes one per kind of IFD only.
    out.extend_from_slice(&order.u32_bytes(0));
}

fn unit_size(field_type: u16) -> usize {
    match field_type {
        TYPE_BYTE | TYPE_ASCII | TYPE_UNDEFINED => 1,
        TYPE_RATIONAL => 8,
        _ => 4,
    }
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}
