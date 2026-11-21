//! Download Agent driver: the one routine in this workspace that programs flash.
//!
//! The DA is rebuilt from the vendor binary rather than ported, and every
//! constant below cites a function and an instruction range in
//! `docs/reference/01-da-protocol.md` (or a row of
//! `docs/dev/03-hardware-facts.md`). If a citation cannot be written for a
//! value, that value is not written here.
//!
//! The module lives in `m241-flash`, not in `m241-core`: the DA writes flash, and
//! a crate whose contract is to carry no write or erase routines must not carry
//! it. `tools/check_safety.py` fails the build if the two crates are ever
//! reversed (`docs/dev/11-safety-review.md`).
//!
//! # The write is the dangerous half of the protocol
//!
//! `0xD3` records a range and `0xD5` erases each sector of it immediately before
//! programming it (`docs/reference/01-da-protocol.md` §5.6, `0x0010251C`–
//! `0x00102560`). A range that includes `0x00000000` therefore erases the reset
//! vector and the unit stops booting. `0xD3` on its own erases nothing: its
//! handler (`0x00101F98`–`0x00102034`) calls no flash driver.
//!
//! Four things keep that from happening by accident, each with a test that
//! fails when it is removed:
//!
//! 1. **The range is a constant.** [`FLASH_WRITE_START`] and
//!    [`FLASH_WRITE_LENGTH`] are literals checked against their expected values
//!    by [`check_layout`] before any byte goes out, and [`check_layout`] refuses
//!    a start of `0` outright. No public function in this crate takes a range.
//! 2. **A confirmation token must be passed** ([`Confirmation`]). It has one
//!    public constructor, [`Confirmation::granted`], and [`DownloadAgent::write`]
//!    takes one as its first argument and refuses a token that is not given.
//!    That makes the prompt a step every caller goes through; it is a convention
//!    of the call site, not a type-level proof that a person was asked.
//! 3. **The device must still hold the image the write was planned from.**
//!    [`DownloadAgent::verify_device`] compares the whole span, read back by
//!    `m241_core`'s BootROM reader, with the user's own backup, so a backup from
//!    a different unit is refused (`AGENTS.md` §2.3). [`check_image`], which
//!    [`DownloadAgent::write`] always runs, refuses any difference outside
//!    [`PATCH_OFFSETS`] and accepts an identical image.
//! 4. **The write is read back and compared.**
//!    [`PendingWrite::verify_written`] restarts the chip (`0xD9`), re-enters the
//!    BootROM through `m241_core`'s client, and compares the span with the
//!    image. The comparison lives in this crate only because the two byte
//!    vectors are already here; the *read* is `m241_core::protocol::brom`'s and
//!    is never re-implemented (`docs/dev/11-safety-review.md`, review item 3).
//!
//! # Shape: write, then read
//!
//! `m241_core`'s `MtkBootRom` owns its transport, and so does the driver. One
//! owner at a time, in this order:
//!
//! ```text
//!   let mut rom = MtkBootRom::new(open_transport);
//!   DownloadAgent::verify_device(&mut rom, &backup)?;   // 0xA2 reads, no write
//!   let mut da = DownloadAgent::new(rom.into_transport());
//!   let pending = da.write(confirmation, &backup, &image)?;   // 0xD3, 0xD5
//!   let report = pending.verify_written(da, &image, FLASH_WRITE_START, reenter)?;
//! ```
//!
//! `reenter` is the caller's BootROM re-entry step: `--assume-brom` or a
//! power-cycle on a unit, [`crate::simulator::FakeDa::set_modelled`] in a test.
//!
//! # The start address is unresolved, so the gate refuses to write
//!
//! Two documents disagree about where the image begins in the DA's address
//! space and no hardware read settles it:
//!
//! * `docs/reference/01-da-protocol.md` §4.2 speaks of a "firmware base
//!   `0x00200000`" as a DA-side address;
//! * `m241_core::protocol::brom::BASE_ADDR_FIRMWARE_M3318` says the DA addresses
//!   the same flash from `0`, so the image's first byte is DA address `0`.
//!
//! The difference decides everything: a range starting at `0` covers the reset
//! vector. [`FLASH_WRITE_START`] carries the value the `m241_core` citation
//! supports, `0`, and [`check_layout`] refuses it. The wire protocol is written
//! and tested against [`crate::simulator::FakeDa`]; **this crate cannot write
//! flash on a real unit** until `docs/dev/05-verification.md` §1 gains a row
//! that fixes the address. [`check_layout`] refuses to run unless a single
//! constant, [`UNRESOLVED_STARTS`], still lists **both** candidate addresses and
//! [`FLASH_WRITE_START`] is one of them: whichever of the two is put in the
//! constant, the gate keeps refusing until the list itself is emptied, which is
//! the edit the hardware row has to justify.
//!
//! # What is verified and what is not
//!
//! 🧪 Nothing in this module has run against a unit. The whole path is exercised
//! by the fake and unit tests only; the hardware session in
//! `docs/dev/05-verification.md` §1 flashed a unit by hand, outside this code.
//! Nothing here may be reported as measured until that page gains a row.
//!
//! Points the disassembly leaves open are marked 未検証 where they are used:
//!
//! * whether the host must send a byte between the two final `0x5A` of `0xD5`
//!   (`docs/reference/01-da-protocol.md` §7 item 1) — this driver sends nothing;
//! * whether `0xD5`'s per-packet retry makes the DA emit `0x69` twice for one
//!   packet (§7 item 7) — an unexpected byte is a failure, not absorbed;
//! * whether a refused `0xD3` really answers a single byte (§4.1) — the
//!   instruction sequence says so; no unit was ever made to refuse one;
//! * whether `0xD5`'s first sector erase can fail and answer a single `0xA5`
//!   (§5.6) — read from the disassembly; the fake models it, no unit did;
//! * whether the DA answers anything to `0xD9` (§4.5, §8 item 4) — the
//!   disassembly says it never returns; this driver does not read after it;
//! * whether the DA zero-fills the short final packet before programming (§4.4
//!   reads that padding off the *read* serialiser) — the client pads it, which
//!   is accepted either way because the sum covers the padded payload.

use std::time::Duration;

use m241_core::Result;
use m241_core::error::Error;
use m241_core::firmware::image::{
    EFFECTIVE_EPOCH_OFFSET, FAMILY, FAMILY_OFFSET, FAMILY_SIZE, FirmwareImage,
};
use m241_core::firmware::profiles::{PROFILE_V1_13, patched_marker};
use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, MtkBootRom};
use m241_core::protocol::port::Transport;

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
/// the first packet's data is programmed, and repeats per sector boundary
/// (`0x0010268C`–`0x001026BC`). The client sends no erase command of its own.
pub const OPCODE_WRITE: u8 = 0xD5;

/// The opcode that restarts the chip.
///
/// `docs/reference/01-da-protocol.md` §5, jump table entry at `0x00102C98`,
/// tail `0x00101920`–`0x00101944`. It takes no argument and sends **no reply**;
/// a client that waits for one hangs (§4.5, §8 item 4).
pub const OPCODE_RESTART: u8 = 0xD9;

/// `ACK`: the DA accepted the command.
///
/// `docs/reference/01-da-protocol.md` §1, "Bytes used as text markers".
pub const ACK: u8 = 0x5A;

/// `NAK`: the DA refused the command, or a checksum failed.
///
/// `docs/reference/01-da-protocol.md` §1.
pub const NAK: u8 = 0xA5;

/// `CONT`: one write packet arrived and was programmed.
///
/// `docs/reference/01-da-protocol.md` §1 and §5.6 (`0x00102734`–`0x00102738`).
pub const CONT: u8 = 0x69;

/// The second byte of a successful `0xD3` reply.
///
/// `docs/reference/01-da-protocol.md` §4.1: `0x00102018` stores zero in the
/// status cell and `0x00102024` reads it straight back, so the only value that
/// reaches the wire is `0x00`. It is a success marker, not a result code.
pub const MEM_CMD_STATUS_OK: u8 = 0x00;

/// Bytes per `0xD5` packet, sent as the command's `u32` length argument.
///
/// `measured elsewhere:` `docs/dev/03-hardware-facts.md` "Write sequence" — the
/// hardware-tested write used `0x100`-byte packets — and
/// `docs/reference/01-da-protocol.md` §4.3, which records that the DA imposes
/// no maximum and that `0x100` is a host convention. §8 item 9 records that
/// the 406,620-byte image ends on a 92-byte packet.
pub const PACKET_SIZE: usize = 0x100;

/// Start of the span this driver writes, in the **DA's** linear flash view.
///
/// **Settled:** the flash answers the BootROM at `BASE_ADDR_FIRMWARE`
/// (`0x0020_0000`) and the image begins with the vector `18 F0 9F E5` there
/// (`measured elsewhere:` `docs/dev/03-hardware-facts.md` "Firmware base,
/// BootROM view").
///
/// **Not settled:** where the same bytes sit in the DA's view.
/// `m241_core::protocol::brom::BASE_ADDR_FIRMWARE_M3318` is `0x0000_0000` —
/// both prior clients wrote at DA address `0` — while
/// `docs/reference/01-da-protocol.md` §4.2 writes the base as `0x00200000`.
/// No read through the DA's own view has settled it, so this constant carries
/// the `m241_core` citation, `0`, and the ambiguity is left visible instead of
/// resolved by preference.
///
/// ➡️ **This value is refused.** [`check_layout`] refuses every write while
/// [`UNRESOLVED_STARTS`] lists it, and it lists both candidates, so swapping
/// this constant for the other one still refuses. See the module
/// documentation, "The start address is unresolved".
pub const FLASH_WRITE_START: u32 = 0x0000_0000;

/// The DA-side start addresses this crate refuses to write at.
///
/// The two candidates of the paragraph above, `0` and `0x0020_0000`. While both
/// are listed, [`check_layout`] refuses a write at either: the constant cannot
/// be "fixed" by swapping in one of them, only by a change to this list, and
/// that change has to cite the hardware row that settled which one is right.
/// Emptying the list is the act the documentation calls for, so it is one
/// visible edit rather than a silently different constant.
pub const UNRESOLVED_STARTS: [u32; 2] = [0x0000_0000, 0x0020_0000];

