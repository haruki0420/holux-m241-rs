//! The settings/state journal that shares a log block with the track records.
//!
//! A block is not a pure array of track records: the unit also writes a journal
//! of configuration and state changes into the same 64 KiB area, between record
//! runs. Two shapes have been observed:
//!
//! * a **16-byte entry**, `AA`×7 | type (1 byte) | value (`u32` LE) | `BB`×4.
//!   Types 3, 4 and 7 have been seen. Their meanings are **not** established:
//!   the values look like the header's period / threshold / policy words, but
//!   that is inference from the values, not firmware. Type 2 has been suggested
//!   to overwrite the record mask; ⚠️ unconfirmed, and not acted on here.
//! * a **20-byte device name**, `HOLUXGR241LOGGER    `, which separates clusters.
//!
//! Reading journal bytes as records is what used to produce checksum-failing
//! points; skipping them keeps the read aligned with the records either side.

/// Bytes of one settings/state journal entry.
pub const ENTRY_SIZE: usize = 16;

/// Bytes of the device-name string that separates journal clusters.
pub const NAME_SIZE: usize = 20;

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
/// Recognition is by exact shape only. An item with a different marker, or a
/// name whose padding differs, returns `None`; the record reader then fails its
/// per-record checksum and drops the bytes, which is the fail-closed behaviour.
/// A wildcard match here would be how the original bug comes back.
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
