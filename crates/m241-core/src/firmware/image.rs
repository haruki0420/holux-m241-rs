//! The MTK/Holux firmware header and the GPS epoch constant it carries.
//!
//! The M-241 image is a Thumb binary loaded at `0x0020_0000`. Its first `0x200`
//! bytes are an MTK header block whose layout is identical across V1.12 and
//! V1.13, so one parser covers both. No firmware bytes are ever stored in this
//! crate: every function works on a user-supplied image.

use crate::error::{Error, Result};

/// Unix seconds at the GPS epoch, 1980-01-06 UTC (`0x12D5_3D80`).
pub const GPS_EPOCH_UNIX: u32 = 315_964_800;

/// The value the constant must become to absorb the second rollover.
///
/// `315_964_800 + 619_315_200`, where `619_315_200 = 1024 weeks × 604_800 s`.
pub const GPS_EPOCH_UNIX_WNRO: u32 = 935_280_000;

/// The shift in seconds: exactly one 1024-week cycle.
pub const WNRO_SHIFT_SECONDS: u32 = 619_315_200;

/// Both places the constant appears. Only the second is effective; patching both
/// double-shifts the clock.
pub const EPOCH_CONSTANT_OFFSETS: [usize; 2] = [0x402DC, 0x402F4];

/// The effective occurrence: the one the NMEA and log paths actually read.
pub const EFFECTIVE_EPOCH_OFFSET: usize = 0x402F4;

pub const HEADER_SIZE: usize = 0x200;
pub const FAMILY_OFFSET: usize = 0x90;
pub const FAMILY_SIZE: usize = 0x20;
pub const FW_SIZE_OFFSET: usize = 0xF4;
pub const BUILD_NUMBER_OFFSET: usize = 0x108;
pub const RELEASE_NAME_OFFSET: usize = 0x148;
pub const DEVICE_TYPE_OFFSET: usize = 0x1B0;
pub const DEVICE_TYPE_SIZE: usize = 0x10;

/// The family string every M-241 image carries.
pub const FAMILY: &str = "HOLUX_M-241";

/// A firmware image with its header decoded on demand.
///
/// The bytes are borrowed, not copied: a dump is ~400 KB and is passed around by
/// reference through the CLI, the patch plan and the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareImage<'a> {
    data: &'a [u8],
}

impl<'a> FirmwareImage<'a> {
    /// Wrap an image, checking only that it is long enough to hold a header.
    ///
    /// # Errors
    ///
    /// [`Error::Firmware`] when the buffer is shorter than [`HEADER_SIZE`].
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(Error::Firmware {
                message: format!(
                    "image is {} bytes; a firmware image is at least {HEADER_SIZE}",
                    data.len()
                ),
                offset: None,
            });
        }
        Ok(Self { data })
    }

    /// The raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.data
    }

    /// Image length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false: [`Self::new`] rejects an empty buffer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// A NUL-trimmed ASCII header field.
    ///
    /// Returns an empty string when the field is not valid UTF-8 rather than
    /// failing: a corrupt header is reported by the caller comparing against
    /// [`FAMILY`], and an error here would hide which field was wrong.
    #[must_use]
    pub fn field(&self, offset: usize, size: usize) -> String {
        let end = offset.saturating_add(size).min(self.data.len());
        let slice = self.data.get(offset..end).unwrap_or(&[]);
        let trimmed = slice.split(|b| *b == 0).next().unwrap_or(&[]);
        String::from_utf8_lossy(trimmed).into_owned()
    }

    /// The firmware family string (`HOLUX_M-241` on every known image).
    #[must_use]
    pub fn family(&self) -> String {
        self.field(FAMILY_OFFSET, FAMILY_SIZE)
    }

    /// Whether this looks like an M-241 image at all.
    #[must_use]
    pub fn is_m241(&self) -> bool {
        self.family() == FAMILY
    }

    /// The device type string, e.g. `01017-00E` (V1.12) or `01017-00F` (V1.13).
    #[must_use]
    pub fn device_type(&self) -> String {
        self.field(DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE)
    }

    /// The little-endian `u16` build number at `0x108` (51 and 67 on the two
    /// known releases).
    ///
    /// # Errors
    ///
    /// Never: the buffer is at least [`HEADER_SIZE`] long, so the two bytes are
    /// present. Returns `None` only if that invariant is ever broken.
    #[must_use]
    pub fn build_number(&self) -> Option<u16> {
        let bytes = self
            .data
            .get(BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2)?;
        let array: [u8; 2] = bytes.try_into().ok()?;
        Some(u16::from_le_bytes(array))
    }

    /// Read the little-endian `u32` at `offset`, or `None` if it does not fit.
    #[must_use]
    pub fn u32_at(&self, offset: usize) -> Option<u32> {
        let bytes = self.data.get(offset..offset + 4)?;
        let array: [u8; 4] = bytes.try_into().ok()?;
        Some(u32::from_le_bytes(array))
    }

    /// The offsets that still hold the **stock** GPS epoch constant.
    ///
    /// Only occurrences whose value is still [`GPS_EPOCH_UNIX`] are returned, so
    /// a patched image reports none. That is deliberate — the patch enumerates
    /// what it would change — but it means an empty result must not be read as
    /// "this image carries no epoch constant". Use [`Self::effective_epoch`] and
    /// [`Self::is_patched`] to ask what the constant currently is.
    #[must_use]
    pub fn stock_epoch_offsets(&self) -> Vec<usize> {
        EPOCH_CONSTANT_OFFSETS
            .into_iter()
            .filter(|offset| self.u32_at(*offset) == Some(GPS_EPOCH_UNIX))
            .collect()
    }

    /// The effective epoch constant, which is what the WNRO patch rewrites.
    #[must_use]
    pub fn effective_epoch(&self) -> Option<u32> {
        self.u32_at(EFFECTIVE_EPOCH_OFFSET)
    }

    /// Whether the effective constant already carries the WNRO shift.
    #[must_use]
    pub fn is_patched(&self) -> bool {
        self.effective_epoch() == Some(GPS_EPOCH_UNIX_WNRO)
    }
}

