//! The binary track log: blocks, records, and the exporters that turn a dump
//! into something a mapping application can open.
//!
//! ```text
//!   block      a 64 KiB block and its 0x200-byte header, whose bitmask
//!              decides which fields each record in that block carries
//!   record     one record -> a TrackPoint, for that block's bitmask
//!   parse      a whole dump -> `Track`, tolerating the damage real dumps have
//!   export     a `Track` -> GPX / KML / CSV / NMEA / JSON
//!   kmz        a `Track` + photos -> uncompressed KMZ (KML 2.0 + Store ZIP)
//!   import     GPX 1.0/1.1 or NMEA 0183 -> `Track`
//!   edit       split, merge, delete points, and multi-step Undo/Redo
//!   coord      format coordinates as Decimal, DMS, or WGS84 UTM
//!   exif       the GPS tags in a JPEG's Exif block: read, validate, replace
//!   geotag     a photo's capture time -> the track point it was taken at
//! ```
//!
//! A decoded point is a [`crate::types::TrackPoint`], the same type the CLI and
//! the GUI pass around, so there is one point type in the crate rather than one
//! per layer.
//!
//! Nothing in here opens a file or a device: a dump arrives as a byte slice. That
//! is what lets `m241 log read` work on a machine with no logger attached, and it
//! is why the exporters can be tested on hand-built records instead of on
//! somebody's real track.
//!
//! [`geotag`] is the one part that does touch the filesystem, because a photo is
//! a file rather than an in-memory document. It is split into [`geotag::plan`],
//! which reads and decides, and [`geotag::apply`], which writes — the same split
//! that keeps `--dry-run` and the real run on one code path everywhere else in
//! this crate.

pub mod block;
pub mod coord;
pub mod edit;
pub mod exif;
pub mod export;
pub mod geotag;
pub mod import;
pub mod kmz;
pub mod parse;
pub mod record;

#[cfg(test)]
mod fixture;

pub use crate::types::TrackPoint;
pub use block::{BLOCK_HEADER_SIZE, BLOCK_SIZE, BlockHeader};
pub use coord::{CoordFormat, format_coordinate};
pub use edit::{TrackEditor, delete_points, merge_tracks, split_track};
pub use exif::{
    GpsPosition, GpsTags, MAX_SEGMENT_LENGTH, read_datetime_original, read_gps, write_gps,
};
pub use export::{export, to_csv, to_gpx, to_json, to_kml, to_nmea};
pub use geotag::{
    CaptureTime, GeotagOptions, GeotagPlan, apply as apply_geotags, group_matched_plans,
    plan as plan_geotags,
};
pub use import::{import_gpx, import_nmea};
pub use kmz::{KmzPhoto, crc32_ieee, export_kmz};
pub use parse::{ParseReport, parse_dump};
pub use record::{DEFAULT_RECORD_BYTES, MIN_RECORD_BYTES, decode};