/// Bytes this driver writes: the V1.13 image size.
///
/// `measured elsewhere:` `docs/dev/03-hardware-facts.md` "V1.13 size" — 406,620
/// bytes (`0x6345C`), read back twice with the same SHA-256 in
/// `docs/dev/05-verification.md` §1. The value equals
/// `m241_core::firmware::profiles::PROFILE_V1_13.size`; a test pins the two
/// together so the constant cannot drift.
pub const FLASH_WRITE_LENGTH: usize = 0x0006_345C;

/// The inclusive last address of the range, which is what `0xD3` needs.
///
/// `docs/reference/01-da-protocol.md` §4.2 and §8 item 8: `end` is an address,
/// not a length, and must be odd.
pub const FLASH_WRITE_END: u32 = FLASH_WRITE_START + (FLASH_WRITE_LENGTH as u32 - 1);

/// The byte offsets a WNRO patch is allowed to move.
///
/// `measured elsewhere:` `docs/dev/03-hardware-facts.md` "Bytes that change" —
/// `0x402F6` and `0x402F7` (the effective epoch constant, LE32 `315964800` at
/// `0x402F4`) and `0x59C2C` (the final digit of the `V1.13` marker at
/// `0x59C28`).
///
/// The table is those three numbers written out literally, as `AGENTS.md` §2.3
/// asks. They are not free-standing: [`epoch_patch_offsets`] and
/// [`marker_patch_offset`] derive the same offsets from `m241_core`'s reader and
/// patch planner, and [`check_layout`] refuses unless the two agree, so a change
/// to `m241_core` fails a test instead of quietly widening what may be written.
pub const PATCH_OFFSETS: [usize; 3] = [0x402F6, 0x402F7, 0x59C2C];

/// The two bytes of the effective epoch constant that change.
///
/// Derived from `m241_core::firmware::image::EFFECTIVE_EPOCH_OFFSET`: the two
/// bytes above the first of `0x402F4`, which is what
/// `docs/dev/03-hardware-facts.md` "Bytes that change" lists.
#[must_use]
pub fn epoch_patch_offsets() -> [usize; 2] {
    [EFFECTIVE_EPOCH_OFFSET + 2, EFFECTIVE_EPOCH_OFFSET + 3]
}

/// The final digit of the version marker, `V1.13` → `V1.14`.
///
/// Derived from `PROFILE_V1_13.version_marker` and
/// `m241_core::firmware::profiles::patched_marker`. `None` when the marker has
/// no bytes, which cannot happen for V1.13 and is a refusal, never a panic.
#[must_use]
pub fn marker_patch_offset() -> Option<usize> {
    let marker = patched_marker(PROFILE_V1_13.name);
    marker
        .len()
        .checked_sub(1)
        .map(|last| PROFILE_V1_13.version_marker + last)
}

/// How long one DA reply may take.
///
/// Not a protocol constant: `docs/reference/01-da-protocol.md` defines no
/// timing, and `docs/dev/05-verification.md` §2 records that nothing in this
/// repository is a stopwatch measurement. Five seconds matches the BootROM
/// client's own default.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The first four bytes of an M-241 firmware image.
///
/// `measured elsewhere:` `docs/dev/03-hardware-facts.md` "Firmware base, BootROM
/// view": `0x00200000` returns the vector table `18 f0 9f e5 …`. This is the
/// known-intact marker [`DownloadAgent::verify_device`] reads before anything
/// is recorded or erased.
pub const VECTOR: [u8; 4] = [0x18, 0xF0, 0x9F, 0xE5];

/// The evidence that a person asked for this write.
///
/// No `Default`, no public field: the only way to obtain one is
/// [`Confirmation::granted`] — the token [`Confirmation::of_command_line`]
/// returns for `--yes`. [`DownloadAgent::write`] takes one as its first
/// argument and refuses a token whose flag is not set, so "write the firmware"
/// cannot appear in code without a line that says who said so.
///
/// That is a convention of the call site, not a proof: the type carries no
/// information a plain `bool` does not. What it buys is a named step at every
/// call site and one place ([`Self::of_command_line`]) where the flag is
/// turned into a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confirmation {
    /// Always `true`; a token that exists was given.
    given: bool,
}

impl Confirmation {
    /// The word a typed prompt accepts as the confirmation (`AGENTS.md` §2.2).
    pub const WORD: &'static str = "WRITE";

    /// The token for "the caller was told and said yes".
    #[must_use]
    pub fn granted() -> Self {
        Self { given: true }
    }

    /// The `--yes` flag, as it reaches this layer.
    ///
    /// Only a flag that is actually set produces a token. An absent flag is
    /// [`Error::Safety`], so a caller that forgets to check still cannot write.
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when `yes` is false.
    pub fn of_command_line(yes: bool) -> Result<Self> {
        if yes {
            return Ok(Self::granted());
        }
        Err(Error::Safety {
            reason: format!(
                "refusing to write flash without an explicit confirmation (--yes, or {WORD:?} \
                 typed at the prompt)",
                WORD = Self::WORD
            ),
        })
    }

    /// Whether the token is present. Always `true`.
    #[must_use]
    pub fn is_given(self) -> bool {
        self.given
    }
}

/// How far a write got before it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// No packet was programmed: the range was refused, a pre-flight check
    /// stopped the run, or `0xD5`'s first sector erase failed.
    NothingWritten,
    /// Every byte below this **DA address** was programmed. The address is
    /// exclusive, so `UpTo(start + length)` means the whole range.
    UpTo(u32),
}

impl Progress {
    /// How many bytes of the range were programmed, given where it started
    /// ([`PendingWrite::start`]).
    #[must_use]
    pub fn bytes(self, start: u32) -> usize {
        match self {
            Self::NothingWritten => 0,
            Self::UpTo(address) => (address.saturating_sub(start) as usize).min(FLASH_WRITE_LENGTH),
        }
    }
}

/// What to do after a write stopped part-way.
///
/// `docs/dev/11-safety-review.md` review item 4 requires the failure to say how
/// far it got and to name reset + BootROM re-entry. This type produces that
/// paragraph, and [`after_write`] appends it to every error raised once `0xD3`
/// has gone out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialWrite {
    /// How far the accepted packets reached.
    pub progress: Progress,
    /// The address the range started at; `progress.bytes(start)` needs it.
    pub start: u32,
    /// `true` when `0xD9` has been sent, so the chip has been restarted.
    pub restart_sent: bool,
    /// `true` when the write reached the end of the range and the read-back is
    /// what disagreed.
    pub reached_read_back: bool,
}

impl PartialWrite {
    /// The recovery paragraph an error carries.
    #[must_use]
    pub fn recovery(self) -> String {
        let written = self.progress.bytes(self.start);
        let total = FLASH_WRITE_LENGTH;
        let percent = written.saturating_mul(100) / total.max(1);
        let mapped = match self.progress {
            Progress::NothingWritten => String::from("no packet was programmed"),
            Progress::UpTo(address) => format!(
                "packets up to DA address 0x{address:08X} were programmed \
                 ({written} of {total} bytes, {percent}%)"
            ),
        };
        let restart = if self.restart_sent {
            "0xD9 (restart) was sent without waiting for a reply"
        } else {
            "send 0xD9 to restart (it answers nothing; do not wait), then"
        };
        let state = if self.reached_read_back {
            "the whole range was sent but the read-back disagreed"
        } else {
            "the range is now only partly erased and programmed"
        };
        format!(
            "{mapped}; {state}. Next: {restart} re-enter the BootROM with --assume-brom (or \
             power-cycle the unit and retry the four-probe sync) and write your own backup image \
             back. Do not download the log from a partly written unit"
        )
    }
}

/// The result of a write whose packets were all accepted.
///
/// It is **not** yet a success: [`Self::verify_written`] is what turns it into
/// one. A caller that drops the report has written flash and not checked it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingWrite {
    /// The DA address the range started at ([`FLASH_WRITE_START`]).
    pub start: u32,
    /// Bytes programmed.
    pub length: usize,
    /// Packets the DA acknowledged with `0x69`.
    pub packets: usize,
    /// How far the packets reached; `UpTo(start + length)` on success.
    pub progress: Progress,
}

/// A write that was read back through the BootROM and matched the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteReport {
    /// The DA address the range started at ([`FLASH_WRITE_START`]).
    pub start: u32,
    /// Bytes programmed and read back.
    pub length: usize,
    /// Packets the DA acknowledged.
    pub packets: usize,
    /// Bytes read back through the BootROM and compared; the same value as
    /// `length`, which [`PendingWrite::verify_written`] checks separately.
    pub verified: usize,
}

