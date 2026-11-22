//! `m241-gui` — the desktop interface.
//!
//! The shell was built before any screen content so that the navigation, the
//! tokens and the signature move exist once, in one place. The three screens in
//! [`screens`] are written against `DESIGN.md` §5.
//!
//! The GUI calls `m241_core` and `m241_flash` directly and never spawns the CLI:
//! a subprocess would make progress reporting, cancellation and the typed
//! confirmation impossible to keep honest. The Firmware screen runs the real
//! patch planner and the real device session, so the plan and the write a person
//! reviews here are the byte-for-byte plan and the safety path the CLI would use.
//!
//! The write path is `m241_flash::flow`, which reaches flash through
//! `DownloadAgent` after `check_layout`, `check_image` and `verify_device`. A
//! typed `WRITE` in the GUI is the user's own acknowledgement; there is no
//! `--yes` equivalent, and an agent may not type it.
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
/// Device jobs the screens run off the UI thread.
pub mod device;
pub mod screens;
pub mod screenshot;
pub mod theme;
pub mod widgets;

pub use app::App;
