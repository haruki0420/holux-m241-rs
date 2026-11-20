//! Byte-order and checksum helpers shared by every layer of the protocol.
//!
//! The chip stores memory little-endian but speaks big-endian frames, and it
//! applies a *different* transformation depending on the read width. Getting
//! this wrong produces plausible garbage rather than an error, so both
//! transformations live here, next to their tests, rather than inline.
//!
//! | Opcode | Wire order | Conversion |
//! |---|---|---|
//! | `0xA1` / `0xA2` | 16-bit pairs swapped | [`swap_pairs`] |
//! | `0xAF` | each 32-bit word byte-reversed | [`reverse_words32`] |
//!
//! Both are involutions: applying the same function converts wire to memory and
//! memory to wire, which is why there is no `_to_wire` / `_from_wire` pair.

/// Swap every 16-bit pair: `ab cd` becomes `ba dc`. An odd trailing byte is
/// passed through unchanged.
///
/// This is the transformation for `0xA1` WRITE16 and `0xA2` READ16 payloads.
/// Measured on hardware.
#[must_use]
pub fn swap_pairs(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let (pairs, tail) = out.as_chunks_mut::<2>();
    for [first, second] in pairs.iter_mut() {
        std::mem::swap(first, second);
    }
    let _ = tail; // an odd trailing byte is passed through unchanged
    out
}

/// Reverse each 32-bit word: `01 02 03 04` becomes `04 03 02 01`.
///
/// This is the transformation for `0xAF` READ32, which returns each dword as a
/// big-endian word. A trailing partial word is left alone.
#[must_use]
pub fn reverse_words32(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let (words, tail) = out.as_chunks_mut::<4>();
    for word in words.iter_mut() {
        word.reverse();
    }
    let _ = tail; // a trailing partial word is left alone
    out
}

/// XOR of the big-endian 16-bit words of `data`.
///
/// A trailing odd byte is treated as the high byte of a word, matching how the
/// chip's `0xA4` checksum command walks a region two bytes at a time.
#[must_use]
pub fn checksum_xor16(data: &[u8]) -> u16 {
    let (words, tail) = data.as_chunks::<2>();
    let mut value: u16 = 0;
    for [high, low] in words {
        value ^= u16::from_be_bytes([*high, *low]);
    }
    if let [last] = tail {
        value ^= u16::from(*last) << 8;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_swap_is_its_own_inverse() {
        let original = vec![0x18, 0xf0, 0x9f, 0xe5, 0x42];
        let swapped = swap_pairs(&original);
        assert_eq!(swapped, vec![0xf0, 0x18, 0xe5, 0x9f, 0x42]);
        assert_eq!(swap_pairs(&swapped), original);
    }

    #[test]
    fn word_reversal_keeps_a_partial_tail() {
        let bytes = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        // The final two bytes are not a whole word and stay in order.
        assert_eq!(
            reverse_words32(&bytes),
            vec![0x04, 0x03, 0x02, 0x01, 0x05, 0x06]
        );
    }

    #[test]
    fn a_thumb_vector_table_survives_a_round_trip() {
        // `ldr pc, [pc, #-0x18]` repeated: exactly what a real M-241 dump starts
        // with, and what the firmware base address was identified by.
        let table: Vec<u8> = std::iter::repeat_n([0x18u8, 0xf0, 0x9f, 0xe5], 8)
            .flatten()
            .collect();
        assert_eq!(reverse_words32(&reverse_words32(&table)), table);
    }

    #[test]
    fn checksum_of_empty_is_zero() {
        assert_eq!(checksum_xor16(&[]), 0);
    }

    #[test]
    fn checksum_xors_big_endian_words() {
        assert_eq!(checksum_xor16(&[0x12, 0x34, 0x56, 0x78]), 0x1234 ^ 0x5678);
    }

    #[test]
    fn checksum_treats_an_odd_tail_as_a_high_byte() {
        assert_eq!(checksum_xor16(&[0x12, 0x34, 0x5a]), 0x1234 ^ 0x5a00);
    }

    #[test]
    fn checksum_matches_a_hand_computed_region() {
        // Four bytes chosen so the XOR is easy to verify by hand:
        // 0x0102 ^ 0x0304 = 0x0206
        assert_eq!(checksum_xor16(&[0x01, 0x02, 0x03, 0x04]), 0x0206);
    }
}
