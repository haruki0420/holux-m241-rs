//! The settings journal that shares a log block with the track records.
//!
//! A block is not a pure array of track records: the unit also writes a journal
//! of configuration and state changes into the same 64 KiB area, between record
//! runs. Two shapes have been observed:
//!
//! * a **16-byte entry**, `AA`×7 | type (1 byte) | value (`u32` LE) | `BB`×4;
//! * a **20-byte name**, `HOLUXGR241LOGGER    `, which separates clusters.
//!
//! A cluster is one name plus the entries that follow it; the walk treats an
//! entry and a name as one "journal item" each, because both are skipped the
//! same way.
//!
//! Entry types observed in the 2026-11-21 dump (248 items: 34 names and 214
//! entries): type 3 ×68, type 4 ×68, type 7 ×75, type 6 ×2, type 2 ×1. The two
//! 2026-11-22 journal-only dumps carry types 3, 4 and 7 only. Meanings are
//! inference from the values, not firmware: 50 looks like the header's period
//! (5.0 s), `0x0104` / `0x0106` like its policy words, and the type-6 values are
//! exactly those policy words.
//!
//! Type 2 is the one type with an observation behind the mask theory: it appears
//! once, in block 0 of the 2026-11-21 dump at file offset `0x220`, with value
//! `0x8800_003D` — the same mask the 20-byte records that follow it use, and the
//! one blocks 1–3 carry in their headers. So a type-2 entry *can* carry a record
//! mask. It is still not applied: the record length is already decided from
//! checksum evidence (see [`crate::log::parse`]), and acting on one unconfirmed
//! value is the kind of guess this module exists to prevent.
//!
//! Reading journal bytes as records is what used to invent points. The earlier
//! parser exported every record whatever its checksum: on the 2026-11-21 dump
//! 11,088 points, 4,481 of them checksum-failing (3,244 from the stale 16-byte
//! mask on block 0 alone), and the two 2026-11-22 journal-only dumps became 135
//! and 137 points built from configuration bytes. Skipping the journal keeps the
//! read aligned with the records on either side.

/// Bytes of one settings journal entry.
const ENTRY_SIZE: usize = 16;

/// Bytes of the device-name string that separates journal clusters.
const NAME_SIZE: usize = 20;

/// The device-name string, exactly as the observed dumps carry it. The four
/// trailing spaces are part of the field.
const NAME: &[u8] = b"HOLUXGR241LOGGER    ";

/// The opening marker of an entry: seven `0xAA` bytes.
const HEAD: &[u8] = &[0xAA; 7];

/// The closing marker of an entry: four `0xBB` bytes.
const TAIL: &[u8] = &[0xBB; 4];

/// Bytes of one item at the start of `bytes`, when the bytes are a journal
/// item.
///
/// Recognition checks only what is known exactly: the seven-byte head, the
/// four-byte tail, and the whole name string. The type and value bytes are not
/// checked because their meanings are unconfirmed — so a track record that
/// happened to carry this template would be skipped as configuration. That is
/// the cost of the strict match, and it is the safe direction: a wildcard match
/// would let configuration be read as records again, which is the bug this
/// module exists to prevent.
#[must_use]
pub fn item_len(bytes: &[u8]) -> Option<usize> {
    if bytes.get(..NAME_SIZE) == Some(NAME) {
        return Some(NAME_SIZE);
    }
    let entry = bytes.get(..ENTRY_SIZE)?;
    if entry.get(..HEAD.len()) == Some(HEAD) && entry.get(12..16) == Some(TAIL) {
        Some(ENTRY_SIZE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: u8, value: u32) -> [u8; ENTRY_SIZE] {
        let mut bytes = [0xAAu8; ENTRY_SIZE];
        bytes[7] = kind;
        bytes[8..12].copy_from_slice(&value.to_le_bytes());
        bytes[12..16].copy_from_slice(&[0xBB; 4]);
        bytes
    }

    #[test]
    fn an_entry_is_recognised_by_its_markers() {
        assert_eq!(item_len(&entry(7, 0x0106)), Some(ENTRY_SIZE));
        assert_eq!(item_len(&entry(3, 50)), Some(ENTRY_SIZE));
    }

    #[test]
    fn the_device_name_is_recognised_and_is_twenty_bytes() {
        assert_eq!(item_len(NAME), Some(NAME_SIZE));
        assert_eq!(NAME.len(), NAME_SIZE);
    }

    #[test]
    fn a_marker_mismatch_is_not_skipped() {
        // Only the exact shape is a journal item: guessing would let a track
        // record be swallowed as configuration.
        let mut wrong_head = entry(3, 50);
        wrong_head[0] = 0xA9;
        assert_eq!(item_len(&wrong_head), None);

        let mut wrong_tail = entry(3, 50);
        wrong_tail[15] = 0xBC;
        assert_eq!(item_len(&wrong_tail), None);

        let mut wrong_name = *b"HOLUXGR241LOGGER    ";
        wrong_name[19] = b'\0';
        assert_eq!(item_len(&wrong_name), None);
    }

    #[test]
    fn a_short_item_is_not_a_journal_item() {
        assert_eq!(item_len(&[]), None);
        assert_eq!(item_len(&[0xAA; 15]), None);
        assert_eq!(item_len(b"HOLUXGR241LOGGER   "), None);
    }
}
