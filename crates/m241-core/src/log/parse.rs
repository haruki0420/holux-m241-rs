//! Walking a dump: blocks, records, damage, and where the track ends.
//!
//! A real dump is not a clean file. It has blocks that were never written (all
//! `0xFF`), blocks that were half-erased, a final record cut short by a power-off,
//! and a **settings journal interleaved between the track records**. The reader
//! returns every point it can defend and says — per block, and in totals — what
//! it had to skip.
//!
//! This is deliberately forgiving in the same way `fsck` is forgiving: one
//! damaged block is a track worth keeping. [`ParseReport`] carries the damage so
//! the caller can warn instead of hiding it.
//!
//! Two rules keep the forgiveness from turning into invention: a record whose
//! checksum does not match is **not** exported as a point, and the record length
//! comes from checksum evidence rather than the header mask alone (a stale mask
//! has been measured). A block that holds only the journal yields no points.

use crate::error::{Error, Result};
use crate::log::block::{
    self, BLOCK_HEADER_SIZE, BLOCK_SIZE, BlockHeader, POLICY_LOG_DISABLED, POLICY_LOG_ENABLED,
    is_padding, record_length,
};
use crate::log::{journal, record};
use crate::types::{LogSummary, Track};

/// The fixed record lengths tried when the header mask may be stale.
///
/// A mask that declares another fixed length is tried too, so this only has to
/// cover the mismatch that has actually been seen.
const DETECTED_RECORD_LENGTHS: [usize; 2] = [16, 20];

/// Records the length-detection scan samples before it decides.
const DETECTION_SAMPLE_RECORDS: usize = 256;

/// Bytes a resync scan looks through for the next journal item or record grid.
const RESYNC_SCAN_BYTES: usize = 0x1000;

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
    /// Records whose checksum did not match. They are **not** in the track.
    pub bad_checksums: usize,
    /// Records skipped because the bytes ran out, typically the final write.
    pub truncated_records: usize,
    /// Settings-journal items skipped because they are not track records.
    pub journal_items: usize,
    /// Blocks whose header mask disagreed with the record length in the data.
    pub layout_mismatches: usize,
    /// Bytes no record grid or journal item could explain; they were skipped.
    pub unparsed_bytes: usize,
    /// Bytes consumed by the last block, for a "how full is it" figure.
    pub used_bytes: usize,
}

impl ParseReport {
    /// The one-line summary the interface shows.
    #[must_use]
    pub fn summary(&self, track: &Track) -> LogSummary {
        let mut summary = LogSummary::of(track);
        summary.damaged_block_count = self.damaged_blocks;
        summary
    }

    /// Whether the dump held only the settings journal, with no track records.
    ///
    /// This is a configured or erased unit that has not recorded a point yet,
    /// not a dump that contains nothing at all.
    #[must_use]
    pub fn is_journal_only(&self) -> bool {
        self.records == 0 && self.journal_items > 0
    }
}

