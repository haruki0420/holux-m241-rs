//! Shared widgets. The rail is here rather than under `screens` because it is
//! the shell's navigation, not one screen's content.
//!
//! The ruled rhythm lives in `screens::common` instead: it is page furniture that
//! screens compose with, not a control the shell draws.

#![allow(dead_code)]

pub mod rail;
