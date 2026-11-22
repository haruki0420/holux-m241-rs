//! The 64 KiB blocks the M-241 writes its track log into.
//!
//! A dump is a flat copy of the log area. Every [`BLOCK_SIZE`] bytes starts with
//! a [`BLOCK_HEADER_SIZE`]-byte header, then variable-length records, then `0xFF`
//! padding up to the next block. The header carries a **bitmask** that decides
//! which fields each record in the block contains, so two blocks in one dump can
//! have different record lengths — and a V1.13 unit can write 16-byte records
//! (UTC + lat + lon + height) or 20-byte records (plus a float speed) depending
//! on nothing but that mask.
//!
//! That is why nothing here keys a record length off the firmware version: the
//! block says what it holds, and this module reads the block.
//!
//! # Header layout (little-endian)
//!
//! ```text
//!  0x00  u16  records in this block; 0xFFFF means "not full, parse to padding"
//!  0x02  u32  bitmask: which fields each record carries
//!  0x06  u16  policy: 0x0104 logging disabled, 0x0106 enabled
//!  0x08  u32  logging interval x 10 (seconds)
//!  0x0C  u32  distance threshold x 10 (metres)
//!  0x10  u32  speed threshold x 10 (km/h)
//!  0x14  ..   bitmaps and UI defaults; not needed to parse a track
//! ```

use crate::error::{Error, Result};

/// Size of one log block in the dump.
pub const BLOCK_SIZE: usize = 0x1_0000;

/// Bytes of header at the start of every block.
pub const BLOCK_HEADER_SIZE: usize = 0x200;

/// Meaningful bytes of the header. The rest is padding and UI defaults.
pub const HEADER_FIELDS_SIZE: usize = 0x14;

/// The record-count value that means "this block is not full".
pub const COUNT_NOT_FULL: u16 = 0xFFFF;

/// Policy value recorded when logging was turned **off** at the start of a block.
pub const POLICY_LOG_DISABLED: u16 = 0x0104;

/// Policy value recorded when logging was turned **on**.
pub const POLICY_LOG_ENABLED: u16 = 0x0106;

/// Bit 31 is a Holux "low precision" marker, not a data field.
///
/// It set on real dumps (`0x8000001D`) and would otherwise be read as a demand
/// for a field that does not exist, shifting every later field by its width.
pub const HOLUX_EXTRA_BIT: u32 = 0x8000_0000;

/// Largest satellite count the format can express.
pub const MAX_SATELLITES: usize = 32;

/// Bit positions of the fields, in the order they appear inside a record.
pub mod bit {
    /// UTC as Unix seconds (`u32`).
    pub const UTC: u8 = 0;
    /// Fix validity flags (`u16`).
    pub const VALID: u8 = 1;
    /// Latitude as a little-endian `f32`.
    pub const LATITUDE: u8 = 2;
    /// Longitude as a little-endian `f32`.
    pub const LONGITUDE: u8 = 3;
    /// Height: three most significant bytes of a little-endian `f32`.
    pub const HEIGHT: u8 = 4;
    /// Speed in km/h as an `f32`.
    pub const SPEED: u8 = 5;
    /// Heading in degrees as an `f32`.
    pub const HEADING: u8 = 6;
    /// DSTA (`u16`).
    pub const DSTA: u8 = 7;
    /// DAGE as an `f32`.
    pub const DAGE: u8 = 8;
    /// PDOP x 100 (`u16`).
    pub const PDOP: u8 = 9;
    /// HDOP x 100 (`u16`).
    pub const HDOP: u8 = 10;
    /// VDOP x 100 (`u16`).
    pub const VDOP: u8 = 11;
    /// Satellites in view / used (`u16`).
    pub const NSAT: u8 = 12;
    /// Satellite block: per-satellite records, variable length.
    pub const SID: u8 = 13;
    /// Elevation, carried inside the satellite block.
    pub const ELEVATION: u8 = 14;
    /// Azimuth, carried inside the satellite block.
    pub const AZIMUTH: u8 = 15;
    /// Signal-to-noise ratio, carried inside the satellite block.
    pub const SNR: u8 = 16;
    /// Log reason bitmask; bit 3 marks a manual waypoint.
    pub const RCR: u8 = 17;
    /// Milliseconds (`u16`).
    pub const MILLISECOND: u8 = 18;
    /// Distance as an `f64`.
    pub const DISTANCE: u8 = 19;
}

/// The highest bit that names a field.
pub const MAX_FIELD_BIT: u8 = bit::DISTANCE;

