//! A single record in the M-241's binary track log.
//!
//! The recorder writes a bitmask-driven format into 64 KiB blocks. A record is
//! the concatenation of the fields whose bit is set, in ascending bit order,
//! followed by a one-byte checksum with **no** `'*'` separator.
//!
//! The M-241 differs from generic MTK loggers in three ways that each cost real
//! debugging time: latitude and longitude are 4-byte floats (not doubles),
//! height is a 3-byte `u24` (not a float32), and the trailer is one byte.

use crate::error::{Error, Result};

/// Smallest possible record: UTC + lat + lon + height + checksum.
pub const MIN_RECORD_BYTES: usize = 4 + 4 + 4 + 3 + 1;

/// Size of an M-241 record with the 20-byte default bitmask.
pub const DEFAULT_RECORD_BYTES: usize = 20;

/// Bit positions, in ascending field order.
pub mod bit {
    /// UTC as Unix seconds (u32).
    pub const UTC: u8 = 0;
    /// Fix validity flags (u16).
    pub const VALID: u8 = 1;
    /// Latitude as a little-endian `f32`.
    pub const LATITUDE: u8 = 2;
    /// Longitude as a little-endian `f32`.
    pub const LONGITUDE: u8 = 3;
    /// Height as a 3-byte little-endian `u24`.
    pub const HEIGHT: u8 = 4;
    /// Speed in km/h as an `f32`.
    pub const SPEED: u8 = 5;
    /// Heading as an `f32`.
    pub const HEADING: u8 = 6;
    /// Log reason bitmask; bit 3 marks a waypoint.
    pub const RCR: u8 = 17;
}

/// One decoded track point.
///
/// Fields are `Option` because the bitmask decides which are present, and
/// because a truncated dump must still yield the fields it did contain.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    /// UTC as Unix seconds.
    pub utc: Option<u32>,
    /// Latitude in degrees (4-byte float in the source, widened here).
    pub latitude: Option<f64>,
    /// Longitude in degrees.
    pub longitude: Option<f64>,
    /// Height in metres, from the 3-byte field.
    pub height_m: Option<f32>,
    /// Speed in km/h.
    pub speed_kmh: Option<f32>,
    /// Log reason bitmask.
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

    /// Decode a 3-byte height as the M-241 stores it.
    ///
    /// The three bytes are the *most significant* bytes of a little-endian
    /// float32, so the implicit zero low byte is prepended.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
