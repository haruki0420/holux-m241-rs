//! The binary track log: blocks, records, and the exporters that turn a dump
//! into something a mapping application can open.
//!
//! ```text
//!   block      a 64 KiB block and its 0x200-byte header, whose bitmask
//!              decides which fields each record in that block carries
//!   parse      a whole dump -> `Track`, tolerating the damage real dumps have
//!   export     a `Track` -> GPX / KML / CSV / NMEA / JSON
//! ```
//!
//! Nothing in here opens a file or a device: a dump arrives as a byte slice. That
//! is what lets `m241 log read` work on a machine with no logger attached, and it
//! is why the exporters can be tested on hand-built records instead of on
//! somebody's real track.

pub mod block;
pub mod export;
pub mod parse;
pub mod record;

pub use block::{BLOCK_HEADER_SIZE, BLOCK_SIZE, BlockHeader};
pub use export::{export, to_csv, to_gpx, to_json, to_kml, to_nmea};
pub use parse::{ParseReport, parse_dump};
pub use record::{DEFAULT_RECORD_BYTES, LogRecord, MIN_RECORD_BYTES};
