//! A single record in the M-241's binary track log.
//!
//! Records are not fixed-width: the block header's bitmask decides which fields
//! each record carries, and the fields appear in ascending bit order. This module
//! decodes one record for a given mask; [`crate::log::parse`] walks a whole dump
//! and [`crate::log::block`] owns the mask itself.
//!
//! Three M-241 differences from generic MTK loggers are handled here, because
//! each of them silently produces plausible garbage if it is got wrong:
//!
//! * latitude and longitude are **4-byte floats**, not doubles;
//! * height is a **3-byte u24**, the high bytes of a little-endian `f32`;
//! * the trailer is **one** checksum byte, with no `'*'` separator.

use crate::error::{Error, Result};
use crate::log::block::{self, bit, field_layout};
use crate::types::TrackPoint;

/// Smallest possible record: UTC + lat + lon + height + checksum.
pub const MIN_RECORD_BYTES: usize = 4 + 4 + 4 + 3 + 1;

/// Size of an M-241 record with the 20-byte default bitmask (UTC + lat + lon +
/// height + float speed + checksum).
pub const DEFAULT_RECORD_BYTES: usize = 20;

/// Bit positions, in ascending field order.
///
/// Re-exported from [`crate::log::block::bit`] rather than duplicated: a second
/// copy of a bit table is a second chance for the two to disagree.
pub mod bit_index {
    pub use crate::log::block::bit::*;
}

/// One decoded record, before the dump is stitched into a [`crate::types::Track`].
///
/// It keeps the fix-validity flags, which [`TrackPoint`] has no room for but the
/// decoder needs to decide whether a position exists at all.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    /// UTC as Unix seconds.
    pub utc: Option<u32>,
    /// Latitude in degrees, widened from the 4-byte float the device stores.
    pub latitude: Option<f64>,
    /// Longitude in degrees.
    pub longitude: Option<f64>,
    /// Height in metres, decoded from the 3-byte field.
    pub height_m: Option<f32>,
    /// Speed in km/h.
    pub speed_kmh: Option<f32>,
    /// Log reason bitmask; bit 3 marks a manual waypoint.
    pub rcr: Option<u16>,
    /// Whether the one-byte checksum matched.
    pub checksum_ok: bool,
}

impl LogRecord {
    /// Whether this record marks a waypoint (`RCR & 0x0008`).
    #[must_use]
    pub fn is_waypoint(&self) -> bool {
        self.rcr.is_some_and(|rcr| rcr & 0x0008 != 0)
    }

    /// The smallest record this format can produce, in bytes.
    ///
    /// A record shorter than this cannot carry a position, so a dump whose tail
    /// is shorter is a truncated final write, not a point.
    #[must_use]
    pub fn is_plausible_length(length: usize) -> bool {
        length >= MIN_RECORD_BYTES
    }

