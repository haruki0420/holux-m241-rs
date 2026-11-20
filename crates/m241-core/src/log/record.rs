//! Decoding one record of the M-241's binary track log.
//!
//! The recorder writes a bitmask-driven format into 64 KiB blocks. A record is
//! the concatenation of the fields whose bit is set, in ascending bit order,
//! followed by a one-byte checksum with **no** `'*'` separator.
//!
//! The M-241 differs from generic MTK loggers in three ways that each cost real
//! debugging time: latitude and longitude are 4-byte floats (not doubles),
//! height is a 3-byte `u24` (not a float32), and the trailer is one byte.
//!
//! This module holds the decoding of those fields and nothing else. A decoded
//! point is a [`crate::types::TrackPoint`], the same type the CLI and the GUI
//! pass around, so a record does not grow a second point-shaped struct that
//! drifts from it.

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
    // The three bytes are the *most significant* bytes of a little-endian f32,
    // so the implicit zero byte goes in front. Building the array directly
    // keeps the zero explicit and needs no mutable slicing.
    let value = [0u8, top[0], top[1], top[2]];
    Ok(f32::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_prepends_the_implicit_zero_byte() {
        // 0x42 0xA0 0x00 as the top three bytes of a little-endian float.
        let height = height_from_u24(&[0x00, 0xA0, 0x42]).expect("three bytes");
        assert_eq!(height, 80.0);
    }

    #[test]
    fn a_short_height_is_an_error_not_a_panic() {
        assert!(height_from_u24(&[0x00, 0xA0]).is_err());
    }
}
