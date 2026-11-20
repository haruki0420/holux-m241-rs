//! The firmware releases this build knows, and how an image is recognised.
//!
//! A profile is not a copy of the firmware: it is the measured facts that make
//! "which image is this" answerable without trusting a filename. Every field
//! here traces to `docs/dev/03-hardware-facts.md`.
//!
//! # Why a hash is not the only evidence
//!
//! The official release packages hash to the constants below, but a dump taken
//! from a *user's own unit* does not: the image contains per-unit runtime fields,
//! so its SHA-256 differs from the package hash. Identification therefore has two
//! levels — exact hash, then structure (size, device string, version marker) —
//! and the result says which one matched so a caller can report the difference
//! instead of hiding it.

use crate::error::{Error, Result};
use crate::firmware::digest::sha256_hex;
use crate::firmware::image::FirmwareImage;
use crate::types::FirmwareProfile;

/// Length of the ASCII version marker run (`V1.12`, `V1.13`, …).
pub const VERSION_MARKER_LEN: usize = 5;

/// The marker a patched image carries: the last digit becomes `4`.
///
/// Every patched unit reports `V1.14` whatever it started as, because the patch
/// only rewrites the final digit. Measured on hardware for V1.13; V1.12 uses the
/// same rule so a reader cannot tell the two apart from the marker alone, which
/// is why the size and device string still decide the profile.
pub const PATCHED_VERSION_DIGIT: u8 = b'4';

/// The version marker a patched `profile` would carry, e.g. `V1.14`.
#[must_use]
pub fn patched_marker(name: &str) -> String {
    let mut text = String::from(name);
    let _ = text.pop();
    text.push(char::from(PATCHED_VERSION_DIGIT));
    text
}

/// Holux M-241 firmware V1.12, as shipped in the official update package.
pub const PROFILE_V1_12: FirmwareProfile = FirmwareProfile {
    name: "V1.12",
    size: 406_312,
    sha256: "4103601a29f256ae5da771bbcbc3ca85a89deb6268ce74865dc84b2583ae98f7",
    version_marker: 0x59B14,
    device_type: "01017-00E",
};

/// Holux M-241 firmware V1.13, as shipped in the official update package.
pub const PROFILE_V1_13: FirmwareProfile = FirmwareProfile {
    name: "V1.13",
    size: 406_620,
    sha256: "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
    version_marker: 0x59C28,
    device_type: "01017-00F",
};

/// Every profile this build can identify, newest first.
pub const PROFILES: [FirmwareProfile; 2] = [PROFILE_V1_13, PROFILE_V1_12];

/// All known profiles.
#[must_use]
pub fn all() -> &'static [FirmwareProfile] {
    &PROFILES
}

/// Look a profile up by name, accepting `V1.13`, `v1.13` or `1.13`.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static FirmwareProfile> {
    let wanted = normalise_name(name);
    PROFILES
        .iter()
        .find(|profile| normalise_name(profile.name) == wanted)
}

fn normalise_name(name: &str) -> String {
    let trimmed = name.trim().to_ascii_uppercase();
    let without_v = trimmed.strip_prefix('V').unwrap_or(&trimmed);
    if without_v.starts_with('1') {
        format!("V{without_v}")
    } else {
        format!("V1.{without_v}")
    }
}

/// What made an image match a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The whole file hashes to the release package's digest: this is the
    /// official image, bit for bit.
    Sha256,
    /// Size, device type and version marker agree. A dump from a user's unit
    /// differs from the package in per-unit fields, so this is the normal case
    /// for a real backup.
    Structure,
}

/// An image's profile plus why it was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identification {
    /// The profile the image was identified as.
    pub profile: &'static FirmwareProfile,
    /// The evidence that matched.
    pub evidence: Evidence,
}

