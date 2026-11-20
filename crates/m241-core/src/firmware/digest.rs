//! SHA-256, computed in-crate.
//!
//! The firmware profiles identify an image by the hash of its bytes. That is a
//! verification, not a security primitive, and it is the only hash this crate
//! needs; adding a dependency for it would put a second copy of an audit
//! surface into a tool whose whole promise is "it does not write anything odd to
//! your device". The implementation therefore lives here, with the standard
//! test vectors next to it.
//!
//! It is a straight FIPS 180-4 §6.2 implementation: 512-bit message blocks, the
//! 64-round compression function, big-endian length padding. Nothing is
//! optimised — a firmware image is 400 KB and hashed once per command.

/// Length of a SHA-256 digest in bytes.
pub const DIGEST_BYTES: usize = 32;

/// The first 32 bits of the fractional parts of the cube roots of the first 64
/// primes (FIPS 180-4 §4.2.2).
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The first 32 bits of the fractional parts of the square roots of the first
/// eight primes (FIPS 180-4 §5.3.3).
const INITIAL_STATE: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// SHA-256 of `data`.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; DIGEST_BYTES] {
    // Padding: one 0x80 byte, zero bytes up to 56 mod 64, then the bit length
    // as a big-endian u64. The length is taken before padding, of course.
    let bit_length = (data.len() as u64).wrapping_mul(8);
    let mut message = Vec::with_capacity(data.len() + 72);
    message.extend_from_slice(data);
    message.push(0x80);
    // The length field occupies the last eight bytes of a block, so padding
    // targets `len % 64 == 56` — not `len % 56 == 0`, which happens to be right
    // only for messages under 56 bytes.
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = INITIAL_STATE;
    let (blocks, _tail) = message.as_chunks::<64>();
    for block in blocks {
        compress(&mut state, block);
    }

    let mut digest = [0u8; DIGEST_BYTES];
    let (words, _tail) = digest.as_chunks_mut::<4>();
    for (slot, word) in words.iter_mut().zip(state) {
        slot.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// SHA-256 of `data` as 64 lowercase hexadecimal digits.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut text = String::with_capacity(DIGEST_BYTES * 2);
    for byte in sha256(data) {
        text.push(hex_digit(byte >> 4));
        text.push(hex_digit(byte & 0x0F));
    }
    text
}

/// Whether `data` hashes to `expected_hex` (case-insensitive).
#[must_use]
pub fn matches(data: &[u8], expected_hex: &str) -> bool {
    sha256_hex(data).eq_ignore_ascii_case(expected_hex.trim())
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'a' + value - 10),
    }
}

/// The word at `index`, or zero when the caller has walked past the end.
///
/// Every lookup in [`compress`] is `len - k` for some `k <= 16`, so the `None`
/// arm is unreachable by construction. Returning zero rather than indexing is
/// what keeps this function total: the crate forbids indexing outside tests so
/// that a serial failure can never become a panic, and a hash of a user's
/// firmware image is no exception.
fn word_at(words: &[u32], index: usize) -> u32 {
    words.get(index).copied().unwrap_or(0)
}

/// One 64-byte message block against the running state (FIPS 180-4 §6.2.2).
fn compress(state: &mut [u32; 8], block: &[u8]) {
    // The message schedule: the 16 words of the block, extended to 64.
    let mut schedule: Vec<u32> = Vec::with_capacity(64);
    let (words, _tail) = block.as_chunks::<4>();
    for word in words {
        schedule.push(u32::from_be_bytes(*word));
    }
    while schedule.len() < 64 {
        let index = schedule.len();
        let w15 = word_at(&schedule, index - 15);
        let w2 = word_at(&schedule, index - 2);
        let w7 = word_at(&schedule, index - 7);
        let w16 = word_at(&schedule, index - 16);
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        schedule.push(w16.wrapping_add(s0).wrapping_add(w7).wrapping_add(s1));
    }

    let mut working = *state;
    for (round, word) in schedule.iter().enumerate() {
        let [a, b, c, d, e, f, g, h] = working;
        let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ (!e & g);
        let temp1 = h
            .wrapping_add(big_s1)
            .wrapping_add(choice)
            .wrapping_add(K.get(round).copied().unwrap_or(0))
            .wrapping_add(*word);
        let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = big_s0.wrapping_add(majority);
        working = [
            temp1.wrapping_add(temp2),
            a,
            b,
            c,
            d.wrapping_add(temp1),
            e,
            f,
            g,
        ];
    }

    for (slot, value) in state.iter_mut().zip(working) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_message_hashes_to_the_published_digest() {
        assert_eq!(
            sha256_hex(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_two_short_published_vectors_hold() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn the_64_byte_block_boundary_pads_and_hashes() {
        // 64 bytes fills one block exactly, forcing the extra padding block and
        // catching an off-by-one in the `56 mod 64` loop.
        let block = vec![0x61u8; 64];
        assert_eq!(
            sha256_hex(&block),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
    }

    #[test]
    fn a_digest_is_32_bytes_of_lowercase_hex() {
        let digest = sha256(b"");
        assert_eq!(digest.len(), DIGEST_BYTES);
        let text = sha256_hex(b"");
        assert_eq!(text.len(), 64);
        assert!(
            text.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        let expected = sha256_hex(b"abc").to_uppercase();
        assert!(matches(b"abc", &format!(" {expected} ")));
        assert!(!matches(b"abd", &expected));
    }
}
