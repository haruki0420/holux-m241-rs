//! The week-number-rollover patch: plan it, apply it, never guess.
//!
//! The M-241's firmware hard-codes the Unix time of the GPS epoch
//! (`315_964_800`, 1980-01-06) and never accounts for the second week-number
//! rollover (2019-04-06). Rewriting the *effective* copy of that constant by one
//! 1024-week cycle (`+619_315_200`) moves every timestamp the firmware produces
//! — the NMEA dates, the OLED clock and the binary log share the conversion — back
//! to the present.
//!
//! # The two offsets, and why only one is written
//!
//! The constant appears twice, at `0x402DC` and `0x402F4`. Only `0x402F4` is read
//! by the code paths that convert time; `0x402DC` is an unused copy. Rewriting
//! both shifts the clock by **two** cycles, so this module reads the unused copy
//! and refuses to proceed if it is not the stock value: that is how "this image
//! was edited by something else" is caught before it becomes a wrong date on the
//! user's unit.
//!
//! # What this module does not do
//!
//! It never writes to a file or a device. It produces a [`PatchPlan`], and
//! [`PatchPlan::apply`] produces a new byte vector. Persisting that vector is the
//! caller's decision, which is what keeps `--dry-run` and the real run on exactly
//! the same code path.

use crate::error::{Error, Result};
use crate::firmware::image::{
    FW_SIZE_OFFSET, FirmwareImage, GPS_EPOCH_UNIX, GPS_EPOCH_UNIX_WNRO, WNRO_SHIFT_SECONDS,
};
use crate::firmware::profiles::{self, VERSION_MARKER_LEN, patched_marker};
use crate::types::{FirmwareProfile, PatchEdit, PatchPlan};

/// Offset of the constant the firmware actually reads.
///
/// Measured: patching this one fixes the date; patching `0x402DC` as well
/// double-shifts it. Recorded in `docs/dev/03-hardware-facts.md`.
pub const EFFECTIVE_EPOCH_OFFSET: usize = 0x402F4;

/// The unused copy of the same constant.
///
/// Read, never written. Its value is a second opinion on whether the image has
/// been touched: an image whose unused copy is also shifted was changed by
/// something that did not follow this module's measured procedure.
pub const INACTIVE_EPOCH_OFFSET: usize = 0x402DC;

/// Where the version marker's final digit sits, for each profile.
///
/// Only that digit changes (`V1.12` → `V1.14`, `V1.13` → `V1.14`), so a patched
/// unit reports its state on the OLED without opening the case. Measured on
/// hardware: the changed byte is `0x59C2C` on V1.13.
#[must_use]
pub fn version_digit_offset(profile: &FirmwareProfile) -> usize {
    profile.version_marker + VERSION_MARKER_LEN - 1
}

/// Build the patch plan for `image`.
///
/// The plan holds at most three edits: the two bytes of the effective epoch
/// constant that actually change, and the final digit of the version marker. An
/// image that needs nothing yields an empty plan whose
/// [`PatchPlan::no_op_reason`] says why.
///
/// # Errors
///
/// * [`Error::Profile`] when the image is not a known release;
/// * [`Error::Firmware`] when the image is too short, when the effective epoch is
///   neither the stock nor the patched constant, or when the unused copy has been
///   changed. Each of those means the image is not what this patch was measured
///   against, and guessing would produce a unit that is wrong in a new way.
pub fn plan(image: &FirmwareImage) -> Result<PatchPlan> {
    let identification = profiles::identify(image)?;
    plan_for(image, identification.profile)
}