/// Identify a firmware image.
///
/// # Errors
///
/// [`Error::Profile`] when no profile matches. The message names the size, the
/// device string and the marker that were found, because those three are what a
/// person needs to decide whether a new profile should be added.
pub fn identify(image: &FirmwareImage) -> Result<Identification> {
    let digest = sha256_hex(image.as_bytes());
    for profile in PROFILES.iter() {
        if digest.eq_ignore_ascii_case(profile.sha256) {
            return Ok(Identification {
                profile,
                evidence: Evidence::Sha256,
            });
        }
    }

    let device = image.device_type();
    let size = image.len();
    for profile in PROFILES.iter() {
        if profile.size != size || profile.device_type != device {
            continue;
        }
        let marker = image.field(profile.version_marker, VERSION_MARKER_LEN);
        // A patched image carries `V1.14`, which matches no profile's own name.
        // The size and device string still identify it, and the epoch constant
        // (which the patch planner inspects) says whether it is already fixed.
        if marker == profile.name || marker == patched_marker(profile.name) {
            return Ok(Identification {
                profile,
                evidence: Evidence::Structure,
            });
        }
    }

    Err(Error::Profile {
        message: format!(
            "no profile matches this image ({} bytes, device {device:?}, sha256 {digest})",
            image.len()
        ),
        offset: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::image::{
        BUILD_NUMBER_OFFSET, DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY,
        FAMILY_OFFSET, GPS_EPOCH_UNIX,
    };

    /// The digest of a stock V1.13 image, recomputed here so the constant in the
    /// profile cannot drift without a failing test.
    const OFFICIAL_V1_13_SHA: &str =
        "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435";

    /// Build an image that *structurally* looks like `profile` without being the
    /// official bytes: this is what a user's own dump looks like.
    fn synthetic(profile: &FirmwareProfile, version: &str) -> Vec<u8> {
        let mut data = vec![0xFFu8; profile.size];
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        // A header field is a C string: the bytes after it are NUL, not 0xFF,
        // and `FirmwareImage::field` stops at the first NUL.
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
        let end = DEVICE_TYPE_OFFSET + device.len();
        data[DEVICE_TYPE_OFFSET..end].copy_from_slice(device);
        let marker = version.as_bytes();
        let end = profile.version_marker + marker.len();
        data[profile.version_marker..end].copy_from_slice(marker);
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
        }
        data[BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2].copy_from_slice(&67u16.to_le_bytes());
        data
    }

    #[test]
    fn the_profile_table_carries_the_measured_facts() {
        assert_eq!(PROFILE_V1_13.size, 406_620);
        assert_eq!(PROFILE_V1_12.size, 406_312);
        assert_eq!(PROFILE_V1_13.version_marker, 0x59C28);
        assert_eq!(PROFILE_V1_12.version_marker, 0x59B14);
        assert_eq!(PROFILE_V1_13.device_type, "01017-00F");
        assert_eq!(PROFILE_V1_12.device_type, "01017-00E");
        assert_eq!(PROFILE_V1_13.sha256, OFFICIAL_V1_13_SHA);
        assert_eq!(PROFILE_V1_13.sha256.len(), 64);
        assert_eq!(PROFILE_V1_12.sha256.len(), 64);
    }

    #[test]
    fn a_per_unit_dump_is_identified_by_structure_not_by_hash() {
        // The documented fact: the measured dump's SHA-256 differs per unit, so
        // hash-only identification would reject every real backup.
        let data = synthetic(&PROFILE_V1_13, "V1.13");
        let image = FirmwareImage::new(&data).expect("header");
        let found = identify(&image).expect("structure matches");
        assert_eq!(found.profile, &PROFILE_V1_13);
        assert_eq!(found.evidence, Evidence::Structure);
    }

    #[test]
    fn a_v1_12_image_is_not_confused_with_v1_13() {
        let data = synthetic(&PROFILE_V1_12, "V1.12");
        let image = FirmwareImage::new(&data).expect("header");
        let found = identify(&image).expect("structure matches");
        assert_eq!(found.profile, &PROFILE_V1_12);
        assert_eq!(found.profile.size, 406_312);
    }

    #[test]
    fn a_wrong_device_string_breaks_the_structural_match() {
        let mut data = synthetic(&PROFILE_V1_13, "V1.13");
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + 9].copy_from_slice(b"01017-00E");
        let image = FirmwareImage::new(&data).expect("header");
        let err = identify(&image).expect_err("device string disagrees");
        assert!(matches!(err, Error::Profile { .. }));
    }

    #[test]
    fn a_wrong_version_marker_breaks_the_structural_match() {
        let mut data = synthetic(&PROFILE_V1_13, "V1.11");
        // Keep the size plausible but the marker unknown: the error must name
        // the found values so a new profile can be written from it.
        data.truncate(PROFILE_V1_13.size);
        let image = FirmwareImage::new(&data).expect("header");
        let err = identify(&image).expect_err("marker disagrees");
        let text = format!("{err}");
        assert!(text.contains("406620"), "got {text}");
        assert!(text.contains("01017-00F"), "got {text}");
    }

    #[test]
    fn the_patched_marker_replaces_only_the_final_digit() {
        assert_eq!(patched_marker("V1.13"), "V1.14");
        assert_eq!(patched_marker("V1.12"), "V1.14");
        assert_eq!(patched_marker("V1.13").len(), VERSION_MARKER_LEN);
    }

    #[test]
    fn a_patched_image_is_still_identified_by_structure() {
        let data = synthetic(&PROFILE_V1_13, "V1.14");
        let image = FirmwareImage::new(&data).expect("header");
        let found = identify(&image).expect("a patched image is identifiable");
        assert_eq!(found.profile, &PROFILE_V1_13);
        assert_eq!(found.evidence, Evidence::Structure);
    }

    #[test]
    fn profiles_are_looked_up_by_any_spelling_of_the_name() {
        for name in ["V1.13", "v1.13", "1.13", " 1.13 "] {
            assert_eq!(by_name(name), Some(&PROFILE_V1_13), "name {name:?}");
        }
        assert_eq!(by_name("V1.99"), None);
        assert_eq!(all().len(), 2);
    }

    #[test]
    fn the_profiles_are_ordered_newest_first() {
        assert_eq!(PROFILES.first(), Some(&PROFILE_V1_13));
        assert_eq!(PROFILES.last(), Some(&PROFILE_V1_12));
    }
}