/// Bytes each field occupies on a V1.12 / V1.13 M-241.
///
/// The two quirks that cost real debugging time are here: latitude and longitude
/// are 4-byte floats (generic MTK loggers use doubles), and height is three bytes
/// (the high bytes of a float32). `None` means "carried inside the satellite
/// block" and has no fixed width.
pub const FIELD_SIZES: [Option<usize>; 20] = [
    Some(4), // 0 UTC
    Some(2), // 1 VALID
    Some(4), // 2 LATITUDE
    Some(4), // 3 LONGITUDE
    Some(3), // 4 HEIGHT
    Some(4), // 5 SPEED
    Some(4), // 6 HEADING
    Some(2), // 7 DSTA
    Some(4), // 8 DAGE
    Some(2), // 9 PDOP
    Some(2), // 10 HDOP
    Some(2), // 11 VDOP
    Some(2), // 12 NSAT
    None,    // 13 SID: variable, see `satellite_block_bytes`
    None,    // 14 ELEVATION (inside SID)
    None,    // 15 AZIMUTH (inside SID)
    None,    // 16 SNR (inside SID)
    Some(2), // 17 RCR
    Some(2), // 18 MILLISECOND
    Some(8), // 19 DISTANCE
];

/// Bytes one detail value (elevation, azimuth or SNR) occupies per satellite.
const SATELLITE_DETAIL_FIELD_BYTES: usize = 2;

/// Bytes of the satellite block that are not detail: the slot's own field.
///
/// A satellite slot is the SID field itself (4 bytes) followed by whichever
/// detail values the mask selects.
const SATELLITE_SLOT_BYTES: usize = 4;

/// Trailer bytes after the fields: one XOR byte, with **no** `'*'` separator.
///
/// Generic MTK loggers write `'*'` plus a checksum (two bytes). Reading two here
/// shifts every subsequent record by one byte and turns a track into noise.
pub const CHECKSUM_BYTES: usize = 1;

/// One decoded block header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    /// Byte offset of the block inside the dump.
    pub offset: usize,
    /// Records the block holds; `None` when the block is not full.
    pub count: Option<u16>,
    /// Which fields each record carries.
    pub bitmask: u32,
    /// Whether logging was on when the block started.
    pub policy: u16,
    /// Logging interval times ten.
    pub period_ds: u32,
    /// Distance threshold times ten.
    pub distance_dm: u32,
    /// Speed threshold times ten.
    pub speed_dkh: u32,
}

impl BlockHeader {
    /// Parse the header at the start of `block` (a slice that begins with it).
    ///
    /// # Errors
    ///
    /// [`Error::Log`] when fewer than [`HEADER_FIELDS_SIZE`] bytes are supplied.
    pub fn parse(block: &[u8], offset: usize) -> Result<Self> {
        let fields = block.get(..HEADER_FIELDS_SIZE).ok_or_else(|| Error::Log {
            message: format!(
                "truncated block header at 0x{offset:X}: {} of {HEADER_FIELDS_SIZE} bytes",
                block.len()
            ),
            offset: Some(offset),
        })?;
        let count = u16_at(fields, 0, offset)?;
        let bitmask = u32_le(fields, 2, offset)?;
        let policy = u16_at(fields, 6, offset)?;
        Ok(Self {
            offset,
            count: if count == COUNT_NOT_FULL {
                None
            } else {
                Some(count)
            },
            bitmask,
            policy,
            period_ds: u32_le(fields, 8, offset)?,
            distance_dm: u32_le(fields, 12, offset)?,
            speed_dkh: u32_le(fields, 16, offset)?,
        })
    }

    /// Whether the block says it was written while recording was enabled.
    #[must_use]
    pub fn logging_enabled(&self) -> bool {
        self.policy == POLICY_LOG_ENABLED
    }

    /// Logging interval in seconds.
    #[must_use]
    pub fn period_seconds(&self) -> f32 {
        self.period_ds as f32 / 10.0
    }

    /// Distance threshold in metres.
    #[must_use]
    pub fn distance_metres(&self) -> f32 {
        self.distance_dm as f32 / 10.0
    }

    /// Speed threshold in km/h.
    #[must_use]
    pub fn speed_kmh(&self) -> f32 {
        self.speed_dkh as f32 / 10.0
    }

    /// The data mask with the Holux "low precision" marker removed.
    #[must_use]
    pub fn effective_bitmask(&self) -> u32 {
        self.bitmask & !HOLUX_EXTRA_BIT
    }

