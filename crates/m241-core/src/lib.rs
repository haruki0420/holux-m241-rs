//! Protocol, firmware and track-log handling for the Holux M-241 GPS logger.
//!
//! The crate is split along the same three seams as the device's own software:
//!
//! * [`protocol`] — the serial wire: MTK BootROM frames, the Download Agent's
//!   flash commands, PMTK sentences the firmware understands.
//! * [`firmware`] — the image on the flash chip: header, profiles, the
//!   week-number-rollover patch.
//! * [`log`] — the binary track log the recorder writes, its exporters, and
//!   matching photos to it.
//!
//! Nothing in here opens a serial port unless [`protocol::Port`] is constructed,
//! so the offline commands (`fw patch`, `log read`, `nmea repair`) work on a
//! machine with no device attached.
//!
//! # Safety posture
//!
//! The crate denies `unsafe`, `unwrap`, `expect` and `panic` outside tests. A
//! serial failure is an error value, never a process abort — the device may be
//! mid-write when the cable is pulled, and the recovery instructions have to
//! survive that.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// Tests are allowed to unwrap and index: a panic there is a failed assertion
// with a useful message, not a device left in an undefined state.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod error;
pub mod firmware;
pub mod log;
pub mod protocol;
pub mod types;

pub use error::{Error, Result};
pub use types::{
    DeviceInfo, ExportFormat, FirmwareProfile, LogSummary, PatchEdit, PatchPlan, Track, TrackPoint,
};

/// Version of the crate, taken from `Cargo.toml` at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
