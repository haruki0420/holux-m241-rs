//! Walking a dump: blocks, records, damage, and where the track ends.
//!
//! A real dump is not a clean file. It has blocks that were never written (all
//! `0xFF`), blocks that were half-erased, a final record cut short by a power-off,
//! and stale records from an earlier overwrite cycle. The reader's job is to
//! return every point it can defend and to say — per block, and in totals — what
//! it had to skip.
//!
//! This is deliberately forgiving in the same way `fsck` is forgiving: a track
//! with one damaged block is still worth exporting, and refusing the whole dump
//! because one checksum failed would throw away the trip. [`ParseReport`] carries
//! the damage so the caller can warn instead of hiding it.

use crate::error::{Error, Result};
use crate::log::block::{
    BLOCK_HEADER_SIZE, BLOCK_SIZE, BlockHeader, POLICY_LOG_DISABLED, POLICY_LOG_ENABLED,
    is_padding, record_length,
};
use crate::log::record::LogRecord;
use crate::types::{LogSummary, Track};

/// What the parser had to skip, in the same units as [`LogSummary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParseReport {
    /// Blocks fully or partly read.
    pub blocks: usize,
    /// Blocks whose header was all `0xFF`: never written.
    pub empty_blocks: usize,
    /// Blocks with at least one record whose checksum failed.
    pub damaged_blocks: usize,
    /// Records that decoded and are in the track.
    pub records: usize,
    /// Records whose checksum did not match (they are still in the track).
    pub bad_checksums: usize,
    /// Records skipped because the bytes ran out, typically the final write.
    pub truncated_records: usize,
    /// Bytes consumed by the last block, for a "how full is it" figure.
    pub used_bytes: usize,
}

impl ParseReport {
    /// The one-line summary the interface shows.
    #[must_use]
    pub fn summary(&self, track: &Track) -> LogSummary {
        let mut summary = LogSummary::of(track);
        summary.damaged_blocks = self.damaged_blocks;
        summary
    }
}

/// Parse a raw log dump.
///
/// The first block's header decides the record layout for that block; each later
/// block carries its own, so a dump written across a configuration change parses
/// correctly. A bad checksum marks the record but keeps it: the payload is almost
/// always intact and the point is almost always worth keeping.
///
/// # Errors
///
/// [`Error::Log`] only when the input cannot be a dump at all: an empty buffer is
/// not an error (the device reports an empty log), but a buffer whose first block
/// has a policy word that is neither of the two the format defines is. That check
/// is what stops a firmware image or a random binary being parsed into a track
/// that looks plausible and exports as someone else's position.
pub fn parse_dump(data: &[u8]) -> Result<(Track, ParseReport)> {
    let mut track = Track::default();
    let mut report = ParseReport::default();
    if data.len() < BLOCK_HEADER_SIZE {
        return Ok((track, report));
    }

    let mut base = 0usize;
    while base + BLOCK_HEADER_SIZE <= data.len() {
        let block = data.get(base..).unwrap_or(&[]);
        let header = BlockHeader::parse(block, base)?;
        report.blocks += 1;

        // An erased block is all 0xFF, which parses as count 0xFFFF and a mask
        // of all ones. It holds no records and must not be reported as damage.
        let erased = header.count.is_none() && header.effective_bitmask() == 0xFFFF_FFFF >> 1;
        if erased {
            report.empty_blocks += 1;
            base += BLOCK_SIZE;
            continue;
        }
        // The policy word is the format's own "is this a log block" marker: it is
        // written when the block starts and carries one of two values. A first
        // block that has neither is not a log, and reading on would produce a
        // track out of whatever the bytes happen to be.
        if base == 0 && !is_known_policy(header.policy) {
            return Err(Error::Log {
                message: format!(
                    "block header at 0x0 has policy 0x{:04X}, which is neither \
                     0x{POLICY_LOG_DISABLED:04X} nor 0x{POLICY_LOG_ENABLED:04X}: \
                     this is not an M-241 log dump",
                    header.policy
                ),
                offset: Some(6),
            });
        }

        let end = base.saturating_add(BLOCK_SIZE).min(data.len());
        let mut pos = header.data_offset();
        let mut in_block = 0usize;
        let mut block_damaged = false;

        while pos < end {
            let rest = match data.get(pos..end) {
                Some(rest) => rest,
                None => break,
            };
            if is_padding(rest) {
                // 0xFF padding ends the used area; the rest of the block is
                // unwritten and there is nothing to recover from it.
                break;
            }
            if header
                .count
                .is_some_and(|count| in_block >= usize::from(count))
            {
                break;
            }
            let length = record_length(header.effective_bitmask(), rest);
            if length == 0 || rest.len() < length {
                report.truncated_records += 1;
                break;
            }
            let Some(bytes) = rest.get(..length) else {
                report.truncated_records += 1;
                break;
            };
            match LogRecord::decode(bytes, header.effective_bitmask()) {
                Ok(record) => {
                    if !record.checksum_ok {
                        report.bad_checksums += 1;
                        block_damaged = true;
                    }
                    // MTK writes the whole record block including a waypoint
                    // record with no position; a point with neither a time nor a
                    // position carries no information, so it is dropped rather
                    // than exported as an empty row.
                    if record.utc.is_some() || record.latitude.is_some() {
                        track.points.push(record.to_track_point());
                        report.records += 1;
                    }
                }
                Err(_) => {
                    report.truncated_records += 1;
                    break;
                }
            }
            in_block += 1;
            report.used_bytes += length;
            pos += length;
        }

        if block_damaged {
            report.damaged_blocks += 1;
            if let Ok(offset) = u32::try_from(base) {
                track.damaged_blocks.push(offset);
            }
        }

        base += BLOCK_SIZE;
    }
    Ok((track, report))
}