/// Parse a raw log dump.
///
/// Each block carries its own header, so a dump written across a configuration
/// change parses correctly. Within a block the walk skips the settings journal,
/// checks every record's checksum before it becomes a point, and — where the
/// header mask and the data disagree about the record length — trusts the data
/// and reports the disagreement.
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
        let mask = header.effective_bitmask();
        // `None` means the mask selects the variable satellite block; that length
        // is a property of each record, not of the block.
        let declared_length = block::fixed_record_length(mask).filter(|length| *length > 0);
        let mut detected_length: Option<usize> = None;
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
            // Journal entries and the device name are configuration, not track
            // records, and they sit between record runs. Skipping them is what
            // keeps the read aligned with the records on the other side.
            if let Some(length) = journal::item_len(rest) {
                report.journal_items += 1;
                pos += length;
                continue;
            }
            if header
                .count
                .is_some_and(|count| in_block >= usize::from(count))
            {
                break;
            }
            // The record boundary is declared by the mask, unless checksum
            // evidence says the mask is stale; a variable-layout mask (the
            // satellite block) says it per record instead.
            let length = if let Some(mask_length) = declared_length {
                match detected_length {
                    Some(length) => length,
                    None => {
                        let Some(length) = detect_record_length(data, pos, end, mask_length) else {
                            // Nothing here is a record. Resume at the next
                            // journal item, padding, or record grid rather than
                            // inventing a point from these bytes.
                            let Some(next) = resync_offset(data, pos, end, mask_length) else {
                                report.unparsed_bytes += end - pos;
                                break;
                            };
                            report.unparsed_bytes += next - pos;
                            pos = next;
                            continue;
                        };
                        if length != mask_length {
                            // The mask's field selection is still the only
                            // evidence of what each field is, so the record is
                            // decoded with it; only the boundary comes from the
                            // data.
                            report.layout_mismatches += 1;
                        }
                        detected_length = Some(length);
                        length
                    }
                }
            } else {
                record_length(mask, rest)
            };
            if length == 0 {
                report.truncated_records += 1;
                break;
            }
            let Some(bytes) = rest.get(..length) else {
                report.truncated_records += 1;
                break;
            };
            // Fail closed: a record whose checksum does not match is not a
            // point. It is counted so the block can be warned about, and the
            // walk still advances by the record length because that much of the
            // format is known.
            if !block::checksum_matches_length(bytes, length) {
                report.bad_checksums += 1;
                block_damaged = true;
                in_block += 1;
                report.used_bytes += length;
                pos += length;
                continue;
            }
            match record::decode(bytes, mask) {
                Ok(point) => {
                    // A record with neither a time nor a position carries no
                    // information, so it is dropped rather than exported as an
                    // empty row.
                    if point.utc.is_some() || point.latitude.is_some() {
                        track.points.push(point);
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

/// Checksum evidence for one candidate record length.
#[derive(Debug, Clone, Copy, Default)]
struct Evidence {
    /// Records on the candidate grid whose trailer matched.
    valid: usize,
    /// Records on the candidate grid that were sampled.
    tried: usize,
    /// Records that matched at the start of the grid, before the first failure.
    ///
    /// This is the strong figure: a record run interrupted by a journal cluster
    /// has few valid records out of many sampled, but a long unbroken leading run
    /// cannot be random bytes.
    leading: usize,
}

/// How much checksum evidence supports `length` as the record length.
fn length_evidence(data: &[u8], pos: usize, end: usize, length: usize) -> Evidence {
    let mut evidence = Evidence::default();
    let mut at = pos;
    let mut leading = true;
    while at < end && evidence.tried < DETECTION_SAMPLE_RECORDS {
        let Some(rest) = data.get(at..end) else {
            break;
        };
        if is_padding(rest) {
            break;
        }
        let Some(candidate) = rest.get(..length) else {
            break;
        };
        evidence.tried += 1;
        if block::checksum_matches_length(candidate, length) {
            evidence.valid += 1;
            if leading {
                evidence.leading += 1;
            }
        } else {
            leading = false;
        }
        at = at.saturating_add(length);
    }
    evidence
}

/// Decide the record length for a block whose header mask declares
/// `mask_length`.
///
/// The mask is the device's own declaration, but a stale one has been measured:
/// a block that says 16-byte records while every record is 20 bytes and only the
/// 20-byte grid checksums. The mask's candidate is preferred when the evidence is
/// equal; an alternative length has to be corroborated more strongly, and neither
/// is accepted on evidence that random bytes could produce.
fn detect_record_length(data: &[u8], pos: usize, end: usize, mask_length: usize) -> Option<usize> {
    let mut candidates = vec![mask_length];
    for length in DETECTED_RECORD_LENGTHS {
        if !candidates.contains(&length) {
            candidates.push(length);
        }
    }
    let mut best: Option<(usize, Evidence)> = None;
    for length in candidates {
        let evidence = length_evidence(data, pos, end, length);
        let trusted = if length == mask_length {
            // The declared layout is believed on a short unbroken run, or when
            // most of the sampled grid matches (a few damaged records at the
            // start are normal).
            evidence.leading >= 2 || (evidence.valid >= 1 && evidence.valid * 2 >= evidence.tried)
        } else {
            // A layout the header did not declare needs an unbroken run before
            // the parser overrides the device's own mask.
            evidence.leading >= 2
        };
        if !trusted {
            continue;
        }
        if best.is_none_or(|(_, best_evidence)| evidence.leading > best_evidence.leading) {
            best = Some((length, evidence));
        }
    }
    best.map(|(length, _)| length)
}

/// Find the next offset that can be parsed: a journal item, the `0xFF` padding,
/// or a record grid that [`detect_record_length`] accepts.
///
/// Used when the bytes at `pos` are neither a journal item nor a record grid, so
/// the walk can resume instead of inventing a point. The scan is bounded; past
/// `RESYNC_SCAN_BYTES` the rest of the block is reported as unparsed.
fn resync_offset(data: &[u8], pos: usize, end: usize, mask_length: usize) -> Option<usize> {
    let stop = end.min(pos.saturating_add(RESYNC_SCAN_BYTES));
    let mut candidates = vec![mask_length, 16, 20];
    candidates.sort_unstable();
    candidates.dedup();
    for at in (pos.saturating_add(1))..stop {
        let Some(rest) = data.get(at..end) else {
            break;
        };
        if journal::item_len(rest).is_some() || is_padding(rest) {
            return Some(at);
        }
        // Cheap prefilter: without it, the evidence scan would run at every byte
        // of the resync window.
        let starts_a_record = candidates.iter().any(|length| {
            rest.get(..*length)
                .is_some_and(|record| block::checksum_matches_length(record, *length))
        });
        if starts_a_record && detect_record_length(data, at, end, mask_length).is_some() {
            return Some(at);
        }
    }
    None
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

    /// The measured 20-byte mask: UTC + lat + lon + height + float speed.
    const MASK20: u32 = 0x8000_003D;

    /// The repository's example coordinate. All three records share it on
    /// purpose: the log stores coordinates as `f32`, so a sub-metre difference
    /// would not survive the round trip and would make the fixture misleading.
    const LAT: f32 = 35.681_236;
    const LON: f32 = 139.767_13;

    /// The device-name string the settings journal interleaves with its entries.
    const JOURNAL_NAME: &[u8; 20] = b"HOLUXGR241LOGGER    ";

    /// One 16-byte settings/state journal entry, in the shape a dump carries.
    fn journal_entry(kind: u8, value: u32) -> [u8; 16] {
        let mut entry = [0xAAu8; 16];
        entry[7] = kind;
        entry[8..12].copy_from_slice(&value.to_le_bytes());
        entry[12..16].copy_from_slice(&[0xBB; 4]);
        entry
    }

    /// One journal cluster: the device name, then six entries. Real dumps use
    /// this shape between record runs.
    fn push_journal_cluster(out: &mut Vec<u8>) {
        out.extend_from_slice(JOURNAL_NAME);
        for (kind, value) in [
            (7u8, 0x0106u32),
            (7, 0x0104),
            (3, 50),
            (4, 1000),
            (3, 50),
            (4, 0),
        ] {
            out.extend_from_slice(&journal_entry(kind, value));
        }
    }

    /// Append a 20-byte record (UTC, lat, lon, height, speed) with a correct
    /// checksum.
    fn push_record20(out: &mut Vec<u8>, utc: u32) {
        let mut body = Vec::new();
        body.extend_from_slice(&utc.to_le_bytes());
        body.extend_from_slice(&LAT.to_le_bytes());
        body.extend_from_slice(&LON.to_le_bytes());
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        body.extend_from_slice(&12.5f32.to_le_bytes());
        let checksum = xor_checksum(&body);
        body.push(checksum);
        out.extend_from_slice(&body);
    }

    /// One block with an explicit mask, count and body, padded with `0xFF`.
    fn build_custom_block(mask: u32, count: u16, body: &[u8]) -> Vec<u8> {
        let mut block = vec![0xFFu8; BLOCK_SIZE];
        block[0..2].copy_from_slice(&count.to_le_bytes());
        block[2..6].copy_from_slice(&mask.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
        block[8..12].copy_from_slice(&10u32.to_le_bytes());
        block[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + body.len()].copy_from_slice(body);
        block
    }

    fn parse_body(mask: u32, count: u16, body: &[u8]) -> (Track, ParseReport) {
        let dump = build_custom_block(mask, count, body);
        parse_dump(&dump).expect("a synthetic block")
    }

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
    fn a_bad_checksum_marks_the_block_and_is_not_exported() {
        let records = sample();
        let mut dump = build_block(&records, 3);
        // Corrupt one field byte of the first record.
        dump[BLOCK_HEADER_SIZE + 3] ^= 0xFF;
        let (track, report) = parse_dump(&dump).expect("the block structure is intact");
        assert_eq!(
            track.points.len(),
            2,
            "a bad checksum must not be exported as a point"
        );
        assert_eq!(report.bad_checksums, 1);
        assert_eq!(report.damaged_blocks, 1);
        assert_eq!(track.damaged_blocks, vec![0]);
        assert_eq!(track.points[0].utc, Some(1_700_000_001));
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

    #[test]
    fn a_hundred_twenty_byte_records_keep_every_point_and_no_damage() {
        let mut body = Vec::new();
        for index in 0..100u32 {
            push_record20(&mut body, 1_700_000_000 + index);
        }
        let (track, report) = parse_body(MASK20, 100, &body);
        assert_eq!(track.points.len(), 100);
        assert_eq!(report.records, 100);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(report.damaged_blocks, 0);
        assert_eq!(report.journal_items, 0);
        assert_eq!(report.layout_mismatches, 0);
        assert!(!report.is_journal_only());
        assert!(track.damaged_blocks.is_empty());
        assert_eq!(track.points[99].utc, Some(1_700_000_099));
        assert_eq!(track.points[99].speed_kmh, Some(12.5));
    }

    #[test]
    fn a_journal_only_dump_yields_no_points_at_all() {
        // The 2026-11-22 dumps: a well-formed header, then nothing but settings
        // journal. Every journal byte is a non-record, so no point may come out,
        // and the result must say "journal only" rather than "empty log".
        let mut body = Vec::new();
        body.extend_from_slice(&journal_entry(3, 50));
        body.extend_from_slice(&journal_entry(4, 1000));
        for _ in 0..3 {
            push_journal_cluster(&mut body);
        }
        let (track, report) = parse_body(MASK20, COUNT_NOT_FULL, &body);
        assert!(track.is_empty(), "journal bytes are not track records");
        assert_eq!(report.records, 0);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(report.damaged_blocks, 0);
        assert_eq!(report.journal_items, 2 + 3 * 7);
        assert_eq!(report.layout_mismatches, 0);
        assert!(report.is_journal_only());
        assert!(track.damaged_blocks.is_empty());
    }

    #[test]
    fn journal_clusters_between_records_do_not_shift_the_points() {
        let mut body = Vec::new();
        let mut utc = 1_700_000_000u32;
        let mut expected = 0usize;
        for run in [10usize, 20, 5] {
            for _ in 0..run {
                push_record20(&mut body, utc);
                utc += 1;
                expected += 1;
            }
            push_journal_cluster(&mut body);
        }
        let (track, report) = parse_body(MASK20, expected as u16, &body);
        assert_eq!(track.points.len(), expected);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(report.damaged_blocks, 0);
        assert_eq!(report.journal_items, 3 * 7);
        assert_eq!(report.layout_mismatches, 0);
        // The points after each cluster keep their place and their time.
        assert_eq!(track.points[10].utc, Some(1_700_000_010));
        assert_eq!(track.points[30].utc, Some(1_700_000_030));
    }

    #[test]
    fn a_stale_sixteen_byte_mask_still_reads_twenty_byte_records() {
        // The 2026-11-21 block 0 shape: the header says 16-byte records, but the
        // data is a journal lead followed by 20-byte records. The record length
        // must come from the data, and the disagreement must be reported rather
        // than silently guessed at.
        let mut body = Vec::new();
        for (kind, value) in [
            (3u8, 50u32),
            (4, 1000),
            (2, 0x8000_003D),
            (6, 0x0104),
            (3, 50),
            (4, 0),
            (7, 0x0106),
        ] {
            body.extend_from_slice(&journal_entry(kind, value));
        }
        body.extend_from_slice(JOURNAL_NAME);
        for index in 0..3u32 {
            push_record20(&mut body, 1_700_000_000 + index);
        }
        let (track, report) = parse_body(0x8000_001D, 3, &body);
        assert_eq!(track.points.len(), 3);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(report.damaged_blocks, 0);
        assert_eq!(report.journal_items, 8);
        assert_eq!(
            report.layout_mismatches, 1,
            "the stale mask must be reported, not silently trusted"
        );
        assert_eq!(track.points[0].utc, Some(1_700_000_000));
    }

    #[test]
    fn a_record_whose_checksum_fails_is_not_exported() {
        let mut body = Vec::new();
        for index in 0..3u32 {
            push_record20(&mut body, 1_700_000_000 + index);
        }
        // Corrupt one field byte of the second record: that record is damage,
        // not a point whose position happens to look plausible.
        body[20 + 3] ^= 0xFF;
        let (track, report) = parse_body(MASK20, 3, &body);
        assert_eq!(track.points.len(), 2);
        assert_eq!(report.bad_checksums, 1);
        assert_eq!(report.damaged_blocks, 1);
        assert_eq!(track.damaged_blocks, vec![0]);
    }

    #[test]
    fn unreadable_bytes_before_the_records_are_skipped() {
        // A short prefix the record grid cannot explain is skipped, and the
        // records after it are still read.
        let mut body = vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        for index in 0..4u32 {
            push_record20(&mut body, 1_700_000_000 + index);
        }
        let (track, report) = parse_body(MASK20, 4, &body);
        assert_eq!(track.points.len(), 4);
        assert_eq!(report.bad_checksums, 0);
        assert_eq!(report.unparsed_bytes, 7);
    }

    #[test]
    fn an_erased_block_is_not_journal_only() {
        let (track, report) = parse_dump(&vec![0xFFu8; BLOCK_SIZE]).expect("erased flash");
        assert!(track.is_empty());
        assert_eq!(report.journal_items, 0);
        assert!(!report.is_journal_only());
    }
}