    /// Decode one complete record (fields **and** trailer) for `bitmask`.
    ///
    /// `record` may be longer than the record itself — the caller usually hands
    /// over the rest of the block — in which case the layout decides how much is
    /// read.
    ///
    /// # Errors
    ///
    /// [`Error::Log`] when the bytes are too short for the fields the mask
    /// selects, which means the record is truncated.
    pub fn decode(record: &[u8], bitmask: u32) -> Result<Self> {
        let layout = field_layout(bitmask, record);
        let mut decoded = Self {
            utc: None,
            latitude: None,
            longitude: None,
            height_m: None,
            speed_kmh: None,
            rcr: None,
            checksum_ok: block::checksum_matches(record, bitmask),
        };

        // The fix flags decide whether the position is real. MTK stores
        // `VALID == 0x0001` for "no fix", and a record with no position must not
        // claim one — reporting (0, 0) would place the track off West Africa.
        // A mask without VALID has no flag to contradict the coordinates, so the
        // position is trusted; that is how the measured 16-byte log behaves.
        let valid = field(record, &layout, bit::VALID)
            .and_then(read_u16)
            .unwrap_or(0);
        let has_fix = valid & 0x0001 == 0;

        for entry in &layout {
            let Some(bytes) = record.get(entry.offset..entry.offset.saturating_add(entry.size))
            else {
                return Err(Error::Log {
                    message: format!(
                        "record is truncated: bit {} needs {} bytes at +{}",
                        entry.bit, entry.size, entry.offset
                    ),
                    offset: Some(entry.offset),
                });
            };
            match entry.bit {
                bit::UTC => decoded.utc = read_u32(bytes),
                bit::LATITUDE => decoded.latitude = read_f32(bytes).map(f64::from),
                bit::LONGITUDE => decoded.longitude = read_f32(bytes).map(f64::from),
                bit::HEIGHT => decoded.height_m = Self::height_from_u24(bytes).ok(),
                bit::SPEED => decoded.speed_kmh = read_f32(bytes),
                bit::RCR => decoded.rcr = read_u16(bytes),
                _ => {}
            }
        }

        // The device stores 0 for "no timestamp"; reporting it as 1970 would put
        // a spurious point at the start of every track.
        if decoded.utc == Some(0) || decoded.utc == Some(u32::MAX) {
            decoded.utc = None;
        }
        if !has_fix {
            decoded.latitude = None;
            decoded.longitude = None;
        }
        Ok(decoded)
    }

    /// Convert to the shared [`TrackPoint`] the compiler and the interface use.
    #[must_use]
    pub fn to_track_point(&self) -> TrackPoint {
        TrackPoint {
            utc: self.utc,
            latitude: self.latitude,
            longitude: self.longitude,
            height_m: self.height_m,
            speed_kmh: self.speed_kmh,
            rcr: self.rcr,
        }
    }

    /// Decode a 3-byte height as the M-241 stores it.
    ///
    /// The three bytes are the *most significant* bytes of a little-endian
    /// float32, so the implicit zero low byte is prepended. Getting this wrong
    /// (reading the bytes as a `u24` number) yields heights in the tens of
    /// millions of metres rather than an error.
    ///
    /// # Errors
    ///
    /// [`Error::Log`] when fewer than three bytes are available.
    pub fn height_from_u24(bytes: &[u8]) -> Result<f32> {
        let top: [u8; 3] = bytes
            .get(..3)
            .and_then(|slice| slice.try_into().ok())
            .ok_or_else(|| Error::Log {
                message: format!("height needs 3 bytes, got {}", bytes.len()),
                offset: None,
            })?;
        // The three bytes are the *most significant* bytes of a little-endian
        // f32, so the implicit zero byte goes in front. Building the array
        // directly keeps the zero explicit and needs no mutable slicing.
        let value = [0u8, top[0], top[1], top[2]];
        Ok(f32::from_le_bytes(value))
    }
}

/// The bytes of one decoded field, if the layout includes it.
fn field<'a>(record: &'a [u8], layout: &[block::Field], wanted: u8) -> Option<&'a [u8]> {
    let entry = layout.iter().find(|entry| entry.bit == wanted)?;
    record.get(entry.offset..entry.offset.saturating_add(entry.size))
}

