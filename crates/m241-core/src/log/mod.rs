//! The binary track log, its exporters, and photo matching.
//!
//! `record` decodes the bytes the recorder writes into
//! [`crate::types::TrackPoint`] values, the same type the CLI and the GUI pass
//! around, so there is one point type in the crate rather than one per layer.
//!
//! The block parser, the exporters and photo matching are still to land.

pub mod record;

pub use crate::types::TrackPoint;