impl PendingWrite {
    /// Restart the chip, re-read the span through `m241_core`'s BootROM client,
    /// and compare it with `image`.
    ///
    /// Takes the transport back from the driver (`da.into_transport()`), sends
    /// `0xD9` on it, and hands it to `m241_core`'s `MtkBootRom` for the read.
    /// The transport therefore has exactly one owner at a time, and the restart
    /// and the read that needs it cannot happen on different ports.
    ///
    /// `da_start` is the address the write used ([`Self::start`]); the BootROM
    /// read is at `BASE_ADDR_FIRMWARE + da_start`. ⚠️ 未検証 that mapping — see
    /// [`FLASH_WRITE_START`].
    ///
    /// `prepare` puts the chip back where a BootROM read works. On a unit that
    /// is the re-entry route (`--assume-brom`, or power-cycle); in a test it is
    /// [`crate::simulator::FakeDa::set_modelled`]. Keeping it a parameter makes
    /// that step explicit instead of hidden in a read helper.
    ///
    /// # Errors
    ///
    /// [`Error::Framing`] carrying the first differing offset when the unit does
    /// not hold `image`, and every error names the recovery step.
    pub fn verify_written<T: Transport>(
        self,
        da: DownloadAgent<T>,
        image: &[u8],
        da_start: u32,
        prepare: impl FnOnce(&mut T),
    ) -> Result<WriteReport> {
        let unfinished = PartialWrite {
            progress: self.progress,
            start: self.start,
            restart_sent: true,
            reached_read_back: true,
        };
        let mut da = da;
        // §4.5 and §8 item 4: `0xD9` never returns and sends no reply, so any
        // read after it would block until the timeout at best.
        da.restart()?;
        let mut rom = MtkBootRom::new(da.into_transport());
        prepare(rom.transport_mut());
        rom.sync().map_err(|err| after_write(err, unfinished))?;
        let read = rom
            .read16(BASE_ADDR_FIRMWARE + da_start, FLASH_WRITE_LENGTH)
            .map_err(|err| after_write(err, unfinished))?;

        let mismatch = read
            .iter()
            .zip(image.iter())
            .position(|(unit, wanted)| unit != wanted);
        if let Some(offset) = mismatch {
            let unit = read.get(offset).copied().unwrap_or(0);
            let wanted = image.get(offset).copied().unwrap_or(0);
            return Err(after_write(
                Error::Framing {
                    what: format!(
                        "read-back differs from the image at offset 0x{offset:X}: the unit holds \
                         0x{unit:02X}, the image has 0x{wanted:02X}"
                    ),
                    offset: Some(offset),
                },
                unfinished,
            ));
        }
        if read.len() != image.len() {
            return Err(after_write(
                Error::Framing {
                    what: format!(
                        "read-back returned {} bytes but the image is {}",
                        read.len(),
                        image.len()
                    ),
                    offset: None,
                },
                unfinished,
            ));
        }
        // `verified` is the length of the comparison above. It equals
        // `length` because the mismatch and length checks both passed; this
        // asserts the invariant, which holds in every build, rather than letting
        // the doc claim an equality nothing checks.
        debug_assert_eq!(read.len(), self.length);
        Ok(WriteReport {
            start: self.start,
            length: self.length,
            packets: self.packets,
            verified: read.len(),
        })
    }
}

/// Drives a DA session over one open transport.
///
/// It owns the transport only for the DA conversation; the read before it and
/// the verification after it are `MtkBootRom`'s, on the same transport (see the
/// module documentation for the sequence).
#[derive(Debug)]
pub struct DownloadAgent<T: Transport> {
    transport: T,
    reply_timeout: Duration,
}

impl<T: Transport> DownloadAgent<T> {
    /// Wrap an open transport at the BootROM baud, past the sync.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    /// Override how long one DA reply may take.
    #[must_use]
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Borrow the transport (tests inspect what went on the wire).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Take the transport back, for the BootROM read-back.
    #[must_use]
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Send `0xD9` and do **not** wait for a reply.
    ///
    /// `docs/reference/01-da-protocol.md` §4.5 and §8 item 4: the handler jumps
    /// to `0x00101920`, sets the reset registers and never returns. The write
    /// path itself never calls this; [`PendingWrite::verify_written`] does,
    /// immediately before the read that needs the restart.
    ///
    /// # Errors
    ///
    /// Whatever the transport returns from writing the single byte.
    pub fn restart(&mut self) -> Result<()> {
        self.transport.write_all(&[OPCODE_RESTART])?;
        self.transport.flush()
    }

    /// Read the span this driver would write and require it to equal `backup`.
    ///
    /// Call this **before** entering the DA, with the BootROM client the caller
    /// already synced. Two cheap checks run first, so a wrong unit is reported
    /// before 400 KB crosses the wire:
    ///
    /// * the vector table starts `18 F0 9F E5` (`measured elsewhere:`
    ///   `docs/dev/03-hardware-facts.md` "Firmware base, BootROM view");
    /// * the family field reads `HOLUX_M-241` (`Vendor images:`
    ///   `m241_core::firmware::image::FAMILY`).
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when the unit is not a Holux M-241 or its flash is not
    /// `backup` byte for byte — for example when the backup came from another
    /// unit. The message names the first differing offset. Nothing is written.
    pub fn verify_device(rom: &mut MtkBootRom<T>, backup: &[u8]) -> Result<()> {
        Self::verify_device_at(rom, backup, FLASH_WRITE_START)
    }

    /// [`Self::verify_device`] with an explicit DA address for the span.
    ///
    /// The BootROM addresses flash at [`BASE_ADDR_FIRMWARE`]; a DA address is
    /// the same flash as seen from zero, so the read adds the BootROM base back.
    /// ⚠️ 未検証 that mapping: see [`FLASH_WRITE_START`].
    ///
    /// # Errors
    ///
    /// As [`Self::verify_device`].
    pub fn verify_device_at(rom: &mut MtkBootRom<T>, backup: &[u8], da_start: u32) -> Result<()> {
        let address = BASE_ADDR_FIRMWARE + da_start;
        let vector = rom.read16(address, VECTOR.len())?;
        if vector != VECTOR {
            return Err(Error::Safety {
                reason: format!(
                    "the unit at 0x{address:08X} reads {vector:02X?}; an M-241 image starts \
                     {VECTOR:02X?}. The vector table is the known-intact marker this write checks \
                     before it erases anything. Nothing was written"
                ),
            });
        }
        let family_address = address + FAMILY_OFFSET as u32;
        let family = rom.read16(family_address, FAMILY_SIZE)?;
        if !family.starts_with(FAMILY.as_bytes()) {
            return Err(Error::Safety {
                reason: format!(
                    "the family field at 0x{family_address:08X} does not read {FAMILY:?}; this \
                     unit does not hold a Holux M-241 image. Nothing was written"
                ),
            });
        }
        let device = rom.read16(address, FLASH_WRITE_LENGTH)?;
        if device != backup {
            let offset = device
                .iter()
                .zip(backup.iter())
                .position(|(unit, saved)| unit != saved)
                .map_or_else(
                    || String::from("the end (lengths differ)"),
                    |at| format!("0x{at:X}"),
                );
            return Err(Error::Safety {
                reason: format!(
                    "the unit's flash differs from the backup at image offset {offset}; the backup \
                     must be the dump of this unit taken before patching. Nothing was written"
                ),
            });
        }
        Ok(())
    }

    /// `0xD3` + `0xD5`: program `image` over the fixed range.
    ///
    /// `backup` is the user's own dump, as [`Self::verify_device`] compared it.
    /// It is used again here for the patch-offset rule, so a caller who skipped
    /// the device check still cannot write an image that changes anything
    /// outside [`PATCH_OFFSETS`].
    ///
    /// `confirmation` is required and checked: a token whose flag was never set
    /// is [`Error::Safety`]. Nothing about the type forces a caller to build it
    /// from a prompt, but every route into this function names the step.
    ///
    /// # Errors
    ///
    /// * [`Error::Safety`] from the confirmation, [`check_layout`] or
    ///   [`check_image`]. Nothing was sent: this runs before `0xD3`.
    /// * [`Error::Rejected`] / [`Error::Framing`] / [`Error::Timeout`] once the
    ///   range is recorded. Each carries [`PartialWrite::recovery`] with how far
    ///   the packets got. `0xD9` is *not* sent here: the restart belongs to
    ///   [`PendingWrite::verify_written`], which sends it immediately before the
    ///   read-back.
    pub fn write(
        &mut self,
        confirmation: Confirmation,
        backup: &[u8],
        image: &[u8],
    ) -> Result<PendingWrite> {
        if !confirmation.is_given() {
            return Err(Error::Safety {
                reason: String::from(
                    "DownloadAgent::write was given a confirmation token that was never granted",
                ),
            });
        }
        check_layout()?;
        self.write_range(backup, FLASH_WRITE_START, image)
    }

    /// The write with an explicit start, used by [`Self::write`] and the tests.
    ///
    /// Private. [`Self::write`] is the only non-test caller and it runs
    /// [`check_layout`] first, which refuses the start address this crate cannot
    /// establish, so a release build has no reachable way to write at a range a
    /// caller chooses. **Every** write goes through the checks in this function:
    /// [`check_image`] against `backup`, which is `AGENTS.md` §2.3's rule, so
    /// `0xD3` cannot go out for a buffer that was never compared with the user's
    /// own dump.
    fn write_range(&mut self, backup: &[u8], start: u32, image: &[u8]) -> Result<PendingWrite> {
        let image = check_image(backup, image)?;
        let mut packets = 0usize;
        let mut progress = Progress::NothingWritten;
        let end = range_end(start);
        let outcome = self
            .record_range(start, end)
            .and_then(|()| self.program(image, start, &mut packets, &mut progress));
        if let Err(err) = outcome {
            let unfinished = PartialWrite {
                progress,
                start,
                restart_sent: false,
                reached_read_back: false,
            };
            return Err(after_write(err, unfinished));
        }
        Ok(PendingWrite {
            start,
            length: FLASH_WRITE_LENGTH,
            packets,
            progress,
        })
    }

    // -- the DA protocol ---------------------------------------------------

    /// `0xD3`: record `start`..=`end` with count 1.
    fn record_range(&mut self, start: u32, end: u32) -> Result<()> {
        let mut frame = Vec::with_capacity(10);
        // §5.3: one count byte, then one pair of big-endian u32 (start, end).
        frame.push(OPCODE_MEM_CMD);
        frame.push(1);
        frame.extend_from_slice(&start.to_be_bytes());
        frame.extend_from_slice(&end.to_be_bytes());
        self.transport.write_all(&frame)?;
        self.transport.flush()?;

        // §4.1: read one byte, and extend to two only when it is `0x5A`. A
        // refusal is a single `A5`; reading a second byte there would consume
        // the first byte of the next reply.
        //
        // 未検証 the one-byte refusal: `0x0010202C` shows it in the instruction
        // sequence, but no unit was ever made to refuse a `0xD3`.
        let first = self.read_byte("0xD3 reply")?;
        if first == NAK {
            return Err(Error::Rejected {
                what: format!(
                    "0xD3 for 0x{start:08X}..=0x{end:08X}: the DA refused the range, so it is not \
                     wholly inside a mapped flash region. The DA keeps whatever ranges an earlier \
                     command recorded"
                ),
            });
        }
        if first != ACK {
            return Err(Error::Framing {
                what: format!("0xD3 answered 0x{first:02X}; expected 0x5A or 0xA5"),
                offset: None,
            });
        }
        let status = self.read_byte("0xD3 status byte")?;
        if status != MEM_CMD_STATUS_OK {
            return Err(Error::Framing {
                what: format!("0xD3 status byte was 0x{status:02X}; the only value is 0x00"),
                offset: None,
            });
        }
        Ok(())
    }

