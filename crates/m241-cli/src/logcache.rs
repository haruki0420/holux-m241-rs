//! The JSON parse cache `m241 log read --cache` keeps beside a dump.
//!
//! Parsing a 64 KiB-block dump and rebuilding its track is the slow part of
//! `log read`; this module stores the result so a second run can read it back.
//! **The dump stays the source of truth**: a cache is used only when it was
//! written for exactly this input (length *and* 64-bit content hash), and
//! anything unreadable, stale or from another schema version is a warning and a
//! re-parse, never an error. A hit returns the **track and the parse report**
//! together, because the report is what tells the user about damaged blocks and
//! skipped bytes — a hit that dropped it would make a damaged dump look clean on
//! the second run.
//!
//! The file is this project's own format, not ezTour's `.tki`: the
//! specification records that ezTour caches, not what its cache holds, so no
//! compatibility is claimed.

use std::path::Path;

use m241_core::log::ParseReport;
use m241_core::{Error, Result, Track, TrackPoint};

use crate::fileio;
use crate::output::Console;

/// Marker and schema version of this project's parse cache.
const CACHE_FORMAT: &str = "m241-cache";
/// Schema version of the parse cache; a different version is ignored and rebuilt.
///
/// Bumped to `2` when the cache gained the parse report: a version-1 cache holds
/// no report, so serving it would silently drop the damage warnings.
const CACHE_VERSION: u64 = 2;

/// What a cache hit yields: the parsed track and the report it was written with.
pub(crate) struct Cached {
    /// The track as the cached run parsed it.
    pub(crate) track: Track,
    /// The parser's observations from that run, replayed as warnings on a hit.
    pub(crate) report: ParseReport,
}

/// 64-bit FNV-1a hash of `bytes`.
///
/// A content hash, not a timestamp: two dumps of the same unit on different days
/// can share a size, and an mtime is not portable between a copy and its source.
/// FNV-1a is not cryptographic — a cache is not a security boundary, and the
/// dump itself is what gets parsed when the hash differs.
fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Load `path` when it describes exactly `bytes`; `None` when it is stale.
///
/// # Errors
///
/// [`Error::Io`] when the cache exists but cannot be read.
pub(crate) fn load(path: &Path, bytes: &[u8], console: &dyn Console) -> Result<Option<Cached>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::io(path.display().to_string(), source)),
    };
    let document = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(document) => document,
        Err(_) => {
            console.warn(&format!(
                "{}: not a JSON cache; the dump will be parsed again",
                path.display()
            ));
            return Ok(None);
        }
    };
    if document.get("format").and_then(serde_json::Value::as_str) != Some(CACHE_FORMAT)
        || document.get("version").and_then(serde_json::Value::as_u64) != Some(CACHE_VERSION)
    {
        console.warn(&format!(
            "{}: not a cache this build understands; the dump will be parsed again",
            path.display()
        ));
        return Ok(None);
    }
    let expected_hash = format!("{:016x}", content_hash(bytes));
    let expected_length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let Some(source) = document.get("source") else {
        console.warn(&format!(
            "{}: the cache records no source; the dump will be parsed again",
            path.display()
        ));
        return Ok(None);
    };
    if source.get("bytes").and_then(serde_json::Value::as_u64) != Some(expected_length)
        || source.get("hash").and_then(serde_json::Value::as_str) != Some(expected_hash.as_str())
    {
        return Ok(None);
    }
    // No report means a cache from before the report was stored (or a hand-edited
    // file); parsing again is cheaper than a run whose warnings are missing.
    let Some(report) = document.get("parse").and_then(report_from_document) else {
        console.warn(&format!(
            "{}: the cache has no parse report; the dump will be parsed again",
            path.display()
        ));
        return Ok(None);
    };
    let Some(entries) = document.get("points").and_then(serde_json::Value::as_array) else {
        console.warn(&format!(
            "{}: the cache has no point list; the dump will be parsed again",
            path.display()
        ));
        return Ok(None);
    };
    let mut track = Track::default();
    for entry in entries {
        let Some(point) = cache_point(entry) else {
            console.warn(&format!(
                "{}: a cached point could not be read; the dump will be parsed again",
                path.display()
            ));
            return Ok(None);
        };
        track.points.push(point);
    }
    if let Some(blocks) = document
        .get("damaged_blocks")
        .and_then(serde_json::Value::as_array)
    {
        for block in blocks {
            let Some(block) = block.as_u64().and_then(|value| u32::try_from(value).ok()) else {
                continue;
            };
            track.damaged_blocks.push(block);
        }
    }
    Ok(Some(Cached { track, report }))
}

