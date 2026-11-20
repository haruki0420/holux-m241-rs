//! The write path: everything that can destroy a logger lives in this crate.
//!
//! The repository is split in two along the line that matters:
//!
//! * [`m241_core`] reads. It talks to the PMTK channel and the BootROM, dumps
//!   firmware, downloads the log and parses it. Nothing in it can erase,
//!   program or restart a device.
//! * this crate writes. Log erasure ([`erase`]) and the Download Agent driver
//!   ([`da`]) live here, and this crate is the only one that may name them.
//!
//! The dependency runs one way: `m241-flash` → `m241-core`. That direction is
//! the safety property, not a style preference. A read-only binary built from
//! `m241-core` cannot link a write routine by accident, because the routine is
//! not in the crate it links — the compiler enforces what a reviewer would
//! otherwise have to keep checking. `tools/check_safety.py` verifies both that
//! `m241-core`'s sources carry no destructive symbol and that `m241-core`'s
//! dependency tree does not contain this crate.
//!
//! # Safety posture
//!
//! This crate inherits `m241-core`'s posture and adds the reason it matters
//! more here: the code in this module runs while a device is mid-operation. The
//! workspace therefore denies `unsafe`, `unwrap`, `expect`, `panic` and slice
//! indexing outside tests, so a failed write is an error value that the caller
//! can report, never a process abort with the chip left in an unknown state.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// See the note in m241-core: a panic in a test is a failed assertion with a
// useful message, not a device left in an undefined state.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod da;
pub mod erase;

pub use da::DownloadAgent;
pub use erase::{BackupProof, ERASE_SUBCOMMAND, LogEraser};

/// Version of the crate, taken from `Cargo.toml` at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