    /// Absolute offset of the first record in this block.
    #[must_use]
    pub fn data_offset(&self) -> usize {
        self.offset.saturating_add(BLOCK_HEADER_SIZE)
    }
}

/// A little-endian `u16` at `at` inside an already length-checked header.
fn u16_at(fields: &[u8], at: usize, offset: usize) -> Result<u16> {
    let bytes = fields.get(at..at + 2).ok_or_else(|| Error::Log {
        message: format!("block header at 0x{offset:X} has no field at +{at}"),
        offset: Some(offset + at),
    })?;
    let array: [u8; 2] = bytes.try_into().map_err(|_| Error::Log {
        message: String::from("block header field is not two bytes"),
        offset: Some(offset + at),
    })?;
    Ok(u16::from_le_bytes(array))
}

fn u32_le(fields: &[u8], at: usize, offset: usize) -> Result<u32> {
    let bytes = fields.get(at..at + 4).ok_or_else(|| Error::Log {
        message: format!("block header at 0x{offset:X} has no field at +{at}"),
        offset: Some(offset + at),
    })?;
    let array: [u8; 4] = bytes.try_into().map_err(|_| Error::Log {
        message: String::from("block header field is not four bytes"),
        offset: Some(offset + at),
    })?;
    Ok(u32::from_le_bytes(array))
}

/// One field inside one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// The bit in the block's bitmask that selected this field.
    pub bit: u8,
    /// Offset from the start of the record.
    pub offset: usize,
    /// Length in bytes.
    pub size: usize,
}

/// The fields a record with `bitmask` carries, in the order they are stored.
///
/// `record` supplies the bytes needed to size the variable satellite block; when
/// it is shorter than the satellite count field, the worst case of
/// [`MAX_SATELLITES`] slots is assumed so a truncated record still yields a
/// length rather than a panic.
#[must_use]
pub fn field_layout(bitmask: u32, record: &[u8]) -> Vec<Field> {
    let mask = bitmask & !HOLUX_EXTRA_BIT;
    let mut fields = Vec::new();
    let mut offset = 0usize;
    for (index, size) in FIELD_SIZES.iter().enumerate() {
        let Some(bit) = u8::try_from(index).ok() else {
            continue;
        };
        if mask & (1u32 << index) == 0 {
            continue;
        }
        let taken = match size {
            Some(fixed) => *fixed,
            None if bit == bit::SID => {
                let count = satellite_count(record, offset);
                count.saturating_mul(SATELLITE_SLOT_BYTES + satellite_detail_bytes(mask))
            }
            // ELEVATION / AZIMUTH / SNR live inside the SID block; when SID is
            // absent there is nowhere to read them from, so they contribute
            // nothing rather than inventing a width.
            None => 0,
        };
        if taken == 0 && size.is_none() && bit != bit::SID {
            continue;
        }
        fields.push(Field {
            bit,
            offset,
            size: taken,
        });
        offset = offset.saturating_add(taken);
    }
    fields
}

/// Bytes of per-satellite detail (elevation, azimuth, SNR) the mask asks for.
fn satellite_detail_bytes(mask: u32) -> usize {
    [bit::ELEVATION, bit::AZIMUTH, bit::SNR]
        .iter()
        .filter(|detail| mask & (1u32 << u32::from(**detail)) != 0)
        .count()
        * SATELLITE_DETAIL_FIELD_BYTES
}

/// The satellite count stored at the start of the SID block, clamped to the
/// format's maximum.
///
/// The count is the `u16` at offset 2 of the SID block. Reading it from the wrong
/// place multiplies the record length by up to 32 and loses the rest of the
/// block, so when it cannot be read the worst case is assumed.
fn satellite_count(record: &[u8], sid_offset: usize) -> usize {
    let wanted = sid_offset.saturating_add(2);
    let bytes = match record.get(wanted..wanted.saturating_add(2)) {
        Some(bytes) => bytes,
        None => return MAX_SATELLITES,
    };
    let Ok(array): std::result::Result<[u8; 2], _> = bytes.try_into() else {
        return MAX_SATELLITES;
    };
    usize::from(u16::from_le_bytes(array)).min(MAX_SATELLITES)
}