/// Build the patch plan against a named profile, skipping identification.
///
/// Useful when the caller has already identified the image and wants to say so,
/// and in tests that need the profile without 400 KB of fixture.
///
/// # Errors
///
/// As [`plan`], minus the identification failure.
pub fn plan_for(image: &FirmwareImage, profile: &'static FirmwareProfile) -> Result<PatchPlan> {
    if image.len() != profile.size {
        return Err(Error::Firmware {
            message: format!(
                "image is {} bytes; profile {} is {} bytes",
                image.len(),
                profile.name,
                profile.size
            ),
            offset: None,
        });
    }

    let effective = read_u32(image, EFFECTIVE_EPOCH_OFFSET)?;
    let already_patched = already_patched_reason(effective);
    if let Some(reason) = already_patched {
        return Ok(PatchPlan {
            edits: Vec::new(),
            profile: Some(*profile),
            no_op_reason: Some(reason),
        });
    }
    if effective != GPS_EPOCH_UNIX {
        return Err(Error::Firmware {
            message: format!(
                "the effective epoch constant at 0x{EFFECTIVE_EPOCH_OFFSET:X} is \
                 0x{effective:08X}; expected the stock 0x{GPS_EPOCH_UNIX:08X}"
            ),
            offset: Some(EFFECTIVE_EPOCH_OFFSET),
        });
    }

    // The unused copy is read as a second opinion, never patched. If it has
    // moved, something else edited this image and the measured behaviour of this
    // patch no longer applies to it.
    let inactive = read_u32(image, INACTIVE_EPOCH_OFFSET)?;
    if inactive != GPS_EPOCH_UNIX {
        return Err(Error::Firmware {
            message: format!(
                "the unused epoch copy at 0x{INACTIVE_EPOCH_OFFSET:X} is 0x{inactive:08X}, \
                 not the stock 0x{GPS_EPOCH_UNIX:08X}: this image was edited by something else"
            ),
            offset: Some(INACTIVE_EPOCH_OFFSET),
        });
    }

    let mut edits = epoch_edits(image)?;
    edits.extend(version_edit(image, profile)?);
    edits.sort_by_key(|edit| edit.offset);
    Ok(PatchPlan {
        edits,
        profile: Some(*profile),
        no_op_reason: None,
    })
}

/// The no-op reason when the effective constant already carries the shift.
fn already_patched_reason(effective: u32) -> Option<String> {
    if effective != GPS_EPOCH_UNIX_WNRO {
        return None;
    }
    Some(format!(
        "already patched: the effective epoch constant at 0x{EFFECTIVE_EPOCH_OFFSET:X} \
         is already 0x{GPS_EPOCH_UNIX_WNRO:08X}"
    ))
}

/// The byte-level difference between the stock and the shifted epoch constant.
fn epoch_edits(image: &FirmwareImage) -> Result<Vec<PatchEdit>> {
    let before = GPS_EPOCH_UNIX.to_le_bytes();
    let after = GPS_EPOCH_UNIX_WNRO.to_le_bytes();
    let mut edits = Vec::new();
    // Only the two high bytes differ (`80 3d d5 12` → `80 3d bf 37`), which is
    // why the hardware notes say "3 bytes change" for the whole patch.
    for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
        if old == new {
            continue;
        }
        let offset = EFFECTIVE_EPOCH_OFFSET + index;
        let actual = image
            .as_bytes()
            .get(offset)
            .copied()
            .ok_or_else(|| Error::Firmware {
                message: format!("the image ends before the epoch constant at 0x{offset:X}"),
                offset: Some(offset),
            })?;
        if actual != *old {
            return Err(Error::Firmware {
                message: format!(
                    "byte 0x{offset:X} is 0x{actual:02X}; the stock constant has 0x{old:02X} there"
                ),
                offset: Some(offset),
            });
        }
        edits.push(PatchEdit {
            offset,
            before: *old,
            after: *new,
        });
    }
    Ok(edits)
}

/// The single version-marker digit, when it still needs changing.
fn version_edit(
    image: &FirmwareImage,
    profile: &'static FirmwareProfile,
) -> Result<Vec<PatchEdit>> {
    let offset = version_digit_offset(profile);
    let actual = image
        .as_bytes()
        .get(offset)
        .copied()
        .ok_or_else(|| Error::Firmware {
            message: format!("the image ends before the version marker at 0x{offset:X}"),
            offset: Some(offset),
        })?;
    let Some(wanted) = patched_marker(profile.name).as_bytes().last().copied() else {
        return Err(Error::Profile {
            message: format!("profile {} has no version marker", profile.name),
            offset: Some(offset),
        });
    };
    if actual == wanted {
        // The epoch check above already returned for a fully patched image, so a
        // marker that is patched while the constant is stock means a half-edited
        // image. Guessing which half is right would be the wrong instinct.
        return Err(Error::Firmware {
            message: format!(
                "the version marker digit at 0x{offset:X} is already 0x{actual:02X} \
                 but the epoch constant is stock: this image is half patched"
            ),
            offset: Some(offset),
        });
    }
    if !actual.is_ascii_digit() {
        return Err(Error::Firmware {
            message: format!(
                "the version marker digit at 0x{offset:X} is 0x{actual:02X}, which is not a digit"
            ),
            offset: Some(offset),
        });
    }
    Ok(vec![PatchEdit {
        offset,
        before: actual,
        after: wanted,
    }])
}

