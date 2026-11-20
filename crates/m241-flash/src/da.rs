//! Download Agent driver — **placeholder pending the rest of the disassembly**.
//!
//! The flash path is rebuilt from the vendor binary rather than ported, and the
//! command semantics are documented in `docs/reference/01-da-protocol.md`. This
//! module exists so the rest of the write path can compile against a stable
//! shape, and it refuses to operate until the driver itself is written against
//! that page.
//!
//! It lives in `m241-flash`, not in `m241-core`: the DA writes flash, and a
//! crate whose whole contract is "cannot change the device" must not carry it.
//! See `tools/check_safety.py`, which fails the build if the two crates are ever
//! reversed.
//!
//! # The write is the dangerous half of the protocol
//!
//! `0xD3` records a range and `0xD5` erases each sector of it immediately before
//! writing it (`docs/reference/01-da-protocol.md` §5.6). A range that includes
//! `0x00000000` therefore erases the reset vector. That is why this module is a
//! refusal rather than a partial implementation: a half-correct flash routine is
//! the most dangerous thing this workspace could ship.

use m241_core::Result;
use m241_core::error::Error;

/// The opcode that records the write range.
///
/// `docs/reference/01-da-protocol.md` §5.3, handler `FUN_00101F98`,
/// `0x00101F98`–`0x00102034`. Success is `5A 00`, refusal is a lone `A5`; the
/// flash is not touched by this command, only by the `0xD5` that follows it.
pub const OPCODE_MEM_CMD: u8 = 0xD3;

/// The opcode that erases and writes each sector in turn.
///
/// `docs/reference/01-da-protocol.md` §5.6, handler `FUN_001024B8`,
/// `0x001024B8`–`0x001028E4`. The erase is at `0x0010251C`–`0x00102540`, before
/// the first packet's data is programmed, and repeats per sector boundary.
pub const OPCODE_WRITE: u8 = 0xD5;

/// The opcode that restarts the chip.
///
/// `docs/reference/01-da-protocol.md` §5, jump table entry at `0x00102C98`,
/// tail `0x00101920`–`0x00101944`. It takes no argument and sends **no reply**;
/// a client that waits for one hangs.
pub const OPCODE_RESTART: u8 = 0xD9;

/// Placeholder until the driver is written against the disassembly.
#[derive(Debug)]
pub struct DownloadAgent;

impl DownloadAgent {
    /// Refuse to operate: the behaviour is not established yet.
    ///
    /// # Errors
    ///
    /// Always. There is no partial implementation here on purpose: a plausible
    /// half-correct flash routine is the most dangerous thing this crate could
    /// ship.
    pub fn unsupported() -> Result<()> {
        Err(Error::State(String::from(
            "the download-agent protocol is being re-established from the vendor binary; \
             see docs/reference/01-da-protocol.md",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_is_a_state_error_that_names_the_specification() {
        let err = DownloadAgent::unsupported().expect_err("the DA refuses");
        assert!(matches!(err, Error::State(_)), "got {err}");
        assert!(
            format!("{err}").contains("01-da-protocol.md"),
            "the refusal must name the page that will replace it: {err}"
        );
    }

    #[test]
    fn the_opcodes_are_the_ones_the_disassembly_records() {
        // Pinned so a typo cannot silently turn `0xD5` into some other command
        // the DA would still accept.
        assert_eq!(OPCODE_MEM_CMD, 0xD3);
        assert_eq!(OPCODE_WRITE, 0xD5);
        assert_eq!(OPCODE_RESTART, 0xD9);
    }
}