    /// `0xD5`: the length, the two `0x5A`, one packet per `0x69`, two `0x5A`.
    fn program(
        &mut self,
        image: &[u8],
        start: u32,
        packets: &mut usize,
        progress: &mut Progress,
    ) -> Result<()> {
        self.send_write_header()?;
        self.await_first_erase()?;
        for (index, chunk) in image.chunks(PACKET_SIZE).enumerate() {
            self.send_packet(chunk)?;
            self.await_packet_reply(index, start)?;
            *packets = index.saturating_add(1);
            // `UpTo` is exclusive, which is exactly the address the next packet
            // would start at, so `bytes(start)` counts the accepted payloads.
            *progress = Progress::UpTo(
                packet_address(index.saturating_add(1), start)
                    .min(start.saturating_add(FLASH_WRITE_LENGTH as u32)),
            );
        }
        self.expect_final_acks()
    }

    /// `0xD5`'s command byte and the `u32` packet length.
    fn send_write_header(&mut self) -> Result<()> {
        let mut frame = Vec::with_capacity(5);
        frame.push(OPCODE_WRITE);
        frame.extend_from_slice(&packet_length().to_be_bytes());
        self.transport.write_all(&frame)?;
        self.transport.flush()
    }

    /// The `0x5A` before the erase and the one after it.
    ///
    /// §5.6: the first is at `0x00102504`; the second follows the first
    /// sector's erase at `0x00102560`, and a failed erase is a single `0xA5` at
    /// `0x0010253C`-`0x0010254C` that arrives instead of the second `0x5A`. The
    /// client sends no erase command: the erase is inside `0xD5`.
    fn await_first_erase(&mut self) -> Result<()> {
        self.expect_ack("0xD5 first ACK")?;
        let after_erase = self.read_byte("0xD5 ACK after the first sector erase")?;
        if after_erase == NAK {
            return Err(Error::Rejected {
                what: String::from(
                    "0xD5: the DA could not erase the first sector of the range (0xA5 at \
                     0x0010253C-0x0010254C) and restarted",
                ),
            });
        }
        if after_erase != ACK {
            return Err(Error::Framing {
                what: format!(
                    "0xD5 ACK after the first sector erase was 0x{after_erase:02X}, expected 0x5A"
                ),
                offset: None,
            });
        }
        Ok(())
    }

    /// One packet: its payload zero-filled to [`PACKET_SIZE`], then the sum.
    ///
    /// §5.6, §4.4: the raw payload may be shorter than the command's length; a
    /// test pins that it is zero-filled before the sum, which is what the DA's
    /// read serialiser does (§4.4, 未検証 for the write direction).
    fn send_packet(&mut self, chunk: &[u8]) -> Result<()> {
        let mut payload = vec![0u8; PACKET_SIZE];
        payload
            .iter_mut()
            .zip(chunk.iter())
            .for_each(|(slot, byte)| *slot = *byte);
        let mut packet = Vec::with_capacity(PACKET_SIZE + 2);
        packet.extend_from_slice(&payload);
        packet.extend_from_slice(&byte_sum(&payload).to_be_bytes());
        self.transport.write_all(&packet)?;
        self.transport.flush()
    }

    /// The `0x69` that acknowledges one packet, or the error that replaces it.
    fn await_packet_reply(&mut self, index: usize, start: u32) -> Result<()> {
        let reply = self.read_byte(&format!("0xD5 packet {index} reply"))?;
        if reply == NAK {
            // §4.4: a checksum mismatch never produces `0x69`; the handler
            // ends in `0xA5` and a chip restart.
            return Err(Error::Rejected {
                what: format!(
                    "0xD5 packet {index} ({PACKET_SIZE} bytes at DA address 0x{:08X}) was refused",
                    packet_address(index, start)
                ),
            });
        }
        if reply != CONT {
            // 未検証 §7 item 7: a retried packet may yield a second `0x69`.
            // Any other byte means the stream and the client disagree about
            // how many packets were sent, so this stops instead of guessing.
            return Err(Error::Framing {
                what: format!("0xD5 packet {index} answered 0x{reply:02X}, expected 0x69"),
                offset: None,
            });
        }
        Ok(())
    }

    /// The two `0x5A` after the last packet.
    ///
    /// Nothing is sent between them. §7 item 1 has two send sites
    /// (`0x001027F8`, `0x00102898`) and one followed by a receive, and the
    /// hardware-tested client sends nothing there and sees exactly two `0x5A`;
    /// 未検証 whether a byte would also be accepted. There is no constant for
    /// the gap because the only value this client can justify is zero.
    fn expect_final_acks(&mut self) -> Result<()> {
        self.expect_ack("0xD5 final ACK (first)")?;
        self.expect_ack("0xD5 final ACK (second)")
    }

    // -- byte plumbing -----------------------------------------------------

    fn expect_ack(&mut self, what: &str) -> Result<()> {
        let got = self.read_byte(what)?;
        if got == ACK {
            return Ok(());
        }
        Err(Error::Framing {
            what: format!("{what} was 0x{got:02X}, expected 0x5A"),
            offset: None,
        })
    }

    fn read_byte(&mut self, what: &str) -> Result<u8> {
        let mut byte = [0u8; 1];
        let deadline = std::time::Instant::now() + self.reply_timeout;
        loop {
            if self.transport.read(&mut byte)? == 1 {
                return byte.first().copied().ok_or_else(|| Error::Timeout {
                    what: what.to_owned(),
                    timeout: self.reply_timeout.into(),
                });
            }
            if std::time::Instant::now() > deadline {
                return Err(Error::Timeout {
                    what: what.to_owned(),
                    timeout: self.reply_timeout.into(),
                });
            }
        }
    }
}

/// Append the recovery paragraph to anything raised once `0xD3` has gone out.
///
/// `Rejected`, `Framing`, `Timeout` and `Safety` keep their variant and carry
/// the paragraph in their message. Every other variant — `Io`, `Port`,
/// `State`, `Firmware` — becomes [`Error::State`], so the machine-readable kind
/// is lost there. That is deliberate: those are transport or bookkeeping
/// failures whose cause (`std::io::Error`, a bad handle) does not fit the same
/// field, and the caller's next action is the same for all of them — the
/// paragraph's reset and re-entry. A caller that needs the original kind must
/// capture it before the write.
fn after_write(err: Error, unfinished: PartialWrite) -> Error {
    let advice = unfinished.recovery();
    match err {
        Error::Rejected { what } => Error::Rejected {
            what: format!("{what}. {advice}"),
        },
        Error::Framing { what, offset } => Error::Framing {
            what: format!("{what}. {advice}"),
            offset,
        },
        Error::Timeout { what, timeout } => Error::Timeout {
            what: format!("{what}. {advice}"),
            timeout,
        },
        Error::Safety { reason } => Error::Safety {
            reason: format!("{reason}. {advice}"),
        },
        other => Error::State(format!("{other}. {advice}")),
    }
}

/// The inclusive last address of a range of [`FLASH_WRITE_LENGTH`] at `start`.
fn range_end(start: u32) -> u32 {
    start.saturating_add(FLASH_WRITE_LENGTH as u32 - 1)
}

/// The DA address packet `index` starts at: `start + index * PACKET_SIZE`.
fn packet_address(index: usize, start: u32) -> u32 {
    start.saturating_add(u32::try_from(index.saturating_mul(PACKET_SIZE)).unwrap_or(u32::MAX))
}

/// One `0xD5` packet's payload length, as the DA's `u32` argument.
fn packet_length() -> u32 {
    u32::try_from(PACKET_SIZE).unwrap_or(0x100)
}

/// 16-bit sum of the bytes, modulo 65536.
///
/// `docs/reference/01-da-protocol.md` §4.4: `FUN_00101944` accumulates the
/// received bytes (`0x0010199C`–`0x001019A4`, bits 16–17 cleared at
/// `0x001019A4`) and the trailer goes high byte first (`0x001019DC`,
/// `0x001019EC`), so it is a big-endian `u16`.
fn byte_sum(data: &[u8]) -> u16 {
    data.iter()
        .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)))
}

/// Refuse unless the range constants are the values the safety rules pin.
///
/// `AGENTS.md` §2.2 and `docs/dev/11-safety-review.md` review items 1 and 2:
/// the range must be a hard-coded constant, it must be refused when it is
/// anything else, and it must not include address zero — `0xD5` erases every
/// sector the range touches (`0x0010251C`–`0x00102560`), so a range starting at
/// zero destroys the reset vector.
///
/// The start address is unresolved (see the module documentation). This
/// function is where that is enforced, and it refuses **both** candidates in
/// [`UNRESOLVED_STARTS`]: `0` covers the reset vector outright, and
/// `0x0020_0000` is the address the protocol document names but no read through
/// the DA's view has confirmed. A write starts only when the constant is not in
/// that list, which means the list has to be changed on purpose, with the
/// hardware row that settled it.
///
/// # Errors
///
/// [`Error::Safety`] when the start is one of [`UNRESOLVED_STARTS`], when any
/// pinned value has moved, or when the patch table and the readers that define
/// it disagree.
pub fn check_layout() -> Result<()> {
    check_layout_of(
        FLASH_WRITE_START,
        FLASH_WRITE_LENGTH,
        EXPECTED_WRITE_START,
        EXPECTED_WRITE_LENGTH,
    )
}

/// The pinned start, in one place so a test can compare it with the constant.
///
/// `check_layout` used to keep this as a local, which meant "the pin and the
/// shipped constant agree" was only observable through the refusal message —
/// and the unresolved-address stage refuses the shipped start first, so moving
/// this value to the other candidate changed nothing a test could see. Naming
/// it is what makes the pin checkable; the refusal logic is unchanged.
const EXPECTED_WRITE_START: u32 = 0x0000_0000;

/// The pinned image length, for the same reason.
const EXPECTED_WRITE_LENGTH: usize = 0x0006_345C;

