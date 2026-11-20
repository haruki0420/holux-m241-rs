//! Firmware images: header parsing, the profile table, and the WNRO patch.
//!
//! [`image`] only decodes an image: it parses the header and reads the epoch
//! constant, and never writes a byte. [`profiles`] says which release an image
//! is, and [`patch`] turns that into a [`crate::types::PatchPlan`]. The plan and
//! its edits are [`crate::types::PatchPlan`] / [`crate::types::PatchEdit`],
//! because the CLI and the GUI both handle a plan before any image is opened.
//!
//! Nothing here reads or writes a device: it operates on a byte slice the caller
//! already has. That is deliberate — the patch path is the one that can leave a
//! unit unbootable, so it is pure and testable, and the code that talks to a
//! device lives in [`crate::protocol`].
//!
//! ```text
//!   image::FirmwareImage   the 0x200-byte header, decoded on demand
//!   profiles               which release is this, by hash or by structure
//!   patch                  the plan that fixes the rollover, as edits only
//!   digest                 SHA-256, in-crate so there is no extra dependency
//! ```

pub mod digest;
pub mod image;
pub mod patch;
pub mod profiles;

pub use digest::{sha256, sha256_hex};
pub use image::{FirmwareImage, GpsEpoch};
pub use patch::{declared_size, is_complete_dump, plan, plan_for};
pub use profiles::{PROFILE_V1_12, PROFILE_V1_13, PROFILES, identify};