/// Read a little-endian `u32` at `offset`, or explain why it is not there.
fn read_u32(image: &FirmwareImage, offset: usize) -> Result<u32> {
    image.u32_at(offset).ok_or(Error::Firmware {
        message: format!(
            "the image ends at {} bytes; 0x{offset:X} is outside it",
            image.len()
        ),
        offset: Some(offset),
    })
}

/// The header's declared image size, when it is plausible.
///
/// The field is written by the firmware build and is the only cross-check that a
/// dump was complete: a short read still looks like an image, but the header says
/// how long it should have been.
#[must_use]
pub fn declared_size(image: &FirmwareImage) -> Option<u32> {
    image.u32_at(FW_SIZE_OFFSET)
}

/// `true` when the image's header and length agree.
#[must_use]
pub fn is_complete_dump(image: &FirmwareImage) -> bool {
    declared_size(image).is_some_and(|declared| declared as usize == image.len())
}

/// The shift the patch applies, for a report line.
#[must_use]
pub fn shift_description() -> String {
    format!(
        "+{WNRO_SHIFT_SECONDS} s (+{} days)",
        WNRO_SHIFT_SECONDS / 86_400
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::image::{
        DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
        HEADER_SIZE,
    };
    use crate::firmware::profiles::{PROFILE_V1_12, PROFILE_V1_13};

    /// A structurally valid image for `profile` with the stock constants.
    fn stock_image(profile: &'static FirmwareProfile) -> Vec<u8> {
        let mut data = vec![0xFFu8; profile.size];
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        // A header field is a C string: the bytes after it are NUL, not 0xFF.
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
        let device_end = DEVICE_TYPE_OFFSET + device.len();
        data[DEVICE_TYPE_OFFSET..device_end].copy_from_slice(device);
        let marker = profile.name.as_bytes();
        let marker_end = profile.version_marker + marker.len();
        data[profile.version_marker..marker_end].copy_from_slice(marker);
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
        }
        data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4]
            .copy_from_slice(&(profile.size as u32).to_le_bytes());
        data
    }

    fn stock_v113() -> Vec<u8> {
        stock_image(&PROFILE_V1_13)
    }

    #[test]
    fn a_stock_image_plans_two_epoch_bytes_and_one_marker_digit() {
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        let plan = plan(&image).expect("stock image");
        let offsets: Vec<usize> = plan.edits.iter().map(|edit| edit.offset).collect();
        assert_eq!(offsets, vec![0x402F6, 0x402F7, 0x59C2C]);
        assert_eq!(
            plan.edits.len(),
            3,
            "the measured patch changes three bytes"
        );
        assert!(plan.changes_anything());
        assert_eq!(plan.profile, Some(PROFILE_V1_13));
        assert_eq!(plan.no_op_reason, None);
    }

    #[test]
    fn the_ineffective_epoch_copy_is_never_in_the_plan() {
        // Patching both copies doubles the shift; that is the failure this test
        // exists to prevent from ever coming back.
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        let plan = plan(&image).expect("stock image");
        assert!(
            plan.edits
                .iter()
                .all(|edit| edit.offset < INACTIVE_EPOCH_OFFSET
                    || edit.offset >= INACTIVE_EPOCH_OFFSET + 4),
            "an edit touches the unused epoch copy at 0x{INACTIVE_EPOCH_OFFSET:X}"
        );
    }

    #[test]
    fn applying_the_plan_moves_the_clock_forward_one_rollover() {
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        let plan = plan(&image).expect("stock image");
        let patched = plan.apply(&data).expect("edits fit");
        let patched_image = FirmwareImage::new(&patched).expect("header");
        assert_eq!(patched_image.effective_epoch(), Some(GPS_EPOCH_UNIX_WNRO));
        assert!(patched_image.is_patched());
        assert_eq!(
            patched_image.u32_at(INACTIVE_EPOCH_OFFSET),
            Some(GPS_EPOCH_UNIX),
            "the unused copy must stay stock"
        );
        assert_eq!(
            patched_image
                .effective_epoch()
                .map(|value| value - GPS_EPOCH_UNIX),
            Some(WNRO_SHIFT_SECONDS)
        );
    }

    #[test]
    fn the_patched_marker_reads_v1_14() {
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        let plan = plan(&image).expect("stock image");
        let patched = plan.apply(&data).expect("edits fit");
        let marker = FirmwareImage::new(&patched)
            .expect("header")
            .field(PROFILE_V1_13.version_marker, VERSION_MARKER_LEN);
        assert_eq!(marker, "V1.14");
    }

    #[test]
    fn an_already_patched_image_produces_an_empty_plan_and_says_why() {
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        let patched = plan(&image)
            .expect("stock image")
            .apply(&data)
            .expect("fit");
        let patched_image = FirmwareImage::new(&patched).expect("header");

        let empty = plan(&patched_image).expect("a patched image is still identifiable");
        assert!(!empty.changes_anything());
        assert_eq!(empty.profile, Some(PROFILE_V1_13));
        let reason = empty
            .no_op_reason
            .clone()
            .expect("a reason for doing nothing");
        assert!(reason.contains("already patched"), "got {reason}");
        // Applying an empty plan is a copy, not a refusal: `--dry-run` and a
        // repeated run must both work.
        assert_eq!(empty.apply(&patched).expect("no edits"), patched);
    }

    #[test]
    fn an_unexpected_epoch_constant_is_refused_not_reinterpreted() {
        let mut data = stock_v113();
        data[EFFECTIVE_EPOCH_OFFSET..EFFECTIVE_EPOCH_OFFSET + 4]
            .copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let image = FirmwareImage::new(&data).expect("header");
        let err = plan_for(&image, &PROFILE_V1_13).expect_err("unknown constant");
        assert!(matches!(err, Error::Firmware { .. }));
        let text = format!("{err}");
        assert!(text.contains("0xDEADBEEF"), "got {text}");
        assert!(text.contains("0x402F4"), "got {text}");
    }

    #[test]
    fn an_image_whose_unused_copy_moved_is_refused() {
        let mut data = stock_v113();
        data[INACTIVE_EPOCH_OFFSET..INACTIVE_EPOCH_OFFSET + 4]
            .copy_from_slice(&GPS_EPOCH_UNIX_WNRO.to_le_bytes());
        let image = FirmwareImage::new(&data).expect("header");
        let err = plan_for(&image, &PROFILE_V1_13).expect_err("edited image");
        let text = format!("{err}");
        assert!(text.contains("unused epoch copy"), "got {text}");
    }

    #[test]
    fn a_v1_12_image_is_patched_at_its_own_marker_offset() {
        let data = stock_image(&PROFILE_V1_12);
        let image = FirmwareImage::new(&data).expect("header");
        let plan = plan(&image).expect("stock V1.12");
        let marker_edit = plan
            .edits
            .iter()
            .find(|edit| edit.offset >= PROFILE_V1_12.version_marker)
            .expect("a marker edit");
        assert_eq!(marker_edit.offset, 0x59B14 + 4);
        assert_eq!(marker_edit.after, b'4');
        assert_eq!(plan.profile, Some(PROFILE_V1_12));
    }

    #[test]
    fn a_half_patched_marker_is_refused() {
        let mut data = stock_v113();
        let offset = version_digit_offset(&PROFILE_V1_13);
        data[offset] = b'4';
        let image = FirmwareImage::new(&data).expect("header");
        let err = plan_for(&image, &PROFILE_V1_13).expect_err("half patched");
        assert!(format!("{err}").contains("half patched"), "got {err}");
    }

    #[test]
    fn an_image_of_the_wrong_length_is_refused_before_it_is_read() {
        let data = stock_v113();
        let short = &data[..HEADER_SIZE + 4];
        let image = FirmwareImage::new(short).expect("header");
        let err = plan_for(&image, &PROFILE_V1_13).expect_err("length differs");
        assert!(format!("{err}").contains("406620"), "got {err}");
    }

    #[test]
    fn the_shift_is_one_thousand_and_twenty_four_weeks() {
        assert_eq!(WNRO_SHIFT_SECONDS, 1_024 * 604_800);
        assert_eq!(WNRO_SHIFT_SECONDS / 86_400, 7_168);
        assert!(shift_description().contains("7168 days"));
    }

    #[test]
    fn a_complete_dump_is_recognised_by_its_declared_size() {
        let data = stock_v113();
        let image = FirmwareImage::new(&data).expect("header");
        assert_eq!(declared_size(&image), Some(PROFILE_V1_13.size as u32));
        assert!(is_complete_dump(&image));
        let truncated = &data[..data.len() - 1];
        let image = FirmwareImage::new(truncated).expect("header");
        assert!(!is_complete_dump(&image));
    }
}