/// Record length for a bitmask that contains no satellite block.
///
/// Returns `None` when the mask asks for the variable satellite block: that
/// length is a property of each record, not of the mask.
#[must_use]
pub fn fixed_record_length(bitmask: u32) -> Option<usize> {
    let mask = bitmask & !HOLUX_EXTRA_BIT;
    if mask == 0 {
        return Some(0);
    }
    if mask & (1u32 << u32::from(bit::SID)) != 0 {
        return None;
    }
    let mut total = CHECKSUM_BYTES;
    for (index, size) in FIELD_SIZES.iter().enumerate() {
        if mask & (1u32 << index) == 0 {
            continue;
        }
        // A satellite-only field with no SID block contributes nothing, which
        // matches `field_layout`.
        total = total.saturating_add(size.unwrap_or(0));
    }
    Some(total)
}

/// Record length including the one-byte checksum.
#[must_use]
pub fn record_length(bitmask: u32, record: &[u8]) -> usize {
    match fixed_record_length(bitmask) {
        Some(length) => length,
        None => field_layout(bitmask, record)
            .last()
            .map_or(CHECKSUM_BYTES, |field| {
                field.offset.saturating_add(field.size) + CHECKSUM_BYTES
            }),
    }
}

/// XOR of every byte of `body`.
#[must_use]
pub fn xor_checksum(body: &[u8]) -> u8 {
    body.iter().fold(0u8, |acc, byte| acc ^ byte)
}

/// Whether the trailer of `record` matches the XOR of its field bytes.
///
/// `record` is the whole record; [`record_length`] must fit inside it.
#[must_use]
pub fn checksum_matches(record: &[u8], bitmask: u32) -> bool {
    checksum_matches_length(record, record_length(bitmask, record))
}

/// Whether the trailer of a `length`-byte record matches the XOR of its fields.
///
/// This is the same check as [`checksum_matches`], for a caller that has
/// detected the record length from the data because the block header's mask
/// disagrees with it. A length below the one trailer byte, or one that does not
/// fit inside `record`, is not a valid record.
#[must_use]
pub fn checksum_matches_length(record: &[u8], length: usize) -> bool {
    if length < CHECKSUM_BYTES {
        return false;
    }
    let body_length = length - CHECKSUM_BYTES;
    let (Some(body), Some(trailer)) = (record.get(..body_length), record.get(body_length)) else {
        return false;
    };
    xor_checksum(body) == *trailer
}