/// The GPS epoch a firmware image is currently using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpsEpoch {
    /// The stock 2006 constant: every timestamp is 7168 days in the past.
    Stock,
    /// The constant has been shifted by one rollover.
    Wnro,
    /// Something else, which this crate does not understand.
    Unexpected(u32),
}

impl GpsEpoch {
    /// Classify a raw constant.
    #[must_use]
    pub fn from_constant(value: u32) -> Self {
        match value {
            GPS_EPOCH_UNIX => Self::Stock,
            GPS_EPOCH_UNIX_WNRO => Self::Wnro,
            other => Self::Unexpected(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Big enough to hold the effective epoch offset at `0x402F4`, which is the
    /// whole point of these tests: a real image is 406,620 bytes.
    const TEST_IMAGE_LEN: usize = 0x60000;

    fn image_with(epoch: u32) -> Vec<u8> {
        let mut data = vec![0u8; TEST_IMAGE_LEN];
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + 9].copy_from_slice(b"01017-00F");
        data[BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2].copy_from_slice(&67u16.to_le_bytes());
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&epoch.to_le_bytes());
        }
        data
    }

    #[test]
    fn a_header_shorter_than_0x200_is_refused() {
        assert!(FirmwareImage::new(&[0u8; 0x100]).is_err());
    }

    #[test]
    fn header_fields_decode_from_the_documented_offsets() {
        let data = image_with(GPS_EPOCH_UNIX);
        let image = FirmwareImage::new(&data).expect("valid header");
        assert_eq!(image.family(), FAMILY);
        assert!(image.is_m241());
        assert_eq!(image.device_type(), "01017-00F");
        assert_eq!(image.build_number(), Some(67));
    }

    #[test]
    fn both_epoch_occurrences_are_reported_when_the_image_is_stock() {
        let data = image_with(GPS_EPOCH_UNIX);
        let image = FirmwareImage::new(&data).expect("valid header");
        assert_eq!(image.stock_epoch_offsets(), vec![0x402DC, 0x402F4]);
    }

    #[test]
    fn a_patched_image_reports_no_stock_epoch_offsets() {
        // The list is what the patch would change, so a patched image has none.
        // It must not be read as "no constant in this image": the constant is
        // there, carrying the WNRO value.
        let data = image_with(GPS_EPOCH_UNIX_WNRO);
        let image = FirmwareImage::new(&data).expect("valid header");
        assert!(image.stock_epoch_offsets().is_empty());
        assert_eq!(image.effective_epoch(), Some(GPS_EPOCH_UNIX_WNRO));
        assert!(image.is_patched());
    }

    #[test]
    fn only_the_effective_offset_counts_for_epoch_classification() {
        let mut data = image_with(GPS_EPOCH_UNIX);
        // Corrupt the ineffective copy only: the image is still stock.
        data[0x402DC..0x402DC + 4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let image = FirmwareImage::new(&data).expect("valid header");
        assert_eq!(image.effective_epoch(), Some(GPS_EPOCH_UNIX));
        assert!(!image.is_patched());
        assert_eq!(GpsEpoch::from_constant(GPS_EPOCH_UNIX), GpsEpoch::Stock);
    }

    #[test]
    fn a_patched_image_is_recognised() {
        let data = image_with(GPS_EPOCH_UNIX_WNRO);
        let image = FirmwareImage::new(&data).expect("valid header");
        assert!(image.is_patched());
    }

    #[test]
    fn the_shift_is_exactly_1024_weeks() {
        assert_eq!(WNRO_SHIFT_SECONDS, 1024 * 604_800);
        assert_eq!(GPS_EPOCH_UNIX + WNRO_SHIFT_SECONDS, GPS_EPOCH_UNIX_WNRO);
    }

    #[test]
    fn an_unexpected_epoch_is_kept_verbatim() {
        assert_eq!(GpsEpoch::from_constant(1), GpsEpoch::Unexpected(1));
    }
}