/// The body of [`check_layout`], with the constants as arguments.
///
/// Test-only, so a test can vary *one* of the four numbers and see which rule
/// refuses it. The public entry point hard-codes both pairs, so no caller can
/// choose a range: this exists because the stages overlap on the shipped
/// values — the shipped start is in [`UNRESOLVED_STARTS`], so the second stage
/// refuses it before the "is it the expected value" stage is ever asked.
fn check_layout_of(
    write_start: u32,
    write_length: usize,
    expected_start: u32,
    expected_length: usize,
) -> Result<()> {
    // The expected start is a candidate, not a decision: if someone edits it to
    // the other candidate alone, the write is still refused here rather than
    // becoming "the pinned range". Only emptying `UNRESOLVED_STARTS` unlocks a
    // write, and that is the edit the hardware row has to justify.
    if !UNRESOLVED_STARTS.contains(&expected_start) {
        return Err(Error::Safety {
            reason: format!(
                "the expected write start 0x{expected_start:08X} is not one of the unresolved \
                 candidates 0x{UNRESOLVED_STARTS:08X?}; the expected start may only be a candidate \
                 until docs/dev/05-verification.md settles it"
            ),
        });
    }

    if UNRESOLVED_STARTS.contains(&write_start) {
        return Err(Error::Safety {
            reason: format!(
                "the write range starts at 0x{write_start:08X}, which is one of the DA-side \
                 addresses 0x{UNRESOLVED_STARTS:08X?} nobody has established by reading the DA's \
                 own view. If the image begins at DA address 0, a 0xD5 over this range erases the \
                 reset vector and the unit stops booting. This driver refuses every write until \
                 docs/dev/05-verification.md settles which address the DA uses — see the module \
                 documentation, \"The start address is unresolved\"",
            ),
        });
    }
    if write_start != expected_start || write_length != expected_length {
        return Err(Error::Safety {
            reason: format!(
                "the write range is 0x{write_start:08X}+0x{write_length:X}, not the \
                 expected 0x{expected_start:08X}+0x{expected_length:X}; refusing to write an \
                 unpinned range"
            ),
        });
    }
    if FLASH_WRITE_END != range_end(expected_start) {
        return Err(Error::Safety {
            reason: String::from(
                "FLASH_WRITE_END is not start + length - 1; 0xD3 would record the wrong span",
            ),
        });
    }
    if write_length != PROFILE_V1_13.size {
        return Err(Error::Safety {
            reason: format!(
                "the write length {} differs from the V1.13 profile's {}",
                write_length, PROFILE_V1_13.size
            ),
        });
    }
    // The patch table must hold exactly the epoch bytes the reader defines and
    // the marker digit the planner defines, so the three numbers cannot drift.
    let mut expected: Vec<usize> = epoch_patch_offsets().to_vec();
    match marker_patch_offset() {
        Some(offset) => expected.push(offset),
        None => {
            return Err(Error::Safety {
                reason: String::from("the V1.13 marker has no final byte to patch"),
            });
        }
    }
    if expected != PATCH_OFFSETS {
        return Err(Error::Safety {
            reason: format!(
                "the patch table {PATCH_OFFSETS:X?} is not the epoch and marker bytes {expected:X?}"
            ),
        });
    }
    Ok(())
}

