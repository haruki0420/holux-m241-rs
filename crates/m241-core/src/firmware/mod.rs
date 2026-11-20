//! Firmware images: the MTK/Holux header, and the GPS epoch constant it carries.
//!
//! [`image`] only decodes an image: it parses the header and reads the epoch
//! constant, and never writes a byte. The profile table and the patch-plan types
//! are [`crate::types::FirmwareProfile`], [`crate::types::PatchPlan`] and
//! [`crate::types::PatchEdit`] in [`crate::types`], because the CLI and the GUI
//! both handle a plan before any image is opened.
//!
//! The rest of the patch path (identify an image, build a plan, write it back)
//! lands with the work tree that owns it. [`crate::types::PatchPlan::apply`] is
//! the part that already exists here.

pub mod image;

pub use image::{FirmwareImage, GpsEpoch};
