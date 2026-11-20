//! `m241-gui` — the desktop interface.
//!
//! The shell is built before any screen content so that the navigation, the
//! tokens and the signature move exist once, in one place. Screens are added
//! against `DESIGN.md`; see `screens/`.
//!
//! The GUI is **planned** to call `m241_core` directly and never spawn the CLI:
//! a subprocess would make progress reporting, cancellation and the typed
//! confirmation impossible to keep honest. That dependency is not declared yet —
//! today the shell only navigates — so the crate compiles without `m241_core`.
//! Declaring a dependency before its consumer exists is the warning this crate
//! deliberately avoids; the path stays reserved for the first screen that does
//! real work.
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
pub mod theme;
pub mod widgets;

pub use app::App;