fn read_u16(bytes: &[u8]) -> Option<u16> {
    let array: [u8; 2] = bytes.get(..2)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

fn read_u32(bytes: &[u8]) -> Option<u32> {
    let array: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

fn read_f32(bytes: &[u8]) -> Option<f32> {
    let array: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(f32::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's example coordinate, stored as the `f32` the log holds.
    const LAT: f32 = 35.681_236;
    const LON: f32 = 139.767_13;

    /// A 16-byte V1.13-default record: UTC, lat, lon, height, checksum.
    fn sixteen_byte_record() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]); // 80.0 m
        let checksum = block::xor_checksum(&body);
        body.push(checksum);
        body
    }

    #[test]
    fn height_prepends_the_implicit_zero_byte() {
        // 0x42 0xA0 0x00 as the top three bytes of a little-endian float.
        let height = LogRecord::height_from_u24(&[0x00, 0xA0, 0x42]).expect("three bytes");
        assert_eq!(height, 80.0);
    }

    #[test]
    fn a_short_height_is_an_error_not_a_panic() {
        assert!(LogRecord::height_from_u24(&[0x00, 0xA0]).is_err());
    }

    #[test]
    fn rcr_bit_three_marks_a_waypoint() {
        let waypoint = LogRecord {
            utc: None,
            latitude: None,
            longitude: None,
            height_m: None,
            speed_kmh: None,
            rcr: Some(0x0008),
            checksum_ok: true,
        };
        assert!(waypoint.is_waypoint());
        assert!(
            !LogRecord {
                rcr: None,
                ..waypoint
            }
            .is_waypoint()
        );
    }

    #[test]
    fn a_sixteen_byte_record_decodes_its_four_fields() {
        let record = sixteen_byte_record();
        let decoded = LogRecord::decode(&record, 0x8000_001D).expect("16-byte record");
        assert_eq!(decoded.utc, Some(1_700_000_000));
        // The stored type is `f32`, so the comparison is against the same value
        // widened, not against the `f64` the example coordinate is written as.
        assert_eq!(decoded.latitude, Some(f64::from(LAT)));
        assert_eq!(decoded.longitude, Some(f64::from(LON)));
        assert_eq!(decoded.height_m, Some(80.0));
        assert_eq!(decoded.speed_kmh, None);
        assert!(decoded.checksum_ok);
    }

    #[test]
    fn the_speed_field_turns_the_same_record_into_a_twenty_byte_one() {
        let mut body = Vec::new();
        body.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        body.extend_from_slice(&42.5f32.to_le_bytes());
        let checksum = block::xor_checksum(&body);
        body.push(checksum);

        let record = LogRecord::decode(&body, 0x8000_003D).expect("20-byte record");
        assert_eq!(record.speed_kmh, Some(42.5));
        assert_eq!(record.height_m, Some(80.0));
    }

    #[test]
    fn a_corrupt_checksum_is_reported_not_hidden() {
        let mut record = sixteen_byte_record();
        record[3] ^= 0xFF;
        let decoded = LogRecord::decode(&record, 0x8000_001D).expect("still decodes");
        assert!(!decoded.checksum_ok);
    }

    #[test]
    fn the_no_fix_placeholder_has_no_position() {
        // MTK stores VALID == 0x0001 for "no fix", with the position fields
        // zeros. Reporting (0, 0) would put a track point in the Gulf of Guinea.
        let bitmask = 0x8000_001F; // UTC | VALID | LAT | LON | HEIGHT
        let mut body = Vec::new();
        body.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&0f32.to_le_bytes());
        body.extend_from_slice(&0f32.to_le_bytes());
        body.extend_from_slice(&[0x00, 0x00, 0x00]);
        let checksum = block::xor_checksum(&body);
        body.push(checksum);

        let record = LogRecord::decode(&body, bitmask).expect("record");
        assert_eq!(record.utc, Some(1_700_000_000));
        assert_eq!(record.latitude, None);
        assert_eq!(record.longitude, None);
        assert!(!record.to_track_point().has_position());
    }

    #[test]
    fn a_zero_timestamp_is_absence_not_1970() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        let checksum = block::xor_checksum(&body);
        body.push(checksum);

        let record = LogRecord::decode(&body, 0x8000_001D).expect("record");
        assert_eq!(record.utc, None);
    }

    #[test]
    fn a_truncated_record_is_an_error() {
        let record = sixteen_byte_record();
        let truncated = &record[..8];
        assert!(LogRecord::decode(truncated, 0x8000_001D).is_err());
    }

    #[test]
    fn a_plausible_length_starts_at_the_minimum_size() {
        assert!(!LogRecord::is_plausible_length(MIN_RECORD_BYTES - 1));
        assert!(LogRecord::is_plausible_length(MIN_RECORD_BYTES));
        assert_eq!(MIN_RECORD_BYTES, 16);
        assert_eq!(DEFAULT_RECORD_BYTES, 20);
    }
}
