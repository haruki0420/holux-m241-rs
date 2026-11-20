//! Shared widgets. The rail is here rather than under `screens` because it is
//! the shell's navigation, not one screen's content.
//!
//! `section` and `figure` are the product's ruled rhythm (DESIGN.md §3): they are
//! written once, before any screen exists, so every screen inherits the same
//! spacing and alignment instead of inventing its own.

#![allow(dead_code)]

pub mod rail;