/// Whether `policy` is one of the two values the log format defines.
fn is_known_policy(policy: u16) -> bool {
    policy == POLICY_LOG_DISABLED || policy == POLICY_LOG_ENABLED
}

/// Parse a dump and return only the track.
///
/// # Errors
///
/// As [`parse_dump`].
pub fn parse_track(data: &[u8]) -> Result<Track> {
    parse_dump(data).map(|(track, _report)| track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::block::{COUNT_NOT_FULL, POLICY_LOG_ENABLED, bit, xor_checksum};

    /// The measured V1.13 default mask: UTC + lat + lon + height.
    const MASK: u32 = 0x8000_001D;

    /// Append a 16-byte record (UTC, lat, lon, height) with a correct checksum.
    fn push_record(out: &mut Vec<u8>, utc: u32, lat: f32, lon: f32, height: &[u8; 3]) {
        let mut body = Vec::new();
        body.extend_from_slice(&utc.to_le_bytes());
        body.extend_from_slice(&lat.to_le_bytes());
        body.extend_from_slice(&lon.to_le_bytes());
        body.extend_from_slice(height);
        let checksum = xor_checksum(&body);
        body.push(checksum);
        out.extend_from_slice(&body);
    }

    /// Build one block: header, the records, then `0xFF` padding.
    fn build_block(records: &[(u32, f32, f32, [u8; 3])], count: u16) -> Vec<u8> {
        let mut block = vec![0xFFu8; BLOCK_SIZE];
        block[0..2].copy_from_slice(&count.to_le_bytes());
        block[2..6].copy_from_slice(&MASK.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
        block[8..12].copy_from_slice(&10u32.to_le_bytes());
        let mut cursor = BLOCK_HEADER_SIZE;
        let mut body = Vec::new();
        for (utc, lat, lon, height) in records {
            push_record(&mut body, *utc, *lat, *lon, height);
        }
        block[cursor..cursor + body.len()].copy_from_slice(&body);
        cursor += body.len();
        let _ = cursor;
        block
    }

    /// The repository's example coordinate. All three records share it on
    /// purpose: the log stores coordinates as `f32`, so a sub-metre difference
    /// would not survive the round trip and would make the fixture misleading.
    const LAT: f32 = 35.681_236;
    const LON: f32 = 139.767_13;

    fn sample() -> Vec<(u32, f32, f32, [u8; 3])> {
        vec![
            (1_700_000_000, LAT, LON, [0x00, 0xA0, 0x42]),
            (1_700_000_001, LAT, LON, [0x00, 0xA0, 0x42]),
            (1_700_000_002, LAT, LON, [0x00, 0xA0, 0x42]),
        ]
    }

    #[test]
    fn a_single_full_block_yields_every_record_in_order() {
        let records = sample();
        let dump = build_block(&records, records.len() as u16);
        let (track, report) = parse_dump(&dump).expect("a valid block");
        assert_eq!(track.points.len(), 3);
        assert_eq!(report.records, 3);
        assert_eq!(report.blocks, 1);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(track.points[0].utc, Some(1_700_000_000));
        assert_eq!(track.points[2].utc, Some(1_700_000_002));
        assert!(track.damaged_blocks.is_empty());
    }

    #[test]
    fn a_not_full_block_stops_at_the_padding() {
        let records = sample();
        let dump = build_block(&records, COUNT_NOT_FULL);
        let (track, _report) = parse_dump(&dump).expect("a valid block");
        assert_eq!(track.points.len(), 3);
    }

    #[test]
    fn two_blocks_are_both_read_and_the_second_keeps_its_own_layout() {
        let first = build_block(&sample(), 3);
        // The second block uses a 20-byte mask: same fields plus speed.
        let mut second = vec![0xFFu8; BLOCK_SIZE];
        second[0..2].copy_from_slice(&1u16.to_le_bytes());
        second[2..6].copy_from_slice(&0x8000_003Du32.to_le_bytes());
        let mut body = Vec::new();
        body.extend_from_slice(&1_700_000_010u32.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        body.extend_from_slice(&12.5f32.to_le_bytes());
        let checksum = xor_checksum(&body);
        body.push(checksum);
        second[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + body.len()].copy_from_slice(&body);

        let mut dump = first.clone();
        dump.extend_from_slice(&second);
        let (track, report) = parse_dump(&dump).expect("two valid blocks");
        assert_eq!(report.blocks, 2);
        assert_eq!(track.points.len(), 4);
        assert_eq!(track.points[3].utc, Some(1_700_000_010));
        assert_eq!(track.points[3].speed_kmh, Some(12.5));
    }

    #[test]
    fn an_erased_block_is_counted_but_not_called_damage() {
        let dump = vec![0xFFu8; BLOCK_SIZE];
        let (track, report) = parse_dump(&dump).expect("erased flash is not an error");
        assert_eq!(track.points.len(), 0);
        assert_eq!(report.empty_blocks, 1);
        assert_eq!(report.damaged_blocks, 0);
        assert!(track.damaged_blocks.is_empty());
    }

    #[test]
    fn a_bad_checksum_marks_the_block_but_keeps_the_point() {
        let records = sample();
        let mut dump = build_block(&records, 3);
        // Corrupt one field byte of the first record.
        dump[BLOCK_HEADER_SIZE + 3] ^= 0xFF;
        let (track, report) = parse_dump(&dump).expect("the block structure is intact");
        assert_eq!(
            track.points.len(),
            3,
            "a bad checksum must not drop the point"
        );
        assert_eq!(report.bad_checksums, 1);
        assert_eq!(report.damaged_blocks, 1);
        assert_eq!(track.damaged_blocks, vec![0]);
    }

    #[test]
    fn a_truncated_final_record_is_skipped_and_counted() {
        let records = sample();
        let mut dump = build_block(&records, COUNT_NOT_FULL);
        // Trim the tail so the third record is incomplete.
        dump.truncate(BLOCK_HEADER_SIZE + 16 + 16 + 8);
        let (track, report) = parse_dump(&dump).expect("a truncated dump is still readable");
        assert_eq!(track.points.len(), 2);
        assert_eq!(report.truncated_records, 1);
    }

    #[test]
    fn a_firmware_image_is_refused_rather_than_parsed_into_a_track() {
        // A dump of the wrong kind of file must not produce a plausible track:
        // the policy word is the format's own marker, and a firmware header does
        // not carry it.
        let mut not_a_log = vec![0u8; BLOCK_SIZE];
        not_a_log[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        not_a_log[2..6].copy_from_slice(&0x8000_001Du32.to_le_bytes());
        not_a_log[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let err = parse_dump(&not_a_log).expect_err("not a log");
        let text = format!("{err}");
        assert!(text.contains("not an M-241 log dump"), "got {text}");
    }

    #[test]
    fn an_empty_dump_is_an_empty_track_not_an_error() {
        let (track, report) = parse_dump(&[]).expect("no bytes");
        assert!(track.is_empty());
        assert_eq!(report.blocks, 0);
        // A dump shorter than a header has nothing to parse.
        let (track, report) = parse_dump(&[0u8; 16]).expect("short input");
        assert!(track.is_empty());
        assert_eq!(report.blocks, 0);
    }

    #[test]
    fn the_summary_reports_the_span_and_the_damage() {
        let dump = build_block(&sample(), 3);
        let (track, report) = parse_dump(&dump).expect("valid block");
        let summary = report.summary(&track);
        assert_eq!(summary.records, 3);
        assert_eq!(summary.fixes, 3);
        assert_eq!(summary.started_at, Some(1_700_000_000));
        assert_eq!(summary.ended_at, Some(1_700_000_002));
    }

    #[test]
    fn a_waypoint_bit_survives_the_round_trip() {
        // UTC | LAT | LON | HEIGHT | RCR
        let mask = MASK | (1u32 << u32::from(bit::RCR));
        let mut block = vec![0xFFu8; BLOCK_SIZE];
        block[0..2].copy_from_slice(&1u16.to_le_bytes());
        block[2..6].copy_from_slice(&mask.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
        let mut body = Vec::new();
        body.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        body.extend_from_slice(&0x0008u16.to_le_bytes());
        let checksum = xor_checksum(&body);
        body.push(checksum);
        block[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + body.len()].copy_from_slice(&body);

        let (track, _report) = parse_dump(&block).expect("valid block");
        assert_eq!(track.points.len(), 1);
        assert!(track.points[0].is_waypoint());
    }
}