/// Refuse unless `image` and `backup` differ only at [`PATCH_OFFSETS`].
///
/// `AGENTS.md` §2.3: "Any image whose differing bytes fall outside the patch
/// offsets of the user's own backup is refused. An image identical to the
/// backup is accepted." That second sentence is part of the rule: a unit
/// already carrying the patch is a legitimate thing to write back, because
/// `0xD5` erases the sectors either way.
///
/// Also refuses a buffer whose length is not [`FLASH_WRITE_LENGTH`] and an
/// image that is not a V1.13 M-241 image: the range is fixed, so neither could
/// be what this driver writes.
///
/// Returns the image it was given, after checking it.
///
/// The return value is the same slice the caller passed, so the signature can
/// steer a release caller into checking before it writes: [`DownloadAgent::write`]
/// can only reach the wire with the slice this function handed back. It does
/// not stop a caller that ignores it — a slice, unlike a newtype, carries no
/// proof — but nothing in this crate offers a write path that skips the check.
///
/// # Errors
///
/// [`Error::Safety`] with the first offending offset when a difference lies
/// outside the table, or with the reason the image or backup is the wrong shape.
pub fn check_image<'a>(backup: &[u8], image: &'a [u8]) -> Result<&'a [u8]> {
    for (label, bytes) in [("backup", backup), ("image", image)] {
        if bytes.len() != FLASH_WRITE_LENGTH {
            return Err(Error::Safety {
                reason: format!(
                    "the {label} is {} bytes; the pinned write range is {FLASH_WRITE_LENGTH} \
                     bytes (V1.13)",
                    bytes.len()
                ),
            });
        }
    }
    let parsed = FirmwareImage::new(image).map_err(|err| Error::Safety {
        reason: format!("the image is too short to hold a firmware header: {err}"),
    })?;
    if !parsed.is_m241() {
        return Err(Error::Safety {
            reason: format!(
                "the image's family field reads {:?}, not {FAMILY:?}",
                parsed.family()
            ),
        });
    }
    if parsed.device_type() != PROFILE_V1_13.device_type {
        return Err(Error::Safety {
            reason: format!(
                "the image's device type is {:?}; the pinned range is {}'s ({:?})",
                parsed.device_type(),
                PROFILE_V1_13.name,
                PROFILE_V1_13.device_type
            ),
        });
    }

    for (offset, (saved, wanted)) in backup.iter().zip(image.iter()).enumerate() {
        if saved == wanted {
            continue;
        }
        if !PATCH_OFFSETS.contains(&offset) {
            return Err(Error::Safety {
                reason: format!(
                    "the image differs from the backup at 0x{offset:X} (0x{saved:02X} -> \
                     0x{wanted:02X}), which is outside the patch offsets {PATCH_OFFSETS:X?}; \
                     only a WNRO patch may be written over a backup"
                ),
            });
        }
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use m241_core::firmware::image::{
        DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, GPS_EPOCH_UNIX,
        GPS_EPOCH_UNIX_WNRO,
    };
    use m241_core::firmware::profiles::VERSION_MARKER_LEN;

    use crate::simulator::{FakeDa, Modelled, enter_download_agent};

    /// The start address the tests drive the wire with.
    ///
    /// ⚠️ **Not established on hardware, and deliberately not the shipped
    /// constant.** Whether the image begins at DA address `0` or at
    /// `0x0020_0000` decides whether `0xD5` erases the reset vector, and no read
    /// through the DA's linear view has been made. The shipped constant is
    /// therefore refused by [`check_layout`]; these bytes only exercise the
    /// protocol on the fake.
    ///
    /// `0x4000` is also chosen so the fake's "nothing recorded" pair `(0, 0)`
    /// can never be mistaken for this range: if it were `0`, a test asserting
    /// `recorded_range() == (0, 0)` after a refusal would pass whether or not
    /// the refusal was recorded.
    const WIRE_START: u32 = 0x0000_4000;
    /// The inclusive end of the test range.
    const WIRE_END: u32 = WIRE_START + (FLASH_WRITE_LENGTH as u32 - 1);

    /// A structurally valid V1.13 image carrying the stock constants.
    ///
    /// Header fields follow `m241_core::firmware::image`: the vector table is
    /// `18 F0 9F E5`, and the family and device fields are NUL-terminated C
    /// strings, so the bytes after them are `0x00`, not `0xFF`.
    fn stock_image() -> Vec<u8> {
        let mut data = vec![0xFFu8; FLASH_WRITE_LENGTH];
        data[..VECTOR.len()].copy_from_slice(&VECTOR);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY_SIZE].fill(0);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
        let device = PROFILE_V1_13.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
        let start = PROFILE_V1_13.version_marker;
        let end = start + VERSION_MARKER_LEN;
        data[start..end].copy_from_slice(PROFILE_V1_13.name.as_bytes());
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
        }
        data
    }

    /// The stock image with the measured three-byte patch applied.
    fn patched_image() -> Vec<u8> {
        let mut data = stock_image();
        let before = GPS_EPOCH_UNIX.to_le_bytes();
        let after = GPS_EPOCH_UNIX_WNRO.to_le_bytes();
        for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
            if old != new {
                data[EFFECTIVE_EPOCH_OFFSET + index] = *new;
            }
        }
        data[PROFILE_V1_13.version_marker + VERSION_MARKER_LEN - 1] = b'4';
        data
    }

    /// How long a reply may take in tests that watch for reads which must not
    /// happen.
    ///
    /// [`REPLY_TIMEOUT`] is five seconds and `read_byte` busy-polls until its
    /// deadline, so a stray read is not just slow — it spins. A fake answers on
    /// the first poll after the command, so anything short is enough: what the
    /// test measures is *how many polls* happened, not how long they took.
    const WATCHED_REPLY_TIMEOUT: Duration = Duration::from_millis(20);

    /// A fake holding `backup` at `WIRE_START`, already in the DA.
    fn fake_in_da(backup: &[u8]) -> FakeDa {
        let mut memory = vec![0xFFu8; WIRE_START as usize];
        memory.extend_from_slice(backup);
        let mut device = FakeDa::with_firmware(&memory);
        let report = enter_download_agent(&mut device);
        assert_eq!(report.first().copied(), Some(0xC0), "the DA banner");
        device
    }

    /// Put a fake in the BootROM, where the read path works.
    fn in_bootrom(mut device: FakeDa) -> FakeDa {
        device.set_modelled(Modelled::BootRom);
        device
    }

    /// The fake's flash window at `WIRE_START`.
    fn window(device: &FakeDa) -> &[u8] {
        let start = WIRE_START as usize;
        device
            .memory()
            .get(start..start + FLASH_WRITE_LENGTH)
            .unwrap_or(&[])
    }

    /// The re-entry step a caller does on hardware, done on the fake.
    fn reenter(device: &mut FakeDa) {
        device.set_modelled(Modelled::BootRom);
    }

    /// Drive the write at the test's own start, so the protocol is exercised
    /// even though the shipped constant is refused.
    ///
    /// `write_range` itself runs [`check_image`]; calling it here with the
    /// backup is the same shape [`DownloadAgent::write`] uses after its layout
    /// check, so the tests go through the real check rather than around it.
    fn write_at_wire_start(
        da: &mut DownloadAgent<FakeDa>,
        backup: &[u8],
        image: &[u8],
    ) -> Result<PendingWrite> {
        da.write_range(backup, WIRE_START, image)
    }

    // -- pinned constants --------------------------------------------------

    #[test]
    fn the_opcodes_are_the_ones_the_disassembly_records() {
        // Pinned so a typo cannot silently turn `0xD5` into another command the
        // DA would still accept.
        assert_eq!(OPCODE_MEM_CMD, 0xD3);
        assert_eq!(OPCODE_WRITE, 0xD5);
        assert_eq!(OPCODE_RESTART, 0xD9);
        assert_eq!((ACK, NAK, CONT), (0x5A, 0xA5, 0x69));
        assert_eq!(MEM_CMD_STATUS_OK, 0x00);
        assert_eq!(PACKET_SIZE, 0x100);
        assert_eq!(VECTOR, [0x18, 0xF0, 0x9F, 0xE5]);
    }

    #[test]
    fn the_shipped_start_is_zero_and_the_layout_check_refuses_it() {
        // The safety rule has no exception: this range includes address zero,
        // so the gate refuses it. A comment promising not to use it would not.
        assert_eq!(FLASH_WRITE_START, 0x0000_0000);
        assert_eq!(FLASH_WRITE_LENGTH, 0x0006_345C);
        assert_eq!(FLASH_WRITE_LENGTH, 406_620);
        assert_eq!(FLASH_WRITE_LENGTH, PROFILE_V1_13.size);
        assert_eq!(FLASH_WRITE_END, 0x0006_345B);
        let err = check_layout().expect_err("start 0 must be refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("reset vector"), "got {err}");
    }

    #[test]
    fn the_layout_check_refuses_both_unresolved_starts() {
        // The start address is not established. `0` covers the reset vector;
        // `0x0020_0000` is the address `01-da-protocol.md` §4.2 names but no
        // read through the DA's view confirms. Whichever one the constant
        // carried, a write must be refused — swapping the constant is exactly
        // the edit that would otherwise turn "refused" into "shipped".
        assert_eq!(
            UNRESOLVED_STARTS,
            [0x0000_0000, 0x0020_0000],
            "both candidates must stay listed; removing one is the act the hardware row has to justify"
        );
        for start in UNRESOLVED_STARTS {
            assert!(
                check_layout().is_err() || start != FLASH_WRITE_START,
                "start 0x{start:08X} must not pass check_layout"
            );
        }
        // The shipped constant is in the list, which is what makes the gate
        // refuse. If someone sets FLASH_WRITE_START to the other candidate,
        // this assertion still holds because the other candidate is listed too.
        assert!(UNRESOLVED_STARTS.contains(&FLASH_WRITE_START));
        // A start that is not in the list and not the pinned value is refused
        // by the second gate, so there is no third address that slips through.
        assert_ne!(WIRE_START, FLASH_WRITE_START);
        assert!(
            !UNRESOLVED_STARTS.contains(&WIRE_START),
            "the test's own start is deliberately outside the candidates"
        );
    }

    #[test]
    fn the_pinned_start_is_the_shipped_start_and_not_the_other_candidate() {
        // The pin and the shipped constant are two numbers that must agree. Only
        // the unresolved-address stage refuses the shipped start, and it accepts
        // *both* candidates, so moving the pin to the other one changes no
        // refusal anywhere — which is why this has to be stated, not deduced
        // from an error message.
        assert_eq!(
            EXPECTED_WRITE_START, FLASH_WRITE_START,
            "the pin must be the shipped start, not the other candidate"
        );
        assert_eq!(EXPECTED_WRITE_LENGTH, FLASH_WRITE_LENGTH);
        assert_eq!(EXPECTED_WRITE_LENGTH, PROFILE_V1_13.size);

        // And the same disagreement, seen through the function: a write start
        // that is not the pin is refused by the pinned-value stage.
        let err = check_layout_of(
            WIRE_START,
            FLASH_WRITE_LENGTH,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("a write start that is not the pin must be refused");
        assert!(
            format!("{err}").contains("unpinned range"),
            "the pinned-value rule must refuse it: {err}"
        );
    }

    #[test]
    fn a_pinned_range_that_disagrees_is_refused_on_its_own() {
        // The shipped start is in `UNRESOLVED_STARTS`, so `check_layout` refuses
        // it at that stage and never reaches "is it the expected value". A test
        // that only calls `check_layout` therefore cannot see a wrong
        // `expected_start`: moving it to the other candidate changes the *first*
        // stage's message, which still looks like a refusal.
        //
        // `check_layout_of` is asked directly, with the shipped start as the
        // expected value — the one case where stage three would agree — and then
        // with a candidate as the expected value and the shipped start as the
        // write. The first call must be refused (the shipped start is still on
        // the list); the second must be refused by the *pinned-value* stage. If
        // `expected_start` inside `check_layout` ever drifts, the shipped
        // constant and the pin stop matching. This test covers the refusal
        // path; the pin itself is compared with the shipped value in
        // `the_pinned_start_is_the_shipped_start_and_not_the_other_candidate`.
        let length = FLASH_WRITE_LENGTH;
        let shipped = FLASH_WRITE_START;

        let err = check_layout_of(shipped, length, shipped, length)
            .expect_err("the shipped start is a candidate");
        assert!(
            format!("{err}").contains("DA-side"),
            "the shipped start must be refused by the unresolved-address rule: {err}"
        );

        // The pin, cross-checked: whichever candidate `check_layout` uses as its
        // expected start, the shipped constant is the other one and the refusal
        // must name the shipped address.
        for expected in UNRESOLVED_STARTS {
            let err = check_layout_of(WIRE_START, length, expected, length)
                .expect_err("a write start that is not the expected one must be refused");
            assert!(
                format!("{err}").contains("unpinned range"),
                "a mismatched pair must be refused by the pinned-value rule: {err}"
            );
        }
    }

    #[test]
    fn the_layout_check_names_the_unresolved_address_in_its_refusal() {
        // The message has to say *why* the shipped constant is refused, so a
        // reader does not "fix" it by pointing at the wrong value. It must not
        // depend on the item number in `11-safety-review.md`, which can be
        // renumbered.
        let err = check_layout().expect_err("the shipped constant is a candidate");
        let text = format!("{err}");
        assert!(text.contains("DA-side"), "got {text}");
        assert!(text.contains("05-verification.md"), "got {text}");
        assert!(
            !text.contains("review item"),
            "the refusal must not cite a document item number: {text}"
        );
    }

    #[test]
    fn the_shipped_write_path_never_reaches_the_wire() {
        // `write` runs `check_layout` first, so with the shipped constant it
        // must fail before the fake sees a byte.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        let err = da
            .write(Confirmation::granted(), &backup, &patched_image())
            .expect_err("the shipped start is refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let device = da.transport_mut();
        assert_eq!(device.accepted_packets(), 0);
        assert!(
            !device.wrote_an_empty_range(),
            "no 0xD5 may arrive without a 0xD3"
        );
    }

    #[test]
    fn write_range_refuses_an_image_outside_the_patch_table() {
        // `AGENTS.md` §2.3 has to hold at the single place that talks to the
        // wire, not at one caller of it: `write_range` runs `check_image`
        // itself, so no caller can reach `0xD3` with an unchecked buffer.
        let backup = stock_image();
        let mut out_of_table = patched_image();
        out_of_table[0x1000] ^= 0xFF;
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        let err = da
            .write_range(&backup, WIRE_START, &out_of_table)
            .expect_err("a change outside the patch table");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let device = da.transport_mut();
        assert_eq!(device.accepted_packets(), 0);
        assert_eq!(
            device.recorded_range(),
            (0, 0),
            "0xD3 must not go out for an image that was never checked"
        );
    }

    #[test]
    fn a_confirmation_token_that_was_not_granted_is_refused() {
        // `is_given()` is checked, not carried along: a token with the flag
        // clear reaches neither `check_layout` nor the wire. No public
        // constructor produces one, so this builds it where it is reachable —
        // the module's own tests — which is exactly the point: the check is the
        // only thing standing between `_confirmation` and the wire.
        let ungranted = Confirmation { given: false };
        assert!(!ungranted.is_given());
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        let err = da
            .write(ungranted, &backup, &backup)
            .expect_err("an ungranted token");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            format!("{err}").contains("never granted"),
            "the refusal says why: {err}"
        );
        let device = da.transport_mut();
        assert_eq!(device.accepted_packets(), 0);
        assert_eq!(device.recorded_range(), (0, 0));
    }

    #[test]
    fn the_write_length_fits_the_part_and_has_the_parity_d3_requires() {
        // `docs/dev/03-hardware-facts.md` "Flash on the retail unit": the DA
        // reports `0x0008_0000`; `0xD3` needs an even start and an odd end.
        let last = u32::try_from(FLASH_WRITE_LENGTH).unwrap_or(u32::MAX) - 1;
        assert!(last < 0x0008_0000);
        assert!(FLASH_WRITE_START.is_multiple_of(2));
        assert!(!FLASH_WRITE_END.is_multiple_of(2));
        assert!(WIRE_START.is_multiple_of(2));
        assert!(!WIRE_END.is_multiple_of(2));
    }

    #[test]
    fn the_patch_table_is_the_measured_three_bytes_and_matches_the_planner() {
        assert_eq!(PATCH_OFFSETS, [0x402F6, 0x402F7, 0x59C2C]);
        assert_eq!(epoch_patch_offsets(), [0x402F6, 0x402F7]);
        assert_eq!(marker_patch_offset(), Some(0x59C2C));
        let data = stock_image();
        let image = FirmwareImage::new(&data).expect("header");
        let plan = m241_core::firmware::patch::plan(&image).expect("stock image");
        let planned: Vec<usize> = plan.edits.iter().map(|edit| edit.offset).collect();
        assert_eq!(planned, PATCH_OFFSETS.to_vec());
    }

    // -- the backup rule ---------------------------------------------------

    #[test]
    fn an_image_identical_to_the_backup_is_accepted() {
        // `AGENTS.md` §2.3: "An image identical to the backup is accepted."
        let backup = stock_image();
        check_image(&backup, &backup).expect("identical");
        let patched = patched_image();
        check_image(&patched, &patched).expect("a patched unit written back");
    }

    #[test]
    fn an_image_that_differs_only_inside_the_patch_table_is_accepted() {
        let backup = stock_image();
        let patched = patched_image();
        let differing: Vec<usize> = backup
            .iter()
            .zip(patched.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(offset, _)| offset)
            .collect();
        assert_eq!(differing, PATCH_OFFSETS.to_vec());
        check_image(&backup, &patched).expect("the measured patch");
    }

    #[test]
    fn an_image_that_differs_outside_the_patch_table_is_refused() {
        let backup = stock_image();
        for offset in [0usize, 0x200, 0x402F8, 0x59C2D, FLASH_WRITE_LENGTH - 1] {
            let mut image = patched_image();
            image[offset] ^= 0xFF;
            let err = check_image(&backup, &image).expect_err("outside the patch table");
            assert!(matches!(err, Error::Safety { .. }), "got {err}");
            assert!(
                format!("{err}").contains(&format!("0x{offset:X}")),
                "the refusal must name the offset: {err}"
            );
        }
    }

    #[test]
    fn an_image_of_the_wrong_length_is_refused() {
        let backup = stock_image();
        let short = &backup[..FLASH_WRITE_LENGTH - 2];
        assert!(matches!(
            check_image(short, short),
            Err(Error::Safety { .. })
        ));
        assert!(matches!(
            check_image(&backup, &[]),
            Err(Error::Safety { .. })
        ));
    }

    #[test]
    fn an_image_that_is_not_a_v1_13_m241_image_is_refused() {
        let backup = patched_image();
        let mut wrong_device = backup.clone();
        wrong_device[DEVICE_TYPE_OFFSET + 6] = b'E';
        assert!(matches!(
            check_image(&backup, &wrong_device),
            Err(Error::Safety { .. })
        ));
        let mut wrong_family = backup.clone();
        wrong_family[FAMILY_OFFSET] = b'X';
        assert!(matches!(
            check_image(&backup, &wrong_family),
            Err(Error::Safety { .. })
        ));
    }

    // -- the confirmation token --------------------------------------------

    #[test]
    fn the_confirmation_token_needs_the_flag() {
        let err = Confirmation::of_command_line(false).expect_err("no flag");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("--yes"), "got {err}");
        assert!(
            Confirmation::of_command_line(true)
                .expect("flag")
                .is_given()
        );
        assert!(Confirmation::granted().is_given());
        assert_eq!(Confirmation::WORD, "WRITE");
    }

    // -- wire arithmetic ---------------------------------------------------

    #[test]
    fn the_packet_trailer_is_the_sum_in_big_endian() {
        // `byte_sum`'s arithmetic, checked against literals.
        assert_eq!(byte_sum(&[0x00, 0xFF]), 0x00FF);
        assert_eq!(byte_sum(&[0xFF, 0xFF, 0x02]), 0x0200);
        assert_eq!(byte_sum(&[]), 0);
        assert_eq!(byte_sum(&[0xFF; 0x100]), 0xFF00);
        assert_eq!(byte_sum(&[0xFF, 0xFF]), 0x01FE);
        // And the order on the wire: high byte first (`0x001019DC`,
        // `0x001019EC`), in the trailer the fake received. The first packet of
        // `stock_image` is 256 bytes of `0xFF`, whose sum is `0xFF00`, so the
        // two bytes must be `FF 00` — a little-endian trailer would be `00 FF`.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        write_at_wire_start(&mut da, &backup, &backup).expect("accepted");
        let device = da.transport_mut();
        let first = device.packets().first().expect("a packet");
        assert_eq!(first.len(), PACKET_SIZE);
        assert_eq!(first.first(), Some(&VECTOR[0]));
        // The sum of that packet, from its bytes: the trailer must carry it
        // high byte first (`0x001019DC`, `0x001019EC`).
        let sum: u16 = first
            .iter()
            .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)));
        assert_eq!(
            device.trailer_of(0),
            Some(sum.to_be_bytes()),
            "the trailer is the payload sum, high byte first"
        );
        assert!(
            sum > 0xFF,
            "this packet's sum must have a non-zero low byte, or byte order is invisible: {sum:X}"
        );
    }

    #[test]
    fn progress_counts_bytes_from_the_start_of_the_range() {
        assert_eq!(Progress::NothingWritten.bytes(WIRE_START), 0);
        assert_eq!(Progress::UpTo(WIRE_START).bytes(WIRE_START), 0);
        assert_eq!(
            Progress::UpTo(WIRE_START + PACKET_SIZE as u32).bytes(WIRE_START),
            PACKET_SIZE
        );
        assert_eq!(
            Progress::UpTo(0xFFFF_FFFF).bytes(WIRE_START),
            FLASH_WRITE_LENGTH
        );
    }

    #[test]
    fn the_recovery_paragraph_names_progress_and_the_restart() {
        let partial = PartialWrite {
            progress: Progress::UpTo(WIRE_START + 0x4000),
            start: WIRE_START,
            restart_sent: false,
            reached_read_back: false,
        };
        let text = partial.recovery();
        assert!(text.contains("16384 of 406620 bytes"), "got {text}");
        assert!(text.contains("0xD9"), "got {text}");
        assert!(text.contains("--assume-brom"), "got {text}");
        assert!(text.contains("backup"), "got {text}");
        let after = PartialWrite {
            restart_sent: true,
            ..partial
        };
        assert!(
            after.recovery().contains("0xD9 (restart) was sent"),
            "a failure after the restart must say so: {}",
            after.recovery()
        );
    }

    // -- against the fake --------------------------------------------------

    #[test]
    fn a_full_write_uses_one_range_and_one_packet_per_0x69_then_verifies() {
        let backup = stock_image();
        let image = patched_image();
        let expected_packets = FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE);
        assert_eq!(expected_packets, 1_589);

        let mut da = DownloadAgent::new(fake_in_da(&backup));
        let pending =
            write_at_wire_start(&mut da, &backup, &image).expect("every packet is accepted");

        assert_eq!(pending.packets, expected_packets);
        assert_eq!(
            pending.progress,
            Progress::UpTo(WIRE_START + FLASH_WRITE_LENGTH as u32)
        );

        {
            let device = da.transport_mut();
            assert_eq!(
                device.recorded_range(),
                (WIRE_START, WIRE_END),
                "0xD3 must record an inclusive (start, end) pair with end = start + len - 1"
            );
            assert_eq!(
                device.accepted_packets(),
                expected_packets,
                "one packet per acknowledged 0x69, no more and no fewer"
            );
            assert!(
                window(device) == image.as_slice(),
                "the fake's flash holds the image the DA programmed"
            );
            assert!(
                !device.restarted(),
                "the write must not send 0xD9: the read-back sends it exactly once"
            );
            assert!(!device.wrote_an_empty_range());
        }

        let report = pending
            .verify_written(da, &image, WIRE_START, reenter)
            .expect("read back");
        assert_eq!(report.verified, FLASH_WRITE_LENGTH);
        assert_eq!(report.verified, report.length);
        assert_eq!(report.packets, expected_packets);
    }

    #[test]
    fn the_read_back_address_is_the_bootrom_view_of_the_da_start() {
        // The client reads the span at `BASE_ADDR_FIRMWARE + da_start`, not at
        // the DA address. The fake records the address it was asked for, so a
        // client that omitted or doubled the base is a wrong address here,
        // not "another convention the fake happens to match".
        let backup = stock_image();
        let mut rom = MtkBootRom::new(in_bootrom(fake_in_da(&backup)));
        rom.sync().expect("sync");
        let read = rom
            .read16(BASE_ADDR_FIRMWARE + WIRE_START, FLASH_WRITE_LENGTH)
            .expect("read the span");
        assert!(
            read == backup,
            "the BootROM view of the DA address holds the same bytes"
        );
        assert_eq!(
            rom.transport_mut().last_read_address(),
            Some(BASE_ADDR_FIRMWARE + WIRE_START),
            "the read must carry the BootROM base"
        );
        assert_ne!(BASE_ADDR_FIRMWARE + WIRE_START, WIRE_START);
    }

    #[test]
    fn a_full_write_consumes_exactly_its_replies() {
        // The two final `0x5A` of 0xD5 are part of the transfer. A client that
        // stops after the last `0x69` leaves them in the buffer, and the next
        // command (here `0xD9`, then the sync probes) reads them as its own.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        write_at_wire_start(&mut da, &backup, &patched_image()).expect("accepted");
        let mut leftover = [0u8; 8];
        let unread = da
            .transport_mut()
            .read(&mut leftover)
            .expect("read the buffer");
        assert_eq!(
            unread, 0,
            "the final 0x5A pair must have been consumed, not left for the restart"
        );
    }

    #[test]
    fn the_final_packet_is_the_short_one_and_is_zero_filled() {
        // §8 item 9: the image ends on a 92-byte packet, and the client pads it
        // to PACKET_SIZE before the sum (§4.4). The padding is observed on the
        // wire — what the fake received — not recomputed from the constants.
        let backup = stock_image();
        let image = patched_image();
        // `stock_image` fills the body with `0xFF`, so a padded zero is
        // distinguishable from a payload byte.
        assert_eq!(image.get(FLASH_WRITE_LENGTH - 92), Some(&0xFF));

        let mut da = DownloadAgent::new(fake_in_da(&backup));
        write_at_wire_start(&mut da, &backup, &image).expect("accepted");
        let device = da.transport_mut();

        let packets = device.packets();
        assert_eq!(packets.len(), FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE));
        let final_packet = packets.last().expect("at least one packet");
        assert_eq!(
            final_packet.len(),
            PACKET_SIZE,
            "every payload is a full packet"
        );
        let (tail, padding) = final_packet.split_at(92);
        assert_eq!(
            tail,
            image.get(FLASH_WRITE_LENGTH - 92..).expect("92 bytes"),
            "the short packet carries the image's last 92 bytes, in order"
        );
        assert!(
            padding.iter().all(|byte| *byte == 0),
            "the client must zero-fill the final packet to PACKET_SIZE: {padding:02X?}"
        );

        // The sum covers the padded payload: the trailer on the wire is the sum
        // of the 256 bytes above, padding included.
        let padded_sum: u16 = final_packet
            .iter()
            .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)));
        let trailer = device
            .last_packet_trailer()
            .expect("the trailer the client sent");
        assert_eq!(u16::from_be_bytes(trailer), padded_sum);
    }

    #[test]
    fn the_restart_does_not_read_a_reply() {
        // §4.5, §8 item 4: `0xD9` never returns and sends nothing. A client that
        // reads after it waits out the timeout at best. The fake queues nothing
        // for the restart, so the assertion is "nothing arrived" — a client
        // that read would block and fail this test on the timeout.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        da.restart().expect("0xD9 goes out");
        assert!(da.transport_mut().restarted(), "0xD9 must have been sent");
        let mut byte = [0u8; 1];
        let read = da.transport_mut().read(&mut byte).expect("read");
        assert_eq!(
            read, 0,
            "0xD9 must not answer; a client may not wait for it"
        );
    }

    #[test]
    fn a_write_stops_before_the_restart_so_the_caller_can_decide() {
        // The write path itself never sends `0xD9`: the restart belongs to
        // `verify_written`, immediately before the read-back. A `0xD9` sent
        // after a failed write would boot the firmware and make the BootROM
        // re-entry the caller had planned harder, not easier.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        da.transport_mut().set_failing_first_erase(true);
        write_at_wire_start(&mut da, &backup, &patched_image()).expect_err("the erase fails");
        assert!(
            !da.transport_mut().restarted(),
            "the write never sends 0xD9"
        );
    }

    #[test]
    fn a_refused_range_is_one_byte_and_a_rejected_error() {
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup).refusing_ranges(true));
        let err = write_at_wire_start(&mut da, &backup, &backup).expect_err("refused");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(format!("{err}").contains("0xD3"), "got {err}");
        let device = da.transport_mut();
        assert_eq!(
            device.accepted_packets(),
            0,
            "nothing may be programmed after a refused range"
        );
        // A one-byte refusal must leave the stream aligned: the next command
        // has to be understood, not shifted by a stray status byte.
        assert_eq!(device.recorded_range(), (0, 0));
    }

    #[test]
    fn the_stream_stays_aligned_after_a_refused_range() {
        // §4.1: a refusal is a **single** `A5`. A client that reads a second
        // byte after it consumes the first byte of the *next* reply and shifts
        // every later frame by one. On the wire the extra read is invisible: an
        // empty buffer answers `Ok(0)` and the next reply starts at the same
        // place. What is observable is the read itself, so the fake counts both
        // the bytes that left and the polls that asked for them.
        let backup = stock_image();
        let mut da =
            DownloadAgent::new(fake_in_da(&backup)).with_reply_timeout(WATCHED_REPLY_TIMEOUT);

        // A successful `0xD3` answers `5A 00` and the `0xD5` that follows draws
        // its own replies. Pin that baseline so the refusal below is compared
        // against a known count, not "the client read something".
        let before = da.transport_mut().read_bytes();
        da.write_range(&backup, WIRE_START, &backup)
            .expect("the first write goes through on the fake");
        let accepted = da.transport_mut().read_bytes().saturating_sub(before);
        assert!(
            accepted > 2,
            "the accepted write reads the range reply and the packet replies; got {accepted}"
        );

        // Refuse every `0xD3`. `0xD5` never goes out: `record_range` returns the
        // `Rejected` first, so the whole command is one byte on the wire and the
        // fake answers it on the very first poll.
        da.transport_mut().set_refusing_ranges(true);
        let refused_before = da.transport_mut().read_bytes();
        let polls_before = da.transport_mut().read_polls();
        let err = write_at_wire_start(&mut da, &backup, &backup).expect_err("refused");
        assert!(
            matches!(err, Error::Rejected { .. }),
            "the refusal must be reported as such, not as a timeout spent waiting for a byte \
             that is not coming: got {err}"
        );
        let device = da.transport_mut();
        assert_eq!(
            device.read_bytes().saturating_sub(refused_before),
            1,
            "a refused 0xD3 is one byte on the wire; reading a status byte after the 0xA5 \
             takes a byte from the next reply (and the 0xD5 would then be shifted)"
        );
        assert_eq!(
            device.read_polls() - polls_before,
            1,
            "the client must read the A5 once and stop: the second read has nothing to take, \
             so it polls until the reply timeout and on a unit would eat the next reply's \
             first byte"
        );
        assert_eq!(
            device.output_len(),
            0,
            "nothing else was queued for the refused command"
        );

        // A refused command records nothing, so the earlier range is still the
        // one in place. Then accept again: the same client must drive the range
        // through cleanly, which is what "the stream is aligned" means.
        assert_eq!(device.recorded_range(), (WIRE_START, WIRE_END));
        device.set_accepting_ranges();
        let mut again =
            DownloadAgent::new(fake_in_da(&backup)).with_reply_timeout(WATCHED_REPLY_TIMEOUT);
        let again_before = again.transport_mut().read_bytes();
        again
            .write_range(&backup, WIRE_START, &backup)
            .expect("a well-formed write after a refusal");
        assert_eq!(
            again.transport_mut().read_bytes() - again_before,
            accepted,
            "the second 0xD3 reads exactly what the first one did: the refusal left no byte \
             behind for it to consume"
        );
        assert_eq!(
            again.transport_mut().recorded_range(),
            (WIRE_START, WIRE_END),
            "the second 0xD3 must be parsed as a whole command after the refusal"
        );
    }

    #[test]
    fn a_failed_first_erase_is_a_rejected_error_with_nothing_written() {
        // §5.6: `0xA5` at `0x0010253C`-`0x0010254C` replaces the second `0x5A`,
        // so no packet follows. The client must report it as a refusal that got
        // nowhere, not as a framing error or as progress.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        da.transport_mut().set_failing_first_erase(true);
        let err = write_at_wire_start(&mut da, &backup, &patched_image())
            .expect_err("the erase is refused");
        let text = format!("{err}");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(text.contains("could not erase"), "got {text}");
        assert!(text.contains("no packet was programmed"), "got {text}");
        assert!(
            text.contains("0xD9"),
            "the recovery step must be named: {text}"
        );
        let device = da.transport_mut();
        assert_eq!(device.accepted_packets(), 0);
        assert!(!device.restarted(), "the write does not restart the chip");
    }

    #[test]
    fn a_packet_refusal_before_any_progress_says_no_packet_was_programmed() {
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup).refusing_packets_from(0, 1_000));
        let err = write_at_wire_start(&mut da, &backup, &patched_image())
            .expect_err("the first packet is refused");
        let text = format!("{err}");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(text.contains("packet 0"), "got {text}");
        assert!(text.contains("no packet was programmed"), "got {text}");
        assert!(text.contains("--assume-brom"), "got {text}");
    }

    #[test]
    fn a_packet_refusal_after_progress_reports_the_bytes_written() {
        // The fake accepts two packets, then refuses; the error must carry
        // "512 of 406620 bytes" and not "no packet".
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup).refusing_packets_from(2, 1_000));
        let err = write_at_wire_start(&mut da, &backup, &patched_image())
            .expect_err("the third packet is refused");
        let text = format!("{err}");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(text.contains("512 of 406620 bytes"), "got {text}");
        assert!(text.contains("packet 2"), "got {text}");
    }

    #[test]
    fn a_read_back_mismatch_is_a_framing_error_that_names_the_offset() {
        let backup = stock_image();
        let image = patched_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        let pending = write_at_wire_start(&mut da, &backup, &image).expect("accepted");
        // One bit that did not take, after the conversation ended.
        da.transport_mut().damage(WIRE_START + 0x1000, 0x01);
        let err = pending
            .verify_written(da, &image, WIRE_START, reenter)
            .expect_err("the read-back disagrees");
        let text = format!("{err}");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(text.contains("0x1000"), "got {text}");
        assert!(text.contains("0xD9 (restart) was sent"), "got {text}");
        assert!(text.contains("backup"), "got {text}");
    }

    #[test]
    fn a_patched_unit_written_over_itself_is_accepted_and_verifies() {
        // §2.3's second sentence: an identical image is accepted, so re-flashing
        // a patched unit does not fail for the wrong reason.
        let patched = patched_image();
        let mut da = DownloadAgent::new(fake_in_da(&patched));
        let pending = write_at_wire_start(&mut da, &patched, &patched).expect("accepted");
        let report = pending
            .verify_written(da, &patched, WIRE_START, reenter)
            .expect("verified");
        assert_eq!(report.verified, FLASH_WRITE_LENGTH);
    }

    #[test]
    fn the_device_check_refuses_a_backup_from_another_unit_before_writing() {
        let backup = stock_image();
        let mut other = backup.clone();
        other[0x1000] ^= 0xFF;
        let mut rom = MtkBootRom::new(in_bootrom(fake_in_da(&backup)));
        rom.sync().expect("sync");
        let err =
            DownloadAgent::verify_device_at(&mut rom, &other, WIRE_START).expect_err("wrong unit");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("0x1000"), "got {err}");
        DownloadAgent::verify_device_at(&mut rom, &backup, WIRE_START).expect("the right unit");
        // The whole-span comparison read the BootROM view of the DA address.
        assert_eq!(
            rom.transport_mut().last_read_address(),
            Some(BASE_ADDR_FIRMWARE + WIRE_START)
        );
    }

    #[test]
    fn the_device_check_refuses_a_unit_whose_vector_table_is_not_an_m241() {
        let mut not_an_m241 = stock_image();
        not_an_m241[0] = 0x42;
        let mut rom = MtkBootRom::new(in_bootrom(fake_in_da(&not_an_m241)));
        rom.sync().expect("sync");
        let err = DownloadAgent::verify_device_at(&mut rom, &not_an_m241, WIRE_START)
            .expect_err("unmapped bus");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("vector table"), "got {err}");
    }

    #[test]
    fn the_device_check_refuses_a_unit_whose_family_field_is_wrong() {
        let mut wrong = stock_image();
        wrong[FAMILY_OFFSET] = b'X';
        let mut rom = MtkBootRom::new(in_bootrom(fake_in_da(&wrong)));
        rom.sync().expect("sync");
        let err = DownloadAgent::verify_device_at(&mut rom, &wrong, WIRE_START)
            .expect_err("not HOLUX_M-241");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("HOLUX_M-241"), "got {err}");
    }
}