/// Write the cache beside the dump.
///
/// # Errors
///
/// [`Error::Io`] when the cache cannot be written, or [`Error::State`] when the
/// document cannot be serialised (it cannot fail for these values).
pub(crate) fn store(path: &Path, bytes: &[u8], track: &Track, report: &ParseReport) -> Result<()> {
    let document = serde_json::json!({
        "format": CACHE_FORMAT,
        "version": CACHE_VERSION,
        "source": {
            "bytes": u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            "hash": format!("{:016x}", content_hash(bytes)),
        },
        "parse": report_document(report),
        "damaged_blocks": track.damaged_blocks,
        "points": track.points.iter().map(|point| serde_json::json!({
            "utc": point.utc,
            "lat": point.latitude,
            "lon": point.longitude,
            "height_m": point.height_m,
            "speed_kmh": point.speed_kmh,
            "rcr": point.rcr,
        })).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string(&document)
        .map_err(|err| Error::State(format!("could not serialise the parse cache: {err}")))?;
    fileio::write_text(path, &text)
}

/// The counters a cache hit has to replay, every field of [`ParseReport`].
///
/// All of them, not only the ones a warning reads: the report is also printed in
/// full by `log info`, and a cache that quietly forgot `journal_items` or
/// `empty_records` would answer differently from a fresh parse.
fn report_document(report: &ParseReport) -> serde_json::Value {
    serde_json::json!({
        "blocks": report.blocks,
        "empty_blocks": report.empty_blocks,
        "damaged_blocks": report.damaged_blocks,
        "records": report.records,
        "bad_checksums": report.bad_checksums,
        "empty_records": report.empty_records,
        "truncated_records": report.truncated_records,
        "journal_items": report.journal_items,
        "layout_mismatched_blocks": report.layout_mismatched_blocks,
        "unparsed_bytes": report.unparsed_bytes,
        "used_bytes": report.used_bytes,
    })
}

/// Parse every [`ParseReport`] counter back; `None` when one is missing or not a
/// non-negative integer.
///
/// A missing counter fails the whole cache rather than becoming zero: a zero
/// would turn "3 damaged blocks" into "clean", which is exactly the silent
/// success this cache must not produce.
fn report_from_document(value: &serde_json::Value) -> Option<ParseReport> {
    let object = value.as_object()?;
    let counter = |key: &str| -> Option<usize> {
        object
            .get(key)?
            .as_u64()
            .and_then(|raw| usize::try_from(raw).ok())
    };
    Some(ParseReport {
        blocks: counter("blocks")?,
        empty_blocks: counter("empty_blocks")?,
        damaged_blocks: counter("damaged_blocks")?,
        records: counter("records")?,
        bad_checksums: counter("bad_checksums")?,
        empty_records: counter("empty_records")?,
        truncated_records: counter("truncated_records")?,
        journal_items: counter("journal_items")?,
        layout_mismatched_blocks: counter("layout_mismatched_blocks")?,
        unparsed_bytes: counter("unparsed_bytes")?,
        used_bytes: counter("used_bytes")?,
    })
}

/// One cached point, or `None` when a field has a shape the cache cannot hold.
///
/// A present-but-wrong field fails the whole cache rather than becoming a
/// default: a cache that silently turns a latitude into `None` would produce a
/// track that differs from the dump, which is worse than parsing the dump again.
fn cache_point(value: &serde_json::Value) -> Option<TrackPoint> {
    let object = value.as_object()?;
    let integer = |key: &str| -> Option<Option<u64>> {
        match object.get(key) {
            None | Some(serde_json::Value::Null) => Some(None),
            Some(found) => found.as_u64().map(Some),
        }
    };
    let float = |key: &str| -> Option<Option<f64>> {
        match object.get(key) {
            None | Some(serde_json::Value::Null) => Some(None),
            Some(found) => found.as_f64().filter(|value| value.is_finite()).map(Some),
        }
    };
    let utc = match integer("utc")? {
        Some(raw) => Some(u32::try_from(raw).ok()?),
        None => None,
    };
    let rcr = match integer("rcr")? {
        Some(raw) => Some(u16::try_from(raw).ok()?),
        None => None,
    };
    let height_m = float("height_m")?.map(|raw| raw as f32);
    let speed_kmh = float("speed_kmh")?.map(|raw| raw as f32);
    Some(TrackPoint {
        utc,
        latitude: float("lat")?,
        longitude: float("lon")?,
        height_m,
        speed_kmh,
        rcr,
    })
}