/// Whether `bytes` is the `0xFF` padding that ends a block's used area.
///
/// Four bytes are checked, not one. A record may legitimately begin with a single
/// `0xFF` byte — a UTC low byte of `0xFF` is a 2106 timestamp, and a waypoint's
/// `RCR` can be `0xFF` — so stopping at the first `0xFF` would truncate a real
/// record. Four in a row is what the format leaves behind.
#[must_use]
pub fn is_padding(bytes: &[u8]) -> bool {
    let head = bytes.get(..4).unwrap_or(bytes);
    !head.is_empty() && head.iter().all(|byte| *byte == 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured V1.13 default: UTC + lat + lon + height, padded with the
    /// Holux low-precision marker.
    const V113_BITMASK: u32 = 0x8000_001D;

    /// The 20-byte default: the same plus a float speed.
    const WITH_SPEED: u32 = 0x8000_003D;

    fn header_bytes(count: u16, bitmask: u32, policy: u16) -> Vec<u8> {
        let mut data = vec![0xFFu8; BLOCK_HEADER_SIZE];
        data[0..2].copy_from_slice(&count.to_le_bytes());
        data[2..6].copy_from_slice(&bitmask.to_le_bytes());
        data[6..8].copy_from_slice(&policy.to_le_bytes());
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data
    }

    #[test]
    fn the_measured_default_bitmask_is_a_sixteen_byte_record() {
        // Lat/lon are floats and height is three bytes: 4+4+4+3+1 = 16.
        assert_eq!(fixed_record_length(V113_BITMASK), Some(16));
    }

    #[test]
    fn the_speed_bit_adds_four_bytes_for_a_twenty_byte_record() {
        assert_eq!(fixed_record_length(WITH_SPEED), Some(20));
    }

    #[test]
    fn the_holux_extra_bit_is_not_a_field() {
        // Without the mask, bit 31 would be an unknown field and the 16-byte
        // record would not be recognised.
        assert_eq!(fixed_record_length(0x8000_001D), Some(16));
        assert_eq!(fixed_record_length(0x0000_001D), Some(16));
    }

    #[test]
    fn a_record_with_no_fields_has_only_its_checksum() {
        assert_eq!(fixed_record_length(0), Some(0));
    }

    #[test]
    fn the_field_layout_puts_utc_first_and_height_last() {
        let layout = field_layout(V113_BITMASK, &[]);
        let bits: Vec<u8> = layout.iter().map(|field| field.bit).collect();
        assert_eq!(
            bits,
            vec![bit::UTC, bit::LATITUDE, bit::LONGITUDE, bit::HEIGHT]
        );
        let offsets: Vec<usize> = layout.iter().map(|field| field.offset).collect();
        assert_eq!(offsets, vec![0, 4, 8, 12]);
        let sizes: Vec<usize> = layout.iter().map(|field| field.size).collect();
        assert_eq!(sizes, vec![4, 4, 4, 3]);
    }

    #[test]
    fn a_satellite_block_is_sized_from_its_own_count() {
        // SID + NSAT: the satellite block is 4 bytes per slot plus 2 bytes per
        // requested detail, times the count the record carries.
        let mask = (1u32 << u32::from(bit::SID)) | (1u32 << u32::from(bit::SNR));
        let mut record = [0u8; 8];
        record[0..2].copy_from_slice(&1u16.to_le_bytes()); // pretend prefix
        record[2..4].copy_from_slice(&3u16.to_le_bytes()); // three satellites
        // `field_layout` reads the count at the SID block's offset, which is 0
        // here because no field precedes SID.
        record[2..4].copy_from_slice(&3u16.to_le_bytes());
        let length = record_length(mask, &record);
        // 3 slots x (4 + 2 SNR) = 18, plus the one-byte checksum.
        assert_eq!(length, 3 * 6 + CHECKSUM_BYTES);
    }

    #[test]
    fn a_truncated_satellite_record_falls_back_to_the_worst_case() {
        let mask = 1u32 << u32::from(bit::SID);
        // No bytes at all: 32 slots x 4 bytes + checksum.
        assert_eq!(
            record_length(mask, &[]),
            MAX_SATELLITES * 4 + CHECKSUM_BYTES
        );
    }

    #[test]
    fn a_header_decodes_the_documented_fields() {
        let data = header_bytes(7, V113_BITMASK, POLICY_LOG_ENABLED);
        let header = BlockHeader::parse(&data, 0x1_0000).expect("twenty bytes");
        assert_eq!(header.count, Some(7));
        assert_eq!(header.bitmask, V113_BITMASK);
        assert_eq!(header.effective_bitmask(), 0x1D);
        assert!(header.logging_enabled());
        assert_eq!(header.period_seconds(), 1.0);
        assert_eq!(header.data_offset(), 0x1_0200);
    }

    #[test]
    fn the_not_full_count_is_reported_as_unknown() {
        let data = header_bytes(COUNT_NOT_FULL, V113_BITMASK, POLICY_LOG_DISABLED);
        let header = BlockHeader::parse(&data, 0).expect("twenty bytes");
        assert_eq!(header.count, None);
        assert!(!header.logging_enabled());
    }

    #[test]
    fn a_short_header_is_an_error_not_a_panic() {
        assert!(BlockHeader::parse(&[0u8; HEADER_FIELDS_SIZE - 1], 0).is_err());
    }

    #[test]
    fn the_checksum_is_the_xor_of_the_field_bytes() {
        let mut body = vec![0x11u8; 15];
        body[0] = 0xAA;
        body[7] = 0x55;
        let trailer = xor_checksum(&body);
        body.push(trailer);
        assert!(checksum_matches(&body, V113_BITMASK));
        // Flip one field byte: the record must be marked bad.
        body[3] ^= 0xFF;
        assert!(!checksum_matches(&body, V113_BITMASK));
    }

    #[test]
    fn a_record_that_is_too_short_for_its_trailer_is_not_valid() {
        assert!(!checksum_matches(&[0u8; 15], V113_BITMASK));
    }

    #[test]
    fn padding_needs_four_ff_bytes_not_just_the_first() {
        assert!(is_padding(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00]));
        // A single 0xFF can be the first byte of a real record, so it is not
        // enough to call the area padding.
        assert!(!is_padding(&[0xFF, 0x00, 0x12, 0x34]));
        assert!(!is_padding(&[0x00]));
        assert!(!is_padding(&[]));
    }

    #[test]
    fn a_short_tail_of_ff_bytes_still_counts_as_padding() {
        // The last block of a dump can end before four padding bytes fit.
        assert!(is_padding(&[0xFF]));
        assert!(is_padding(&[0xFF, 0xFF]));
    }
}
