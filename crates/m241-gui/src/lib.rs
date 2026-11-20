//! `m241-gui` — the desktop interface.
//!
//! The shell was built before any screen content so that the navigation, the
//! tokens and the signature move exist once, in one place. The three screens in
//! [`screens`] are written against `DESIGN.md` §5.
//!
//! The GUI calls `m241_core` directly and never spawns the CLI: a subprocess would
//! make progress reporting, cancellation and the typed confirmation impossible to
//! keep honest. The Firmware screen runs the real patch planner and the real
//! exporter, so the plan a person reviews here is the byte-for-byte plan the CLI
//! would apply.
//!
//! What the screens do *not* do yet is drive the device: the session layer that
//! would read a firmware image or download a log is not written, and the write
//! path lives in `m241-flash`, which this crate deliberately does not depend on.
//! Those paths are visible on the screens as disabled controls labelled 未検証,
//! never as buttons that quietly do nothing.
//!
//! Exposed as a library so integration tests can drive the real [`App`] through
//! `egui`'s headless context. A GUI that compiles is not a GUI that works.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// See the note in m241-core: test code may panic.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod app;
pub mod screens;
pub mod screenshot;
pub mod theme;
pub mod widgets;

pub use app::App;
