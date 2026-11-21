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
//!    every start other than the settled `0`. No public function in this crate
//!    takes a range.
//! 2. **A confirmation token must be passed** ([`Confirmation`]). It has one
//!    public constructor, [`Confirmation::granted`], and [`DownloadAgent::write`]
//!    takes one as its first argument and refuses a token that is not given.
//!    That makes confirmation a named step at every call site; it only records
//!    that `--yes` was passed via [`Confirmation::of_command_line`] and does
//!    not mechanically verify a reviewer record or session ID (`AGENTS.md` §2.5
//!    defines the procedural gate).
//! 3. **The device must still hold the image the write was planned from.**
//!    [`DownloadAgent::verify_device`] compares the whole span, read back by
//!    `m241_core`'s BootROM reader, with the user's own backup, so a backup from
//!    a different unit is refused (`AGENTS.md` §2.3). [`check_image`], which
//!    [`DownloadAgent::write`] always runs, accepts only an identical image or
//!    an image that differs from the user's own backup at that profile's patch
//!    offsets. There is no second, digest-based route: a vendor release has
//!    per-unit fields different from any one unit's dump, and `AGENTS.md` §2.3
//!    says the backup check must not be weakened. Write your own backup with the
//!    WNRO patch applied.
//! 4. **The write is read back and compared.**
//!    [`PendingWrite::verify_written`] restarts the chip (`0xD9`), hands the
//!    transport to the caller's BootROM re-entry step, and compares the span
//!    with the image. The comparison lives in this crate only because the two
//!    byte vectors are already here; the *read* is `m241_core::protocol::brom`'s
//!    and is never re-implemented (`docs/dev/11-safety-review.md`, review item 3).
//!    The re-entry step can fail (for example when the restarted firmware does
//!    not stream NMEA), and that failure is reported as a read-back that could
//!    not be performed, not as a mismatch.
//!
//! A fifth, added with the loader, is about which opcodes exist at all: **the
//! loader speaks only the BootROM's `0xA1` and `0xA8`, and the only DA opcode in
//! the probe path is the read-only `0xD6`.** `0xD4` (erase a range) and `0xD8`
//! (write a DA-memory half-word) have no client in this crate. Tests drive the
//! loader and the reader against the fake and assert the fake saw no other
//! opcode, which is the check a comment cannot be.
//!
//! # The write plan: what `fw flash --dry-run` can say without a port
//!
//! [`write_plan`] runs the same acceptance rule [`DownloadAgent::write`] does
//! ([`check_image`]) and reports what a write would do — the profile, the
//! device type, the packet count, and how the image differs from the backup —
//! without sending a byte or opening a port. Its start
//! is [`FLASH_WRITE_START`], the settled DA-side base: a plan reports the range a
//! real write would use and cannot unlock a write, which still runs
//! [`check_layout`] and [`DownloadAgent::verify_device`]. It is what makes
//! "would this image be accepted" answerable before a unit is in front of you,
//! and V1.12 / V1.13 / V1.14 are all expressible (see [`profile_patch_offsets`]).
//!
//! # Getting the DA there at all: the loader
//!
//! Everything above assumes the chip is already running the DA. It is not: a
//! power-on leaves the firmware running, `$PMTK180*3B` leaves the BootROM, and a
//! jump to [`BASE_ADDR_DA`] at an address that holds nothing runs whatever the
//! stale RAM there happens to be. [`DaLoader`] is the step between the two — the
//! BootROM's own `0xA1` writes the DA image to the chip's RAM, then `0xA8`
//! starts it.
//!
//! **The load writes RAM, not flash.** The DA image is 14,896 bytes of code in
//! the chip's internal SRAM at `0x00100800`–`0x0010422F`
//! (`docs/reference/01-da-protocol.md` §2, §9); nothing in the sequence reaches
//! a `0xD*` opcode, and the DA is gone at the next power-on. The DA the loader
//! calls up *can* program flash (that is the whole point of loading it), which
//! is why the load still asks for a [`Confirmation`]: what the confirmation
//! covers is the hand-over of the device to a program that does.
//!
//! The image is **not** acceptable from anywhere: [`DaLoader::load`] insists on
//! exactly [`DA_IMAGE_SIZE`] bytes starting with the DA's own first four bytes
//! ([`DA_IMAGE_HEAD`]), then reads the whole image back before it jumps. Those
//! three checks are what turn "a 14,896-byte file" into "the DA v3.8.4 the rest
//! of this crate was written against"; the 20-byte report afterwards is the
//! fourth, and the only one the DA itself contributes.
//!
//! # Reading the flash from the DA's side: `0xD6`
//!
//! [`DownloadAgent::read_range`] is the DA's read path (`docs/reference/01-da-protocol.md`
//! §5.5): three big-endian `u32` — page length, start, total length — then one
//! zero-padded, checksummed page at a time, each answered by the host with `0x5A`
//! to advance or `0xA5` to ask again. It is the only way to read flash through
//! the DA's own address view, and that view is what settled
//! [`FLASH_WRITE_START`].
//!
//! # Shape: load, then read; or write, then read
//!
//! `m241_core`'s `MtkBootRom` owns its transport, and so does the driver. One
//! owner at a time, in this order:
//!
//! ```text
//!   let mut rom = MtkBootRom::new(open_transport);
//!   rom.sync()?;
//!   let mut loader = DaLoader::new(rom.transport_mut());
//!   let id = loader.load(confirmation, &da_bytes)?;        // 0xA1, 0xA8
//!   let mut da = DownloadAgent::new(rom.into_transport());
//!   let page = da.read_range(DA_ADDRESS_TEST, 64)?;         // 0xD6
//!
//!   let mut rom = MtkBootRom::new(open_transport);
//!   DownloadAgent::verify_device(&mut rom, &backup)?;   // 0xA2 reads, no write
//!   let mut da = DownloadAgent::new(rom.into_transport());
//!   let pending = da.write(confirmation, &backup, &image)?;   // 0xD3, 0xD5
//!   let report = pending.verify_written(da, &image, FLASH_WRITE_START, reenter)?;
//! ```
//!
//! `reenter` is the caller's BootROM re-entry step: on a unit the CLI probes
//! for NMEA and sends `$PMTK180*3B` again, a power-cycle followed by
//! `--assume-brom` also works, and a test names [`crate::simulator::FakeDa`]'s
//! mode explicitly. It returns the transport so the BootROM client can take it
//! over.
//!
//! # The start address is settled: the image begins at DA address 0
//!
//! Two documents used to disagree:
//!
//! * `docs/reference/01-da-protocol.md` §4.2 spoke of a "firmware base
//!   `0x00200000`";
//! * `m241_core::protocol::brom::BASE_ADDR_FIRMWARE_M3318` said the DA addresses
//!   the same flash from `0`.
//!
//! A `0xD6` read through the DA's own view settled it on 2026-11-21: DA address
//! `0` returns the vector `18 F0 9F E5` and the first 64 bytes are the vendor
//! V1.13 image's first 64, while DA address `0x0020_0000` returns
//! `02 02 20 42` — the unmapped-bus pattern (`42 20 02 02` byte-reversed), not
//! firmware. The run, its command and its manual checks are recorded in
//! `docs/dev/05-verification.md` §1.
//!
//! The two views therefore address the same image differently: `0` in the DA's
//! view is `0x0020_0000` in the BootROM's. The same number is not the same
//! thing, and `0x00200000` in the DA's view is unmapped even though the BootROM
//! reads firmware there.
//!
//! [`FLASH_WRITE_START`] is `0` and [`UNRESOLVED_STARTS`] is empty. The range
//! includes the reset vector; that is intended, because the image's own first
//! bytes are what gets erased and reprogrammed. What keeps that safe is the
//! image rule: [`check_image`] refuses any difference from the user's backup
//! outside [`PATCH_OFFSETS`], and [`DownloadAgent::verify_device`] refuses a
//! unit whose span does not equal that backup. [`check_layout`] passes only
//! while the list is empty and the start is the settled `0`; a non-empty list,
//! or any other start, refuses every write.
//!
//! # What is verified and what is not
//!
//! 🧪 The **write** path (`0xD3`, `0xD5`, the read-back) has not run against a
//! unit. It is exercised by the fake and unit tests only; the write recorded in
//! `docs/dev/05-verification.md` §1 was a manual session outside this code. The
//! DA load and one `0xD6` page per candidate have run: 2026-11-21,
//! `/dev/ttyUSB0`, `fw probe-da --assume-brom` (same section). That run wrote no
//! flash, only the DA's RAM.
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
//!
//! And for the two additions above:
//!
//! * whether `0xA1` starts one frame per serial chunk or one frame for a whole
//!   block — the client sends one header of [`DA_LOAD_CHUNK`] half-words per
//!   chunk, from [`BASE_ADDR_DA`] upwards, which is the shape
//!   `docs/dev/03-hardware-facts.md` "Write sequence" records for a BootROM
//!   write, and every chunk's echo is checked;
//! * whether the chip emits anything between a `0xA1` write and the next frame
//!   (the echo lags one byte, `AGENTS.md` §4.2) — the client requires the echo of
//!   everything but the last byte of each chunk and drains the rest;
//! * what the four bytes after the DA's twenty-byte banner are — the 2026-11-21
//!   run left four bytes on the line before the first page; the client drains
//!   the line before `0xD6`, so the page starts where the reader expects it, but
//!   the bytes' origin is not established;
//! * the page size the DA emits for a page-length argument other than
//!   [`PROBE_PAGE_SIZE`], and the advance after the final page of a multi-page
//!   read — one page was measured (64 data bytes plus the two-byte trailer) and
//!   only one page was read.

use std::time::Duration;

use m241_core::Result;
use m241_core::error::Error;
use m241_core::firmware::image::{
    EFFECTIVE_EPOCH_OFFSET, FAMILY, FAMILY_OFFSET, FAMILY_SIZE, FirmwareImage,
};
use m241_core::firmware::profiles::{PROFILE_V1_12, PROFILE_V1_13, patched_marker};
use m241_core::protocol::brom::{
    BASE_ADDR_DA, BASE_ADDR_FIRMWARE, Frame, MtkBootRom, OPCODE_JUMP, OPCODE_READ32, OPCODE_WRITE16,
};
use m241_core::protocol::frame::swap_pairs;
use m241_core::protocol::port::Transport;
use m241_core::types::FirmwareProfile;

/// The opcode that records the write range.
///
/// `docs/reference/01-da-protocol.md` §5.3, handler `FUN_00101F98`,
/// `0x00101F98`–`0x00102034`. Success is `5A 00`, refusal is a lone `A5`; the
/// flash is not touched by this command, only by the `0xD5` that follows it.
pub const OPCODE_MEM_CMD: u8 = 0xD3;

/// The opcode that reads a range and sends it one page at a time.
///
/// `docs/reference/01-da-protocol.md` §5.5, handler `FUN_00101E78`,
/// `0x00101E78`–`0x00101F04`. **Read-only**: the handler walks the range and
/// calls the flash driver's read helper, and the erase and program entries are
/// not reachable from it. Arguments are three big-endian `u32`.
pub const OPCODE_READ_RANGE: u8 = 0xD6;

/// The read page's terminator on the host's side: advance to the next page.
///
/// `docs/reference/01-da-protocol.md` §5.5, `0x00101ED4`–`0x00101EF0`: after
/// each page the host sends `0x5A` to advance or `0xA5` to have the same page
/// sent again; any other byte is ignored and the DA keeps waiting.
pub const PAGE_ADVANCE: u8 = ACK;

/// The read page's retry on the host's side: send this page again.
///
/// `docs/reference/01-da-protocol.md` §5.5. Used by a caller who computed the
/// page's checksum and found it wrong, never blindly.
pub const PAGE_RETRY: u8 = NAK;

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
/// **Settled: `0x0000_0000`.** The DA answers a `0xD6` read at address `0` with
/// the firmware vector `18 F0 9F E5`, and its first 64 bytes are the vendor
/// V1.13 image's first 64. The same read at DA address `0x0020_0000` returns
/// `02 02 20 42`, the unmapped-bus pattern (`42 20 02 02` byte-reversed).
/// Measured 2026-11-21 on `/dev/ttyUSB0` with `fw probe-da --assume-brom`; the
/// output is recorded in `docs/dev/05-verification.md` §1.
///
/// The BootROM reads the same image at `BASE_ADDR_FIRMWARE` (`0x0020_0000`).
/// That is a different view, not a different image: the same number means the
/// firmware to the BootROM and unmapped memory to the DA.
///
/// ➡️ The range starts at zero and therefore includes the reset vector. That is
/// intended: those are the image's own first instructions, erased and
/// reprogrammed together with the rest. What keeps it safe is the image rule —
/// [`check_image`] refuses any difference from the user's backup outside
/// [`PATCH_OFFSETS`], and [`DownloadAgent::verify_device`] refuses a unit whose
/// span does not equal that backup. [`check_layout`] passes only while
/// [`UNRESOLVED_STARTS`] is empty and this constant is the settled `0`.
pub const FLASH_WRITE_START: u32 = 0x0000_0000;

/// The DA-side start addresses this crate refuses to write at: none.
///
/// This held the two candidates while the address was unresolved. The 2026-11-21
/// read settled it at `0` (see [`FLASH_WRITE_START`] and
/// `docs/dev/05-verification.md` §1), so the list is empty. It stays in the
/// gate: [`check_layout`] refuses every write when the list is non-empty, so
/// re-adding a candidate closes the gate again instead of being a comment.
pub const UNRESOLVED_STARTS: [u32; 0] = [];

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

/// The byte offsets a WNRO patch is allowed to move in a **V1.13** image.
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
///
/// V1.12 has its own table in [`PATCH_OFFSETS_V1_12`]; the write range is still
/// the V1.13 one, so this constant remains the table [`check_layout`] checks.
pub const PATCH_OFFSETS: [usize; 3] = [0x402F6, 0x402F7, 0x59C2C];

/// The same three offsets for a **V1.12** image.
///
/// The epoch bytes are the same address in both releases (`0x402F4` does not
/// move between them); only the version marker does — `V1.12` sits at
/// `0x59B14`, so its final digit is `0x59B18`. Measured on the vendor image
/// `01017-01B …V1.12….bin`: the two bytes at `0x402F6` / `0x402F7` carry the
/// stock constant and `0x59B18` carries `b'2'`.
///
/// Pinned the same way as [`PATCH_OFFSETS`]: [`profile_patch_offsets`] derives it
/// from `m241_core`'s reader and planner, and
/// `the_v1_12_patch_table_is_the_measured_three_bytes` compares the two.
pub const PATCH_OFFSETS_V1_12: [usize; 3] = [0x402F6, 0x402F7, 0x59B18];

/// The V1.13 write length, spelled out as the literal [`check_layout`] pins.
///
/// 406,620 bytes (`0x6345C`), the V1.13 image size.
pub const V1_13_WRITE_LENGTH: usize = 0x0006_345C;

/// The V1.12 image size, as [`m241_core::firmware::profiles::PROFILE_V1_12`]
/// records it: 406,312 bytes (`0x63328`).
///
/// Confirmed against the vendor package `01017-01B …V1.12….bin`, which is that
/// many bytes and hashes to `PROFILE_V1_12.sha256`. No write range uses it yet —
/// the shipped range is V1.13's — but a plan has to be able to say what a V1.12
/// write would cover.
pub const V1_12_WRITE_LENGTH: usize = 406_312;

/// The byte offsets a WNRO patch is allowed to move in `profile`'s image.
///
/// `None` for a profile this crate has no measured table for. The two tables
/// above are the only ones; a third profile means measuring its bytes first,
/// which is why this returns an option rather than deriving the marker offset
/// from `profile.version_marker` alone.
#[must_use]
pub fn profile_patch_offsets(profile: &FirmwareProfile) -> Option<&'static [usize; 3]> {
    if profile.name == PROFILE_V1_13.name {
        return Some(&PATCH_OFFSETS);
    }
    if profile.name == PROFILE_V1_12.name {
        return Some(&PATCH_OFFSETS_V1_12);
    }
    None
}

/// The length of the write range a plan for `profile` would cover.
///
/// V1.13's is [`V1_13_WRITE_LENGTH`], the shipped range. A patched image is
/// written at its own profile's length — V1.14 is V1.13-based, so it uses the
/// same one — which is the number [`write_plan`] reports and the one an actual
/// write will use once the start address is settled.
#[must_use]
pub fn profile_write_length(profile: &FirmwareProfile) -> usize {
    if profile.name == PROFILE_V1_12.name {
        return V1_12_WRITE_LENGTH;
    }
    V1_13_WRITE_LENGTH
}

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
/// is recorded or erased, and the marker `fw probe-da` compares at each of its
/// candidates. On 2026-11-21 it matched at DA address `0` and did not match at
/// DA address `0x0020_0000` (see [`FLASH_WRITE_START`]).
pub const VECTOR: [u8; 4] = [0x18, 0xF0, 0x9F, 0xE5];

/// Bytes in `DA_v3.8.4.bin`.
///
/// `docs/reference/01-da-protocol.md` §9: the image covers `0x00100800`–`0x0010422F`,
/// which is 14,896 bytes. §2 uses the same figure. `m241 device doctor` checks
/// the same number against the name `DA_v3.8.4.bin`; a test pins the two
/// together so they cannot drift.
pub const DA_IMAGE_SIZE: usize = 14_896;

/// Where the DA image starts on the wire after a [`DaLoader::load`].
///
/// `docs/reference/01-da-protocol.md` §2 and §9: the reset vector is at
/// `0x00100800`–`0x00100828`, and the 20-byte banner starts with the three
/// literal bytes `C0 03 08`. Must equal `m241_core::protocol::brom::BASE_ADDR_DA`;
/// a test pins the two.
pub const DA_LOAD_ADDRESS: u32 = BASE_ADDR_DA;

/// The count word of the `0xA8` jump frame: zero.
///
/// `docs/reference/01-da-protocol.md` §5 and §9 describe `0xA8` as taking an
/// address and nothing else; `docs/dev/03-hardware-facts.md` "DA jump" records
/// the count as **not established**. The one client that ran it on a unit
/// (`python/src/m241/protocol/brom.py`, `BootRom.jump`) sent zero, and zero is
/// the only value this crate can justify: a count the BootROM does not read
/// changes nothing, and one it does read would be an argument nobody measured.
/// Pinned by `the_jump_frame_carries_count_zero_and_the_fake_refuses_any_other`.
pub const DA_JUMP_COUNT: u32 = 0;

/// The first four bytes of `DA_v3.8.4.bin`.
///
/// Read from the vendor image (`holux/M-241 Firmware V1.13 update package/
/// DA_v3.8.4.bin`, 14,896 bytes, SHA-256 `190d7064…9aa`; the full value is
/// recorded in `AGENTS.md` §2.5). The four ARM
/// instructions are `mov r0, r0` (`00 00 A0 E1`, NOP), `mrs r0, CPSR`
/// (`00 00 0F E1`), `mov r0, #C0_32` (`C0 10 A0 E3`) and
/// `orr r0, r0, r1` (`01 00 80 E1`) — `docs/reference/01-da-protocol.md` §2
/// reads the entry at `0x00100800` the same way. It is **not**
/// [`VECTOR`] (`18 F0 9F E5`, the firmware's first instruction): the two
/// images share only `0x00100800` as an address.
///
/// This is a fingerprint of the image at [`DA_LOAD_ADDRESS`], not a protocol
/// constant. [`check_da_image`] compares it so "14,896 bytes" narrows to
/// "the DA v3.8.4 image", before the size check even has a port to write to.
pub const DA_IMAGE_HEAD: [u8; 4] = [0x00, 0x00, 0xA0, 0xE1];

/// The DA version the 20-byte report must carry at bytes 1–2.
///
/// `docs/reference/01-da-protocol.md` §2: `0x00102C40` and `0x00102C48` push the
/// literal bytes `0x03` and `0x08` (held here as a big-endian `u16`), and the
/// hardware table records `DA v3.8.4` answering `C0 03 08 49 …`.
pub const DA_VERSION: u16 = 0x0308;

/// The region id at report byte 3, `0x49` on the retail part.
///
/// `0x00101C0C`–`0x00101C18` send the region entry's `+0x00` byte
/// (`docs/reference/01-da-protocol.md` §2, §6). A wrong value here means the DA
/// came up believing it is driving a different chip, so the loader refuses it.
pub const DA_REGION_ID: u8 = 0x49;

/// The manufacturer id the report must carry (`docs/reference/01-da-protocol.md` §2).
pub const DA_FLASH_MANUFACTURER: u16 = 0x00C2;

/// The device id that goes with [`DA_FLASH_MANUFACTURER`] on the retail part.
pub const DA_FLASH_DEVICE: u16 = 0x22BA;

/// Bytes per `0xA1` frame: ten half-words, [`m241_core::protocol::brom::WRITE_CHUNK`].
///
/// `docs/dev/03-hardware-facts.md` "Write sequence": the measured BootROM write
/// streamed the image in ten-byte serial chunks. Even, so a 14,896-byte image is
/// a whole number of them.
pub const DA_LOAD_CHUNK: usize = m241_core::protocol::brom::WRITE_CHUNK;

/// The two addresses a `0xD6` probe reads, and the only two.
///
/// The probe is a read-only diagnostic, so it still reads both former
/// candidates after the start was settled: `0` is the DA-side base
/// ([`FLASH_WRITE_START`]) and `0x0020_0000` is the BootROM-side base, which is
/// unmapped in the DA's view. The 2026-11-21 run is what established that
/// difference. Keeping both means a later unit can be compared with this one,
/// and no caller chooses the address. `0` is where
/// `m241_core::protocol::brom::BASE_ADDR_FIRMWARE_M3318` says the DA sees the
/// flash; `0x0020_0000` is `docs/reference/01-da-protocol.md` §4.2's base in the
/// BootROM's view.
///
/// Deliberately **not** derived from [`UNRESOLVED_STARTS`]: that list is empty
/// now, while the probe must keep reading both addresses.
pub const PROBE_ADDRESSES: [u32; 2] = [0x0000_0000, 0x0020_0000];

/// The whole of `fw probe-da`'s read path: one `0xD6` page at each candidate.
///
/// The only caller of [`DownloadAgent::read_range`] outside tests, and the only
/// place in the workspace that decides an address for a DA read. Both are fixed
/// by [`PROBE_ADDRESSES`]; no flag, environment variable or caller parameter
/// reaches this list. `docs/dev/11-safety-review.md` records why the probe stays
/// this narrow.
///
/// # Errors
///
/// One entry per address, in [`PROBE_ADDRESSES`] order. An address whose read
/// failed is `Err` with the reader's own error — a timeout, a framing error, a
/// bad checksum after the retries — never silently an empty page.
pub fn read_candidates<T: Transport>(
    da: &mut DownloadAgent<T>,
) -> [Result<Vec<u8>>; PROBE_ADDRESSES.len()] {
    PROBE_ADDRESSES.map(|address| da.read_range(address, PROBE_PAGE_SIZE))
}

/// Bytes per page in a `0xD6` page-length argument: 64.
///
/// A host-chosen value, not a protocol one — `docs/reference/01-da-protocol.md`
/// §4.3 records that even `0xD5`'s length has no maximum in the DA, and §7 item 4
/// leaves `0xD6`'s emitted page size unmeasured for any other argument. 64 is
/// small enough that a page that goes wrong costs one retry and large enough to
/// carry a vector table and its neighbours.
///
/// The value being even is **not** a `0xD6` requirement. The read page's
/// checksum is a byte sum (§4.4, [`byte_sum`]) and the serialiser pads the page
/// to the length asked for; the pair-swap and the half-word units belong to the
/// write path's payload (`0xD5`, `0xA1`), not to this page. §5.5 does not even
/// say whether the DA accepts an odd page length, so the number of choice that
/// a caller may raise to 128 is "who chooses it", not "which values are legal".
///
/// Measured on 2026-11-21: with this argument the DA emitted 64 data bytes plus
/// the two-byte big-endian trailer. The size for other arguments, and the
/// advance after the final page of a multi-page read, are 未検証.
pub const PROBE_PAGE_SIZE: usize = 64;

/// How many times one `0xD6` page with a bad checksum is asked for again.
///
/// Not a protocol constant: the DA has no retry of its own, and
/// `docs/reference/01-da-protocol.md` §5.5 defines the host's `0xA5` re-request
/// without bounding it. Three re-requests is enough for a marginal page and
/// short enough that a wrong cable is reported instead of hammered.
pub const READ_RETRIES: usize = 3;

/// A call-site confirmation token indicating that `--yes` (or an interactive
/// confirmation) was supplied for this operation.
///
/// No `Default`, no public field: the only way to obtain one is
/// [`Confirmation::granted`] — the token [`Confirmation::of_command_line`]
/// returns when `--yes` is `true` (and [`Confirmation::of_probe_command_line`]
/// returns for `fw probe-da --yes`). [`DownloadAgent::write`] takes one as its
/// first argument so confirmation is an explicit step at every call site.
/// [`DaLoader::load`] takes one for the same reason: loading the DA hands the
/// device to a program that can erase and program flash.
///
/// This is strictly a call-site convention, not a proof: the type carries no
/// information a plain `bool` does not, and [`Self::of_command_line`] only
/// checks that `--yes` was passed. It does **not** mechanically verify a
/// reviewer record, approval ledger, or session ID (`AGENTS.md` §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confirmation {
    /// Always `true`; a token that exists was given.
    given: bool,
}

impl Confirmation {
    /// The word a typed prompt accepts as the confirmation (`AGENTS.md` §2.2).
    pub const WORD: &'static str = "WRITE";

    /// Construct the confirmation token indicating that the call-site
    /// confirmation flag was supplied.
    ///
    /// Like [`Self::of_command_line`], this performs no mechanical verification
    /// of an external reviewer record or session ID (`AGENTS.md` §2.5).
    #[must_use]
    pub fn granted() -> Self {
        Self { given: true }
    }

    /// The `--yes` flag, as it reaches this layer.
    ///
    /// Only a flag that is actually set produces a token. An absent flag is
    /// [`Error::Safety`], so a caller that forgets to check still cannot write.
    /// This build has no typed confirmation prompt, so the refusal names
    /// `--yes` only; [`Self::WORD`] records the word `AGENTS.md` §2.2 allows a
    /// future prompt to accept, but no code reads it as input here.
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when `yes` is false.
    pub fn of_command_line(yes: bool) -> Result<Self> {
        if yes {
            return Ok(Self::granted());
        }
        Err(Error::Safety {
            reason: String::from(
                "refusing to write flash without an explicit confirmation: pass --yes. There is \
                 no typed prompt in this build; `AGENTS.md` §2.5 governs who may pass it",
            ),
        })
    }

    /// The `--yes` flag for a command that loads the DA but writes no flash.
    ///
    /// The same shape as [`Self::of_command_line`], and the same refusal when
    /// the flag is absent, because the hand-over itself is the thing being
    /// confirmed: a DA that is talking can program flash on the next command.
    ///
    /// `fw probe-da` has no typed prompt — the flag is its only input — so the
    /// refusal names `--yes` and nothing else. Telling a user to type a word
    /// that no code reads would send them looking for a prompt that does not
    /// exist.
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when `yes` is false.
    pub fn of_probe_command_line(yes: bool) -> Result<Self> {
        if yes {
            return Ok(Self::granted());
        }
        Err(Error::Safety {
            reason: String::from(
                "refusing to load the DA without an explicit confirmation: pass --yes. \
                 (fw probe-da has no typed prompt; --yes writes no flash, it authorises the \
                 hand-over of the unit to the DA in RAM)",
            ),
        })
    }

    /// Whether the token is present. Always `true`.
    #[must_use]
    pub fn is_given(self) -> bool {
        self.given
    }
}

/// What the DA said about itself before it was started.
///
/// Built only by [`DaLoader::load`], from the twenty bytes of
/// `docs/reference/01-da-protocol.md` §2, and only after every field matched the
/// value the rest of this crate expects. Handing one to a caller is the
/// evidence that a DA — the one this crate was written against — answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaLoadId {
    /// The `H` at report bytes 1–2: `0x0308` ([`DA_VERSION`]).
    pub version: u16,
    /// The region id at byte 3 ([`DA_REGION_ID`]).
    pub region: u8,
    /// Total bytes the DA reports for the part (`L` at bytes 4–7), from RAM.
    pub flash_bytes: u32,
    /// The manufacturer id (`H`, [`DA_FLASH_MANUFACTURER`]).
    pub manufacturer: u16,
    /// The device id (`H`, [`DA_FLASH_DEVICE`]).
    pub device: u16,
}

/// Loads the DA image into the chip's RAM and starts it.
///
/// Borrows the transport of a synced [`MtkBootRom`] rather than owning it, so
/// the caller still holds the one object that owns the port and can take it back
/// for the DA conversation afterwards (`Readme` shape, module documentation).
///
/// Nothing here touches flash: [`Self::load`] runs the BootROM's `0xA1` writes
/// and one `0xA8` jump, and every byte it writes is RAM. The question "does this
/// program write flash" is answered by *which* addresses and opcodes it uses,
/// and the module documentation records both.
#[derive(Debug)]
pub struct DaLoader<'a, T: Transport> {
    transport: &'a mut T,
    reply_timeout: Duration,
}

impl<'a, T: Transport> DaLoader<'a, T> {
    /// Wrap a synced BootROM's transport.
    pub fn new(transport: &'a mut T) -> Self {
        Self {
            transport,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    /// Override how long one reply may take.
    #[must_use]
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Check the image's size and first bytes, write it to [`DA_LOAD_ADDRESS`],
    /// read it back, jump to it, and check the DA's report.
    ///
    /// # Errors
    ///
    /// All of these run **before the first byte goes out**; the port is not
    /// touched, so a refusal leaves a unit exactly as it was.
    ///
    /// 1. [`Error::Safety`] when no confirmation token is given.
    /// 2. [`Error::Safety`] when the image is not exactly [`DA_IMAGE_SIZE`]
    ///    bytes.
    /// 3. [`Error::Safety`] when its first four bytes are not [`DA_IMAGE_HEAD`].
    /// 4. [`Error::Framing`] when the write-back disagrees, or when the report
    ///    is not the DA this crate was written against.
    /// 5. [`Error::Timeout`] / [`Error::Io`] from the transport.
    pub fn load(&mut self, confirmation: Confirmation, image: &[u8]) -> Result<DaLoadId> {
        if !confirmation.is_given() {
            return Err(Error::Safety {
                reason: String::from(
                    "DaLoader::load was given a confirmation token that was never granted",
                ),
            });
        }
        check_da_image(image)?;
        self.write_image(image)?;
        self.read_back(image)?;
        self.jump()?;
        let report = self.read_report()?;
        parse_report(&report)
    }

    /// Write `image` in [`DA_LOAD_CHUNK`] frames, each `0xA1` + address + count.
    ///
    /// The wire order of the payload is pair-swapped, exactly as `m241_core`'s
    /// `read16` undoes for a read; the two directions are the same transform
    /// (`docs/reference/01-da-protocol.md` §4.4).
    fn write_image(&mut self, image: &[u8]) -> Result<()> {
        let wire = swap_pairs(image);
        let mut sent = 0usize;
        let mut echo: Vec<u8> = Vec::new();
        for (index, chunk) in wire.chunks(DA_LOAD_CHUNK).enumerate() {
            let offset = index.saturating_mul(DA_LOAD_CHUNK);
            let address = DA_LOAD_ADDRESS
                .checked_add(u32::try_from(offset).unwrap_or(u32::MAX))
                .ok_or_else(|| Error::State(String::from("the DA load address overflows")))?;
            let frame = Frame::for_bytes(OPCODE_WRITE16, address, chunk.len())?;
            let mut block = frame.to_bytes().to_vec();
            block.extend_from_slice(chunk);
            self.transport.write_all(&block)?;
            self.transport.flush()?;
            sent = sent.saturating_add(block.len());

            // A frame is echoed as it arrives, but the echo of the newest byte
            // has not arrived while nothing follows it (`AGENTS.md` §4.2): after
            // N bytes there are N−1. Requiring N would hang on the last chunk;
            // not comparing at all would accept a garbled frame.
            let available = sent.saturating_sub(1);
            if available > echo.len() {
                let want = available - echo.len();
                let mut more = vec![0u8; want];
                self.read_exact(&mut more, &format!("0xA1 echo at 0x{address:08X}"))?;
                echo.extend_from_slice(&more);
            }
            let expected = block
                .get(..block.len().saturating_sub(1))
                .unwrap_or_default();
            if !echo.ends_with(expected) {
                return Err(Error::Framing {
                    what: format!(
                        "the BootROM echo does not repeat the 0xA1 frame for 0x{address:08X}; \
                         the line is not where this load thinks it is"
                    ),
                    offset: Some(offset),
                });
            }
        }
        // The final byte's echo is still in the chip's UART when the caller
        // starts its own frame. Draining it is what keeps the DA's first reply
        // from being read one byte short (the failure is a one-byte shift in
        // every later frame, which looks like a different bug).
        //
        // A failed drain does not mean "something shifted": the drain is
        // post-transfer housekeeping, and the stream that follows (`read_back`,
        // `jump`, the banner) reads from the same buffer, so a byte the drain
        // could not take is still in front of the next read. Consuming it later
        // is not a false success either: `read_back` compares the nine-byte
        // `0xAF` echo against the frame it sent, so anything out of place —
        // an echo that arrived late, a byte the drain missed, a frame that
        // arrived short — is `Framing` there, before the jump. The drain's own
        // failure is therefore reported without a recovery paragraph: no
        // `0xD3` has been sent in this path and nothing has been erased.
        self.transport.clear_input()
    }

    /// Read the image back through `0xAF` and require it byte for byte.
    ///
    /// A RAM write that did not land would otherwise be found only by the jump,
    /// which runs whatever arrived; the comparison here is the last cheap check
    /// before that.
    ///
    /// The frame and the `0xAF` reply transform are `m241_core`'s
    /// (`Frame::for_bytes`, `reverse_words32`); only the send-and-read is here,
    /// because `MtkBootRom` wants to own the transport and the loader borrows it
    /// (the same reason [`DownloadAgent`] talks to `T` directly).
    fn read_back(&mut self, bytes: &[u8]) -> Result<()> {
        let frame = Frame::for_bytes(OPCODE_READ32, DA_LOAD_ADDRESS, bytes.len())?;
        self.transport.write_all(&frame.to_bytes())?;
        self.transport.flush()?;
        let mut echo = [0u8; 9];
        self.read_exact(&mut echo, "0xAF echo after the DA load")?;
        if echo != frame.to_bytes() {
            return Err(Error::Framing {
                what: format!(
                    "the BootROM echoed {echo:02X?} for a 0xAF read of the loaded DA, not the \
                     frame that was sent"
                ),
                offset: None,
            });
        }
        let mut wire = vec![0u8; bytes.len()];
        self.read_exact(&mut wire, "the DA image read back through 0xAF")?;
        let read = m241_core::protocol::frame::reverse_words32(&wire);
        if read == bytes {
            return Ok(());
        }
        let offset = read
            .iter()
            .zip(bytes.iter())
            .position(|(unit, wanted)| unit != wanted)
            .unwrap_or(0);
        Err(Error::Framing {
            what: format!(
                "the DA read-back differs at image offset 0x{offset:X}; the RAM load did not \
                 land, so jumping would run whatever is there. The DA was not started"
            ),
            offset: Some(offset),
        })
    }

    /// `0xA8` to [`DA_LOAD_ADDRESS`], and no echo to read (the chip hands
    /// control over part-way through the frame).
    ///
    /// The frame's count is **0**, not 1. `docs/reference/01-da-protocol.md`
    /// gives no meaning for `0xA8`'s count word, and the only client that ran
    /// this jump on a unit sent zero (`python/src/m241/protocol/brom.py`,
    /// `BootRom.jump`; `03-hardware-facts.md` "DA jump"). A non-zero count is
    /// unmeasured and, if the BootROM reads the field at all, a value it also
    /// used would send control somewhere other than [`DA_LOAD_ADDRESS`] — so
    /// the frame is built by hand rather than through
    /// [`Frame::for_bytes`], whose `1`-byte unit would turn a byte count into
    /// something the protocol defines nowhere.
    fn jump(&mut self) -> Result<()> {
        let frame = Frame {
            opcode: OPCODE_JUMP,
            address: DA_LOAD_ADDRESS,
            count: DA_JUMP_COUNT,
        };
        self.transport.write_all(&frame.to_bytes())?;
        self.transport.flush()
    }

    /// The twenty-byte report, after skipping what the DA sends before it.
    ///
    /// The banner is emitted by the command loop's prologue
    /// (`docs/reference/01-da-protocol.md` §2) and `0x7D` is sent once while the
    /// DA waits to be started, so the first byte is searched for rather than
    /// assumed. [`REPORT_SCAN_LIMIT`] bounds the search.
    fn read_report(&mut self) -> Result<Vec<u8>> {
        let mut seen: Vec<u8> = Vec::with_capacity(REPORT_SCAN_LIMIT);
        let mut byte = [0u8; 1];
        while seen.len() < REPORT_SCAN_LIMIT {
            self.read_exact(&mut byte, "the DA's first report byte")?;
            seen.push(byte[0]);
            if seen.len() >= DA_REPORT_SIZE
                && seen.get(seen.len() - DA_REPORT_SIZE).copied() == Some(DA_REPORT_MARKER)
            {
                let start = seen.len() - DA_REPORT_SIZE;
                return Ok(seen.get(start..).unwrap_or(&[]).to_vec());
            }
            if seen.len() >= DA_REPORT_SIZE && seen.first().copied() == Some(DA_REPORT_MARKER) {
                return Ok(seen.get(..DA_REPORT_SIZE).unwrap_or(&[]).to_vec());
            }
        }
        Err(Error::Framing {
            what: format!(
                "no 0x{DA_REPORT_MARKER:02X} report from the DA within {REPORT_SCAN_LIMIT} bytes \
                 after the jump; the chip may not have started the image that was written"
            ),
            offset: None,
        })
    }

    fn read_exact(&mut self, buf: &mut [u8], what: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + self.reply_timeout;
        let mut filled = 0usize;
        while filled < buf.len() {
            let Some(rest) = buf.get_mut(filled..) else {
                break;
            };
            filled += self.transport.read(rest)?;
            if std::time::Instant::now() > deadline {
                return Err(Error::Timeout {
                    what: format!("{what} ({filled} of {} bytes)", buf.len()),
                    timeout: self.reply_timeout.into(),
                });
            }
        }
        Ok(())
    }
}

/// Bytes in the DA's info report (`docs/reference/01-da-protocol.md` §2).
pub const DA_REPORT_SIZE: usize = 20;

/// The report's first byte: `0xC0` (`0x00102C38`).
pub const DA_REPORT_MARKER: u8 = 0xC0;

/// How many bytes after the jump may be searched for [`DA_REPORT_MARKER`].
///
/// Not a protocol constant. The only known noise is the single `0x7D` the DA
/// sends while it waits to be started (§1), so a page is already generous; the
/// bound exists so a wrong image cannot make the loader read forever.
pub const REPORT_SCAN_LIMIT: usize = 256;

/// Refuse an image that is not the DA the rest of this crate is written for.
///
/// Two checks, in this order, and both are cheap enough to run before a port is
/// opened:
///
/// 1. the file is exactly [`DA_IMAGE_SIZE`] bytes — the size check
///    `m241 device doctor` makes against the file name;
/// 2. its first four bytes are [`DA_IMAGE_HEAD`], the vendor image's first
///    instruction (`mov r0, r0`), so a 14,896-byte file that is not the DA is
///    refused before anything is written to RAM.
///
/// Content is not hashed **yet**. The DA is a user's own copy of a vendor
/// binary, never committed here, but its SHA-256 is known (the full value is in
/// `AGENTS.md` §2.5), so the hash can be checked without committing the image.
/// A four-byte head still narrows "the right size" to "the right image"; adding
/// the full SHA-256 comparison here is follow-up W2a, and until then the
/// approval rule requires the approver to compare it manually. What makes the
/// load safe in the meantime is still the steps after it
/// ([`parse_report`] refusing any other version, flash part or region, and
/// [`DaLoader::read_back`] refusing to jump at RAM that did not take the write).
///
/// # Errors
///
/// [`Error::Safety`] for a wrong length, an empty image, or first bytes that are
/// not [`DA_IMAGE_HEAD`].
pub fn check_da_image(image: &[u8]) -> Result<()> {
    if image.len() != DA_IMAGE_SIZE {
        return Err(Error::Safety {
            reason: format!(
                "the DA image is {} bytes; DA_v3.8.4.bin is exactly {DA_IMAGE_SIZE} bytes \
                 (0x00100800–0x0010422F, docs/reference/01-da-protocol.md §9). Nothing was written",
                image.len()
            ),
        });
    }
    if !image.starts_with(&DA_IMAGE_HEAD) {
        let head: Vec<u8> = image.iter().copied().take(DA_IMAGE_HEAD.len()).collect();
        return Err(Error::Safety {
            reason: format!(
                "the DA image starts {head:02X?}; DA_v3.8.4.bin starts {DA_IMAGE_HEAD:02X?} \
                 (00 00 A0 E1, mov r0, r0). The size alone does not make a file the DA. Nothing \
                 was written"
            ),
        });
    }
    Ok(())
}

/// Check a 20-byte report and turn it into a [`DaLoadId`].
///
/// # Errors
///
/// [`Error::Framing`] when a field is not the value this crate expects, with the
/// field named so a wrong DA is not mistaken for a wrong cable.
pub fn parse_report(report: &[u8]) -> Result<DaLoadId> {
    if report.len() != DA_REPORT_SIZE {
        return Err(Error::Framing {
            what: format!(
                "the DA report is {} bytes; the banner is {DA_REPORT_SIZE}",
                report.len()
            ),
            offset: None,
        });
    }
    let marker = report.first().copied().unwrap_or(0);
    if marker != DA_REPORT_MARKER {
        return Err(Error::Framing {
            what: format!("the DA report starts 0x{marker:02X}, not 0x{DA_REPORT_MARKER:02X}"),
            offset: None,
        });
    }
    let version = be_u16_at(report, 1);
    let region = report.get(3).copied().unwrap_or(0);
    let flash_bytes = be_u32_at(report, 4);
    let manufacturer = be_u16_at(report, 8);
    let device = be_u16_at(report, 10);
    let id = DaLoadId {
        version,
        region,
        flash_bytes,
        manufacturer,
        device,
    };
    if version != DA_VERSION {
        return Err(Error::Framing {
            what: format!(
                "the DA reports version 0x{version:04X}; this crate drives 0x{DA_VERSION:04X} \
                 (DA v3.8.4)"
            ),
            offset: None,
        });
    }
    if region != DA_REGION_ID {
        return Err(Error::Framing {
            what: format!(
                "the DA reports flash region 0x{region:02X}; the retail M-241 is \
                 0x{DA_REGION_ID:02X}"
            ),
            offset: None,
        });
    }
    if manufacturer != DA_FLASH_MANUFACTURER || device != DA_FLASH_DEVICE {
        return Err(Error::Framing {
            what: format!(
                "the DA reports flash {manufacturer:04X}:{device:04X}; the MX29LV400CB is \
                 {DA_FLASH_MANUFACTURER:04X}:{DA_FLASH_DEVICE:04X}"
            ),
            offset: None,
        });
    }
    Ok(id)
}

fn be_u16_at(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..at.saturating_add(2))
        .and_then(|slice| <[u8; 2]>::try_from(slice).ok())
        .map_or(0, u16::from_be_bytes)
}

fn be_u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at.saturating_add(4))
        .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        .map_or(0, u32::from_be_bytes)
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
    /// ([`PendingWrite::start`]) and how long it is ([`PendingWrite::length`]).
    ///
    /// `length` is the range's, not a constant: it was [`FLASH_WRITE_LENGTH`]
    /// while a write could only be V1.13's, and a plan can now be made for more
    /// than one profile.
    #[must_use]
    pub fn bytes(self, start: u32, length: usize) -> usize {
        match self {
            Self::NothingWritten => 0,
            Self::UpTo(address) => (address.saturating_sub(start) as usize).min(length),
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
    /// The address the range started at; `progress.bytes(start, length)` needs it.
    pub start: u32,
    /// How many bytes the range covers ([`PendingWrite::length`]).
    pub length: usize,
    /// `true` when `0xD9` has been sent, so the chip has been restarted.
    pub restart_sent: bool,
    /// `true` once the BootROM sync has answered, immediately before the read.
    ///
    /// `false` after `0xD9` means the re-entry itself failed, so the read-back
    /// never ran; the recovery text must not call that "the read-back disagreed".
    pub reached_read_back: bool,
}

impl PartialWrite {
    /// The recovery paragraph an error carries.
    #[must_use]
    pub fn recovery(self) -> String {
        let written = self.progress.bytes(self.start, self.length);
        let total = self.length;
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
        // Three different states, not two: the write may have stopped part-way,
        // the restart may have happened but the BootROM re-entry failed, or the
        // read may have run and disagreed. Telling a user whose write completed
        // that the range is "partly erased" is false and sends them to the
        // wrong recovery step.
        let state = if written < total {
            "the range is now only partly erased and programmed"
        } else if self.reached_read_back {
            "the whole range was sent, but the read-back did not confirm it"
        } else {
            "the whole range was sent, but the BootROM read-back could not be reached"
        };
        format!(
            "{mapped}; {state}. Next: {restart} re-enter the BootROM with --assume-brom (or \
             power-cycle the unit and retry the four-probe sync) and write your own backup image \
             back. After a power-cycle, `m241 fw dump` shows what the unit actually holds; use \
             it to confirm the read-back before retrying. Do not download the log from a partly \
             written unit"
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
    /// read is at `BASE_ADDR_FIRMWARE + da_start`. The mapping was settled on
    /// 2026-11-21: DA address `0` holds the image the BootROM reads at
    /// `0x0020_0000` — see [`FLASH_WRITE_START`].
    ///
    /// `prepare` performs the BootROM re-entry and returns the transport. On a
    /// unit that is the NMEA probe followed by `$PMTK180*3B` (what the CLI
    /// does), or a power-cycle and `--assume-brom`; in a test it is an explicit
    /// [`crate::simulator::FakeDa::set_modelled`]. Keeping it a parameter makes
    /// that step visible instead of hidden in a read helper, and returning a
    /// `Result` lets a failed re-entry stop the read with its own error rather
    /// than a bare sync timeout.
    ///
    /// # Errors
    ///
    /// [`Error::Framing`] carrying the first differing offset when the unit does
    /// not hold `image`; the error returned by `prepare` when the BootROM cannot
    /// be re-entered; and every error names the recovery step. A re-entry
    /// failure is reported as a read-back that could not be reached, not as a
    /// mismatch.
    pub fn verify_written<T: Transport>(
        self,
        da: DownloadAgent<T>,
        image: &[u8],
        da_start: u32,
        prepare: impl FnOnce(T) -> Result<T>,
    ) -> Result<WriteReport> {
        let unfinished = PartialWrite {
            progress: self.progress,
            start: self.start,
            length: self.length,
            restart_sent: true,
            reached_read_back: false,
        };
        let mut da = da;
        // §4.5 and §8 item 4: `0xD9` never returns and sends no reply, so any
        // read after it would block until the timeout at best.
        da.restart()?;
        let transport = prepare(da.into_transport()).map_err(|err| after_write(err, unfinished))?;
        let mut rom = MtkBootRom::new(transport);
        rom.sync().map_err(|err| after_write(err, unfinished))?;
        let read_back = PartialWrite {
            reached_read_back: true,
            ..unfinished
        };
        let read = rom
            .read16(BASE_ADDR_FIRMWARE + da_start, self.length)
            .map_err(|err| after_write(err, read_back))?;

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
                read_back,
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
                read_back,
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

    /// `0xD6`: read `length` bytes at DA address `address`, one page at a time.
    ///
    /// `length` is in whole [`PROBE_PAGE_SIZE`] pages (that is what the checks
    /// below accept); `fw probe-da` reads one page per candidate through
    /// [`read_candidates`], which is the only non-test caller.
    ///
    /// Arguments are three big-endian `u32` — the page length, the start
    /// address, the total length (`docs/reference/01-da-protocol.md` §5.5,
    /// `0x00101E78`–`0x00101E90`). Each page arrives through the same serialiser
    /// as `0xD5`'s packets, so it is `page` bytes (zero-padded) followed by a
    /// two-byte big-endian byte sum (§4.4). After each page the host sends
    /// [`PAGE_ADVANCE`] to take the next one, or [`PAGE_RETRY`] to have the same
    /// one sent again; when the range is complete the DA sends **nothing** and
    /// goes back to its command loop, so the last page's advance is the last
    /// byte in either direction.
    ///
    /// Read-only: the handler walks the range and sends what it reads. No alarm
    /// range is recorded, nothing is erased, and nothing is programmed.
    ///
    /// A page whose checksum does not match is asked for again, up to
    /// [`READ_RETRIES`] times, and then refused: the DA's read path has no
    /// retry of its own, so a page that arrives wrong twice is a cable problem
    /// rather than a protocol one.
    ///
    /// The line is cleared before the command goes out: a byte that arrived
    /// earlier cannot be this command's answer. The 2026-11-21 run left four
    /// bytes on the line after the twenty-byte report and before the first page;
    /// their origin is not established (`docs/dev/05-verification.md` §1), so
    /// stale bytes are treated defensively on either side of the command: when a
    /// page fails its checksum, [`Self::read_page`] also clears the line before
    /// asking for it again, so leftover prefix bytes cannot shift every retry.
    ///
    /// # Errors
    ///
    /// * [`Error::State`] when `length` is zero or is not a whole number of
    ///   [`PROBE_PAGE_SIZE`] pages, or the range overflows the address space.
    /// * [`Error::Framing`] when a page's checksum fails after the retries, or
    ///   the page length the DA emits is not the one asked for.
    /// * [`Error::Timeout`] / [`Error::Io`] from the transport.
    pub fn read_range(&mut self, address: u32, length: usize) -> Result<Vec<u8>> {
        if length == 0 || !length.is_multiple_of(PROBE_PAGE_SIZE) {
            return Err(Error::State(format!(
                "0xD6 reads whole {PROBE_PAGE_SIZE}-byte pages; {length} is not one"
            )));
        }
        address
            .checked_add(u32::try_from(length).unwrap_or(u32::MAX))
            .ok_or_else(|| {
                Error::State(format!(
                    "0xD6 for {length} bytes at 0x{address:08X} runs past the 32-bit address \
                     space"
                ))
            })?;
        let pages = length / PROBE_PAGE_SIZE;
        // Anything already on the line belongs to an earlier exchange. The
        // 2026-11-21 run left four bytes after the twenty-byte report, ahead of
        // the first page, so drain defensively before sending 0xD6 in case
        // stale bytes sit there.
        self.transport.clear_input()?;
        let mut frame = Vec::with_capacity(13);
        frame.push(OPCODE_READ_RANGE);
        frame.extend_from_slice(&u32::try_from(PROBE_PAGE_SIZE).unwrap_or(64).to_be_bytes());
        frame.extend_from_slice(&address.to_be_bytes());
        frame.extend_from_slice(&u32::try_from(length).unwrap_or(0).to_be_bytes());
        self.transport.write_all(&frame)?;
        self.transport.flush()?;

        let mut data = Vec::with_capacity(length);
        for page in 0..pages {
            let payload = self.read_page(page, address)?;
            data.extend_from_slice(&payload);
            // The DA is waiting for the host's byte after every page, the last
            // one included; "nothing is sent when the range is complete" (§5.5)
            // means the DA's *reply* to this advance, not that the advance is
            // skipped.
            self.transport.write_all(&[PAGE_ADVANCE])?;
            self.transport.flush()?;
        }
        Ok(data)
    }

    /// One `0xD6` page, checksummed, with [`READ_RETRIES`] re-requests.
    fn read_page(&mut self, page: usize, address: u32) -> Result<Vec<u8>> {
        let page_address = address.saturating_add(
            u32::try_from(page.saturating_mul(PROBE_PAGE_SIZE)).unwrap_or(u32::MAX),
        );
        let mut attempt = 0usize;
        loop {
            let mut packet = vec![0u8; PROBE_PAGE_SIZE + 2];
            self.read_packet(&mut packet, &format!("0xD6 page {page}"))?;
            let want = PROBE_PAGE_SIZE + 2;
            let payload = packet.get(..PROBE_PAGE_SIZE).unwrap_or_default();
            let trailer = packet.get(PROBE_PAGE_SIZE..want).unwrap_or_default();
            let checksum = u16::from_be_bytes(<[u8; 2]>::try_from(trailer).unwrap_or([0, 0]));
            if checksum == byte_sum(payload) {
                return Ok(payload.to_vec());
            }
            attempt = attempt.saturating_add(1);
            if attempt > READ_RETRIES {
                return Err(Error::Framing {
                    what: format!(
                        "0xD6 page {page} at 0x{page_address:08X} failed its checksum \
                         {attempt} times (trailer 0x{checksum:04X}, bytes sum to \
                         0x{:04X})",
                        byte_sum(payload)
                    ),
                    offset: Some(page.saturating_mul(PROBE_PAGE_SIZE)),
                });
            }
            // A page that failed may not have been the whole page: if extra
            // bytes arrived ahead of it, the tail is still queued and asking
            // again would repeat the same shift. Drop what is left, then ask.
            self.transport.clear_input()?;
            self.transport.write_all(&[PAGE_RETRY])?;
            self.transport.flush()?;
        }
    }

    /// Read a whole page, trailer included, or time out.
    fn read_packet(&mut self, buf: &mut [u8], what: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + self.reply_timeout;
        let mut filled = 0usize;
        while filled < buf.len() {
            let Some(rest) = buf.get_mut(filled..) else {
                break;
            };
            filled += self.transport.read(rest)?;
            if std::time::Instant::now() > deadline {
                return Err(Error::Timeout {
                    what: format!("{what} ({filled} of {} bytes)", buf.len()),
                    timeout: self.reply_timeout.into(),
                });
            }
        }
        Ok(())
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
    /// [`Error::Safety`] when the unit is not a Holux M-241, when `backup` is
    /// not [`FLASH_WRITE_LENGTH`] bytes, or when its flash is not `backup` byte
    /// for byte — for example when the backup came from another unit. The
    /// message names the first differing offset. Nothing is written.
    ///
    /// The length guard is deliberate: this method reads the shipped
    /// [`FLASH_WRITE_START`] span, which [`check_layout`] only allows at
    /// [`FLASH_WRITE_LENGTH`]. The CLI has already run [`check_image`] against
    /// the same profile, so a different-length backup is refused twice rather
    /// than read past the image.
    pub fn verify_device(rom: &mut MtkBootRom<T>, backup: &[u8]) -> Result<()> {
        if backup.len() != FLASH_WRITE_LENGTH {
            return Err(Error::Safety {
                reason: format!(
                    "the backup is {} bytes; the shipped write range reads {} bytes from \
                     0x{FLASH_WRITE_START:08X}. Nothing was written",
                    backup.len(),
                    FLASH_WRITE_LENGTH
                ),
            });
        }
        Self::verify_device_at(rom, backup, FLASH_WRITE_START)
    }

    /// [`Self::verify_device`] with an explicit DA address for the span.
    ///
    /// The BootROM addresses flash at [`BASE_ADDR_FIRMWARE`]; a DA address is
    /// the same flash as seen from zero, so the read adds the BootROM base back.
    /// The 2026-11-21 read settled that mapping: DA address `0` returned the
    /// vector `18 F0 9F E5` and the vendor image's first 64 bytes, which is the
    /// image the BootROM reads at `0x0020_0000` (see [`FLASH_WRITE_START`]).
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
        // The span is the backup's own length, not the V1.13 constant: a V1.12
        // unit's dump is 308 bytes shorter, and reading V1.13's length there
        // would compare 308 bytes of whatever follows the image and refuse a
        // correct backup.
        let device = rom.read16(address, backup.len())?;
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
    /// It is used again here for the acceptance rule ([`check_image`]), so a
    /// caller who skipped the device check still cannot write an image other
    /// than their own dump with only [`PATCH_OFFSETS`] moved.
    ///
    /// `confirmation` is required and checked: a token whose flag was never set
    /// is [`Error::Safety`]. Nothing about the type forces a caller to build it
    /// from a prompt, but every route into this function names the step.
    ///
    /// The range is still V1.13's: [`check_layout`] refuses every start address
    /// this crate cannot establish and every length but [`V1_13_WRITE_LENGTH`],
    /// so a V1.12 image is refused here rather than written into a shorter span.
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
    /// [`check_layout`] first, which accepts only the settled `0`, so a release
    /// build has no reachable way to write at a range a caller chooses.
    /// **Every** write goes through the checks in this function:
    /// [`check_image`] against `backup`, which is `AGENTS.md` §2.3's rule, so
    /// `0xD3` cannot go out for a buffer that was never compared with the user's
    /// own dump.
    fn write_range(&mut self, backup: &[u8], start: u32, image: &[u8]) -> Result<PendingWrite> {
        // The shipped range is V1.13's ([`check_layout`] refuses any other
        // length), so the profile accepting the image is that one. A V1.12 or
        // V1.14 image cannot arrive here: `check_image` refuses a length that is
        // not the profile's, and the profile is V1.13.
        let accepted = check_image(&PROFILE_V1_13, backup, image)?;
        let image = accepted.image;
        let mut packets = 0usize;
        let mut progress = Progress::NothingWritten;
        let end = range_end(start);
        // A refused `0xD3` records nothing and no packet was sent, so its error
        // stands on its own. Wrapping it in the partial-write paragraph would
        // tell the user the range is "now only partly erased and programmed"
        // when this run did not erase or program a byte.
        self.record_range(start, end)?;
        let outcome = self.program(image, start, &mut packets, &mut progress);
        if let Err(err) = outcome {
            let unfinished = PartialWrite {
                progress,
                start,
                length: image.len(),
                restart_sent: false,
                reached_read_back: false,
            };
            return Err(after_write(err, unfinished));
        }
        Ok(PendingWrite {
            start,
            length: image.len(),
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
///
/// Public because the `m241-cli` test double has to build the same trailer the
/// reader checks: the fake in `crates/m241-cli/src/testing.rs` frames a `0xD6`
/// page with this function, so an "ok" page is one the crate's own reader
/// accepts. Using the function under test is taken deliberately — that fake
/// exists to watch *which opcodes the command sends*, and a checksum bug on
/// the reader's side belongs in `crate::da`'s tests, which compute the sum
/// independently.
pub fn byte_sum(data: &[u8]) -> u16 {
    data.iter()
        .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)))
}

/// Refuse unless the range constants are the values the safety rules pin.
///
/// `AGENTS.md` §2.2 and `docs/dev/11-safety-review.md` review items 1 and 2:
/// the range must be a hard-coded constant, and it must be refused when it is
/// anything else. The start was settled at `0` by the 2026-11-21 read through
/// the DA's own view (`docs/dev/05-verification.md` §1), and
/// [`UNRESOLVED_STARTS`] is empty. This function is the gate: it passes only
/// while that list is empty, the pinned start is the settled `0`, and the length
/// is [`FLASH_WRITE_LENGTH`]. A non-empty list closes the gate again, and so
/// does any other start — `0x0020_0000` is the BootROM's view and reads as
/// unmapped memory through the DA.
///
/// The range does include address zero, which is where the DA sees the image
/// begin. `0xD5` erases every sector the range touches
/// (`0x0010251C`–`0x00102560`), so the reset vector is erased and reprogrammed
/// with the rest; [`check_image`] and [`DownloadAgent::verify_device`] are what
/// require those bytes to be the unit's own backup.
///
/// # Errors
///
/// [`Error::Safety`] when [`UNRESOLVED_STARTS`] is non-empty, when the pinned
/// start is not the settled `0`, when the write start is not that pin, when any
/// other pinned value has moved, or when the patch table and the readers that
/// define it disagree.
pub fn check_layout() -> Result<()> {
    check_layout_of(
        FLASH_WRITE_START,
        FLASH_WRITE_LENGTH,
        EXPECTED_WRITE_START,
        EXPECTED_WRITE_LENGTH,
    )
}

/// The DA-side base the 2026-11-21 hardware read settled: `0x0000_0000`.
///
/// `docs/dev/05-verification.md` §1 records the run: a `0xD6` read at this
/// address returned the vector `18 F0 9F E5` and the vendor image's first 64
/// bytes, while `0x0020_0000` returned the unmapped-bus pattern. A change to
/// this constant needs a new row in that section.
const SETTLED_WRITE_START: u32 = 0x0000_0000;

/// The pinned start, in one place so a test can compare it with the constant.
///
/// It is the settled `0`, not a candidate. [`check_layout_of`] refuses a pin
/// that moves away from [`SETTLED_WRITE_START`] before it compares the shipped
/// constant, so editing this value alone cannot turn the BootROM's view into
/// "the pinned range".
const EXPECTED_WRITE_START: u32 = SETTLED_WRITE_START;

/// The pinned image length, for the same reason.
const EXPECTED_WRITE_LENGTH: usize = 0x0006_345C;

/// The body of [`check_layout`], with the constants as arguments.
///
/// Test-only, so a test can vary *one* of the four numbers and see which rule
/// refuses it. The public entry point hard-codes both pairs, so no caller can
/// choose a range.
fn check_layout_of(
    write_start: u32,
    write_length: usize,
    expected_start: u32,
    expected_length: usize,
) -> Result<()> {
    // A non-empty list means an address is unresolved again. Refusing whichever
    // value the list holds keeps the gate shut without choosing a candidate.
    if !UNRESOLVED_STARTS.is_empty() {
        return Err(Error::Safety {
            reason: format!(
                "the DA-side write start is unresolved again: UNRESOLVED_STARTS is \
                 0x{UNRESOLVED_STARTS:08X?}. The 2026-11-21 read settled it at \
                 0x{SETTLED_WRITE_START:08X} (docs/dev/05-verification.md §1); refusing every \
                 write until a new hardware row settles the list"
            ),
        });
    }

    // The pin is the settled value, not a candidate. Moving it is refused here,
    // so editing it to the BootROM's 0x0020_0000 cannot turn into a shipped
    // range.
    if expected_start != SETTLED_WRITE_START {
        return Err(Error::Safety {
            reason: format!(
                "the expected write start 0x{expected_start:08X} is not the settled DA-side base \
                 0x{SETTLED_WRITE_START:08X} (docs/dev/05-verification.md §1); the pin may not \
                 move without a new hardware row"
            ),
        });
    }

    // Every other start is refused, including the BootROM's 0x0020_0000: the
    // DA reads that address as unmapped memory, not as the image.
    if write_start != expected_start {
        return Err(Error::Safety {
            reason: format!(
                "the write range starts at 0x{write_start:08X}, not the settled DA-side base \
                 0x{expected_start:08X}. The image begins at DA address 0; 0x0020_0000 is the \
                 BootROM's view and reads as unmapped through the DA \
                 (docs/dev/05-verification.md §1). Refusing to write an unpinned range"
            ),
        });
    }
    if write_length != expected_length {
        return Err(Error::Safety {
            reason: format!(
                "the write range is 0x{write_start:08X}+0x{write_length:X}, not the settled \
                 0x{expected_start:08X}+0x{expected_length:X}; refusing to write an unpinned range"
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

/// An image that passed [`check_image`], plus the profile it belongs to.
///
/// Produced by [`check_image`] and turned into a plan by [`write_plan`]. It is
/// not a permission to write: [`DownloadAgent::write`] still runs
/// [`check_layout`], which refuses every start address this crate cannot
/// establish.
///
/// There is exactly one acceptance rule (`AGENTS.md` §2.3): the image is the
/// user's own backup, or differs from it only at that profile's patch offsets.
/// No digest, vendor package, or "known profile" route exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedImage<'a> {
    /// The profile the image was read against.
    pub profile: &'static FirmwareProfile,
    /// The image itself, after checking.
    pub image: &'a [u8],
}

/// How many differing offsets [`WritePlan`] keeps.
///
/// A WNRO patch changes three bytes, so the limit never hides a patched backup;
/// a large diff (for example a whole vendor image) is refused before a plan is
/// built, but the limit still keeps a diagnostic short.
pub const PLAN_DIFF_LIMIT: usize = 16;

/// Everything `fw flash --dry-run` reports that can be decided without a wire.
///
/// Built by [`write_plan`], which sends nothing and opens no port. The plan is
/// the same computation a real write would do up to the first byte —
/// [`DownloadAgent::write`] reaches the client only through [`check_image`] —
/// plus the numbers a person needs to decide whether to continue: how many
/// `0xD5` packets, how much would change, and which profile the image is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritePlan {
    /// The profile the image is written as.
    pub profile: &'static FirmwareProfile,
    /// Bytes the image holds; equals `profile.size`.
    pub size: usize,
    /// The image's device type string (`01017-00F`, …).
    pub device_type: String,
    /// The length of the span a real write would program.
    pub write_length: usize,
    /// The DA-side start a real write would use ([`FLASH_WRITE_START`]).
    ///
    /// This is the settled base, not a candidate: the 2026-11-21 read placed the
    /// image at DA address `0` (`docs/dev/05-verification.md` §1), and
    /// [`UNRESOLVED_STARTS`] is empty.
    pub start: u32,
    /// `0xD5` packets a write of `write_length` would take.
    pub packets: usize,
    /// Bytes that differ from the backup, when the backup is the same length.
    pub differing_bytes: Option<usize>,
    /// The offsets where the image differs from the backup, truncated to the
    /// first [`PLAN_DIFF_LIMIT`] so a large diff cannot print 400,000 lines.
    pub differing_offsets: Vec<usize>,
}

/// Refuse unless `image` is the user's own backup or a WNRO patch of it.
///
/// `AGENTS.md` §2.3 defines the only rule: an image identical to the backup is
/// accepted, and an image whose every differing byte falls inside `profile`'s
/// patch offsets is accepted. Every other difference is refused. There is no
/// digest, vendor-package, or "known profile" bypass: a vendor release carries
/// per-unit fields that no one unit's dump has, so accepting it would weaken the
/// backup check §2.3 tells this code not to weaken.
///
/// The backup's shape is checked too: it must be `profile.size` bytes, or a
/// backup taken with a different `--length` would be compared against an image
/// it can never match.
///
/// # Errors
///
/// [`Error::Safety`] when either buffer is not `profile.size`, when the image is
/// not an M-241 image, when the image's device type is not the profile's, or when
/// the image differs from the backup outside the profile's patch offsets. The
/// refusal names the first differing offset and the patch table it checked.
pub fn check_image<'a>(
    profile: &'static FirmwareProfile,
    backup: &[u8],
    image: &'a [u8],
) -> Result<AcceptedImage<'a>> {
    let offsets = profile_patch_offsets(profile).ok_or_else(|| Error::Safety {
        reason: format!(
            "no measured patch table for profile {}; this crate can only check {} and {}",
            profile.name, PROFILE_V1_13.name, PROFILE_V1_12.name
        ),
    })?;
    for (label, bytes) in [("backup", backup), ("image", image)] {
        if bytes.len() != profile.size {
            return Err(Error::Safety {
                reason: format!(
                    "the {label} is {} bytes; profile {} is {} bytes",
                    bytes.len(),
                    profile.name,
                    profile.size
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
    if parsed.device_type() != profile.device_type {
        return Err(Error::Safety {
            reason: format!(
                "the image's device type is {:?}; profile {} is {:?}",
                parsed.device_type(),
                profile.name,
                profile.device_type
            ),
        });
    }

    if !all_within(backup, image, offsets) {
        let offset = backup
            .iter()
            .zip(image.iter())
            .position(|(saved, wanted)| saved != wanted)
            .unwrap_or(0);
        let saved = backup.get(offset).copied().unwrap_or(0);
        let wanted = image.get(offset).copied().unwrap_or(0);
        return Err(Error::Safety {
            reason: format!(
                "the image differs from the backup at 0x{offset:X} (0x{saved:02X} -> \
                 0x{wanted:02X}), which is outside the {} patch offsets {offsets:X?}. Only your \
                 own backup, or a WNRO-patched copy of it, may be written; write your own dump \
                 and patch that",
                profile.name
            ),
        });
    }

    Ok(AcceptedImage { profile, image })
}

/// Whether every byte where the two buffers differ is in `offsets`.
fn all_within(backup: &[u8], image: &[u8], offsets: &[usize]) -> bool {
    backup
        .iter()
        .zip(image.iter())
        .enumerate()
        .all(|(offset, (saved, wanted))| saved == wanted || offsets.contains(&offset))
}

/// Decide whether `image` could be written over `backup`, and describe it.
///
/// Sends nothing, opens nothing, and does not touch [`check_layout`]: this is
/// the function `fw flash --dry-run` calls, and the one a real write would call
/// before the port is opened. The two answer the same questions, so a dry run
/// that says "acceptable" cannot be followed by a write that refuses for a
/// reason the plan did not name.
///
/// The result's start is [`FLASH_WRITE_START`], the settled DA-side base. The
/// plan reports the range a real write would use; it does not grant permission
/// to use it, because [`DownloadAgent::write`] still runs [`check_layout`] and
/// [`DownloadAgent::verify_device`] runs against the unit first.
///
/// # The backup is the image's own dump
///
/// `profile` is the image's, and `backup` must be the same length as the image
/// (see [`check_image`]). Because every differing byte must be in the patch
/// table, a backup from another unit or another model is refused: its per-unit
/// fields differ outside those offsets. That is the intended refusal, not a
/// consent prompt — a cross-model vendor image can neither be produced by
/// patching this unit's dump nor written by this build.
///
/// # Errors
///
/// As [`check_image`].
pub fn write_plan(
    profile: &'static FirmwareProfile,
    backup: &[u8],
    image: &[u8],
) -> Result<WritePlan> {
    check_image(profile, backup, image)?;
    let device_type = FirmwareImage::new(image)
        .map(|parsed| parsed.device_type())
        .unwrap_or_default();
    let differing_offsets: Vec<usize> = backup
        .iter()
        .zip(image.iter())
        .enumerate()
        .filter(|(_offset, (saved, wanted))| saved != wanted)
        .map(|(offset, _)| offset)
        .take(PLAN_DIFF_LIMIT)
        .collect();
    let differing_bytes = (backup.len() == image.len()).then(|| {
        backup
            .iter()
            .zip(image.iter())
            .filter(|(saved, wanted)| saved != wanted)
            .count()
    });
    let write_length = profile_write_length(profile);
    Ok(WritePlan {
        profile,
        size: image.len(),
        device_type,
        write_length,
        start: FLASH_WRITE_START,
        packets: write_length.div_ceil(PACKET_SIZE),
        differing_bytes,
        differing_offsets,
    })
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

    /// The start address the protocol tests drive the wire with.
    ///
    /// Deliberately not the shipped constant. `0x4000` keeps the fake's
    /// "nothing recorded" pair `(0, 0)` distinguishable from a recorded range,
    /// so a test asserting `recorded_range() == (0, 0)` after a refusal cannot
    /// pass because the range itself started at zero. The public `write` path is
    /// covered with the settled `0` in
    /// `the_shipped_write_path_opens_the_settled_range_and_verifies`.
    const WIRE_START: u32 = 0x0000_4000;
    /// The inclusive end of the test range.
    const WIRE_END: u32 = WIRE_START + (FLASH_WRITE_LENGTH as u32 - 1);

    /// A structurally valid V1.13 image carrying the stock constants.
    ///
    /// Header fields follow `m241_core::firmware::image`: the vector table is
    /// `18 F0 9F E5`, and the family and device fields are NUL-terminated C
    /// strings, so the bytes after them are `0x00`, not `0xFF`.
    ///
    /// Note that these fixtures are **not** the vendor packages: they share the
    /// header and the constant offsets, but every other byte is `0xFF`. The
    /// tests use them to exercise the one backup-diff rule without any vendor
    /// bytes.
    fn stock_image_of(profile: &'static FirmwareProfile) -> Vec<u8> {
        let mut data = vec![0xFFu8; profile.size];
        data[..VECTOR.len()].copy_from_slice(&VECTOR);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY_SIZE].fill(0);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
        let start = profile.version_marker;
        let end = start + VERSION_MARKER_LEN;
        data[start..end].copy_from_slice(profile.name.as_bytes());
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
        }
        data
    }

    /// The V1.13 stock image.
    fn stock_image() -> Vec<u8> {
        stock_image_of(&PROFILE_V1_13)
    }

    /// `profile`'s stock image with the measured three-byte patch applied.
    fn patched_image_of(profile: &'static FirmwareProfile) -> Vec<u8> {
        let mut data = stock_image_of(profile);
        let before = GPS_EPOCH_UNIX.to_le_bytes();
        let after = GPS_EPOCH_UNIX_WNRO.to_le_bytes();
        for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
            if old != new {
                data[EFFECTIVE_EPOCH_OFFSET + index] = *new;
            }
        }
        data[profile.version_marker + VERSION_MARKER_LEN - 1] = b'4';
        data
    }

    /// The V1.13 stock image with the measured three-byte patch applied.
    fn patched_image() -> Vec<u8> {
        patched_image_of(&PROFILE_V1_13)
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
    fn reenter(device: FakeDa) -> Result<FakeDa> {
        let mut device = device;
        device.set_modelled(Modelled::BootRom);
        Ok(device)
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
    fn the_shipped_start_is_zero_and_the_layout_check_accepts_it() {
        // The 2026-11-21 DA read settled the base at 0 (05-verification §1):
        // a `0xD6` at 0 returns the vector and the vendor image's first 64
        // bytes, and 0x00200000 returns the unmapped-bus pattern.
        assert_eq!(FLASH_WRITE_START, 0x0000_0000);
        assert_eq!(FLASH_WRITE_START, SETTLED_WRITE_START);
        assert_eq!(FLASH_WRITE_LENGTH, 0x0006_345C);
        assert_eq!(FLASH_WRITE_LENGTH, 406_620);
        assert_eq!(FLASH_WRITE_LENGTH, PROFILE_V1_13.size);
        assert_eq!(FLASH_WRITE_END, 0x0006_345B);
        assert!(
            UNRESOLVED_STARTS.is_empty(),
            "the list is empty only because 05-verification.md §1 records the read"
        );
        check_layout().expect("the settled range must pass the layout gate");
    }

    #[test]
    fn the_layout_check_refuses_an_unsettled_start() {
        // `0x0020_0000` is the BootROM's view of the image, and the DA reads
        // unmapped memory there. A write at that address would erase and
        // program the wrong sectors. The probe still reads it, but the
        // write gate does not accept it.
        assert_eq!(PROBE_ADDRESSES[0], FLASH_WRITE_START);
        assert_eq!(PROBE_ADDRESSES[1], 0x0020_0000);

        let err = check_layout_of(
            0x0020_0000,
            FLASH_WRITE_LENGTH,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("the BootROM view is not the DA-side base");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("unpinned range"), "got {err}");
        assert!(format!("{err}").contains("0x00000000"), "got {err}");
        assert!(format!("{err}").contains("0x00200000"), "got {err}");

        // A start that is neither the settled value nor the other view is
        // refused by the same rule, so there is no third address that slips
        // through.
        let err = check_layout_of(
            WIRE_START,
            FLASH_WRITE_LENGTH,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("a third start");
        assert!(format!("{err}").contains("unpinned range"), "got {err}");
    }

    #[test]
    fn the_pinned_start_is_the_shipped_start_and_the_settled_zero() {
        // The pin and the shipped constant are two numbers that must agree, and
        // both must be the value the hardware row settled.
        assert_eq!(SETTLED_WRITE_START, 0x0000_0000);
        assert_eq!(EXPECTED_WRITE_START, SETTLED_WRITE_START);
        assert_eq!(
            EXPECTED_WRITE_START, FLASH_WRITE_START,
            "the pin must be the shipped start, not the BootROM's view"
        );
        assert_eq!(EXPECTED_WRITE_LENGTH, FLASH_WRITE_LENGTH);
        assert_eq!(EXPECTED_WRITE_LENGTH, PROFILE_V1_13.size);

        // The pin drifting to the BootROM view is refused before the shipped
        // constant is compared.
        let err = check_layout_of(
            FLASH_WRITE_START,
            FLASH_WRITE_LENGTH,
            0x0020_0000,
            FLASH_WRITE_LENGTH,
        )
        .expect_err("a pin at the other view");
        assert!(
            format!("{err}").contains("settled DA-side base"),
            "the pin rule must name the settled base: {err}"
        );
    }

    #[test]
    fn a_pinned_range_that_disagrees_is_refused_on_its_own() {
        // The gate opens for exactly one range: the settled start and the
        // pinned length. Vary each number in turn and check the stage that owns
        // it refuses, so a drift in any one cannot hide behind another.
        let length = FLASH_WRITE_LENGTH;
        let start = FLASH_WRITE_START;
        check_layout_of(start, length, EXPECTED_WRITE_START, EXPECTED_WRITE_LENGTH)
            .expect("the settled range passes");

        let err = check_layout_of(
            start,
            length + 2,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("a longer range");
        assert!(format!("{err}").contains("unpinned range"), "got {err}");

        let err = check_layout_of(start, length, 0x0020_0000, length).expect_err("a moved pin");
        assert!(
            format!("{err}").contains("settled DA-side base"),
            "got {err}"
        );

        let err = check_layout_of(
            0x0020_0000,
            length,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("the BootROM view");
        assert!(format!("{err}").contains("unpinned range"), "got {err}");
    }

    #[test]
    fn the_layout_check_names_the_settled_base_in_its_refusal() {
        // The message has to say *why* another start is refused, so a reader
        // does not "fix" it by pointing at the BootROM's view. It must not
        // depend on the item number in `11-safety-review.md`, which can be
        // renumbered.
        let err = check_layout_of(
            0x0020_0000,
            FLASH_WRITE_LENGTH,
            EXPECTED_WRITE_START,
            EXPECTED_WRITE_LENGTH,
        )
        .expect_err("the BootROM view is refused");
        let text = format!("{err}");
        assert!(text.contains("0x00000000"), "got {text}");
        assert!(text.contains("0x00200000"), "got {text}");
        assert!(text.contains("05-verification.md"), "got {text}");
        assert!(
            !text.contains("review item"),
            "the refusal must not cite a document item number: {text}"
        );
    }

    #[test]
    fn the_shipped_write_path_opens_the_settled_range_and_verifies() {
        // The gate passes only because 05-verification §1 settled the start at
        // 0. Drive the public `write` (not the test-only `write_range`) so the
        // range on the wire is the shipped one, and verify it through the
        // BootROM view.
        let backup = stock_image();
        let image = patched_image();
        let mut device = FakeDa::with_firmware(&backup);
        let report = enter_download_agent(&mut device);
        assert_eq!(report.first().copied(), Some(0xC0), "the DA banner");
        let mut da = DownloadAgent::new(device);
        let pending = da
            .write(Confirmation::granted(), &backup, &image)
            .expect("the settled range passes check_layout");
        assert_eq!(pending.start, 0);
        assert_eq!(
            da.transport_mut().recorded_range(),
            (0, FLASH_WRITE_END),
            "0xD3 records the settled range"
        );
        let report = pending
            .verify_written(da, &image, FLASH_WRITE_START, reenter)
            .expect("read back through the BootROM view");
        assert_eq!(report.start, 0);
        assert_eq!(report.verified, FLASH_WRITE_LENGTH);
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
        let accepted = check_image(&PROFILE_V1_13, &backup, &backup).expect("identical");
        assert_eq!(accepted.image, backup.as_slice());
        let patched = patched_image();
        check_image(&PROFILE_V1_13, &patched, &patched).expect("a patched unit written back");
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
        let accepted = check_image(&PROFILE_V1_13, &backup, &patched).expect("the measured patch");
        assert_eq!(accepted.image, patched.as_slice());
    }

    #[test]
    fn a_v1_12_patched_backup_is_accepted_against_the_v1_12_table() {
        // The V1.12 marker digit is at `0x59B18`, not V1.13's `0x59C2C`. Checking
        // a V1.12 patch against the V1.13 table would refuse it at the marker
        // byte, which is exactly the regression this pins.
        let backup = stock_image_of(&PROFILE_V1_12);
        let patched = patched_image_of(&PROFILE_V1_12);
        let differing: Vec<usize> = backup
            .iter()
            .zip(patched.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(offset, _)| offset)
            .collect();
        assert_eq!(differing, PATCH_OFFSETS_V1_12.to_vec());
        let accepted = check_image(&PROFILE_V1_12, &backup, &patched)
            .expect("a V1.12 image patched at V1.12's offsets");
        assert_eq!(accepted.image, patched.as_slice());
        // And the V1.13 table must not accept it: the two tables differ at the
        // marker byte, so the diff-only route is profile-specific.
        assert!(
            check_image(&PROFILE_V1_13, &backup, &patched).is_err(),
            "a V1.12 image is not a V1.13 image"
        );
    }

    #[test]
    fn an_image_that_differs_outside_the_patch_table_is_refused() {
        let backup = stock_image();
        for offset in [0usize, 0x200, 0x402F8, 0x59C2D, FLASH_WRITE_LENGTH - 1] {
            let mut image = patched_image();
            image[offset] ^= 0xFF;
            let err =
                check_image(&PROFILE_V1_13, &backup, &image).expect_err("outside the patch table");
            assert!(matches!(err, Error::Safety { .. }), "got {err}");
            assert!(
                format!("{err}").contains("outside the V1.13 patch offsets"),
                "the refusal names the rule: {err}"
            );
        }
    }

    #[test]
    fn an_image_that_differs_outside_the_patch_table_names_the_first_offset() {
        // The simplest shape of the same refusal: the backup is this image minus
        // one changed byte outside the table, so there is exactly one differing
        // offset and the message must name it.
        let image = stock_image();
        let mut backup = image.clone();
        backup[0x1234] ^= 0xFF;
        let err = check_image(&PROFILE_V1_13, &backup, &image).expect_err("outside the table");
        let text = format!("{err}");
        assert!(text.contains("0x1234"), "got {text}");
        assert!(
            text.contains("outside the V1.13 patch offsets"),
            "got {text}"
        );
    }

    #[test]
    fn an_image_of_the_wrong_length_is_refused() {
        let backup = stock_image();
        let short = &backup[..FLASH_WRITE_LENGTH - 2];
        assert!(matches!(
            check_image(&PROFILE_V1_13, short, short),
            Err(Error::Safety { .. })
        ));
        assert!(matches!(
            check_image(&PROFILE_V1_13, &backup, &[]),
            Err(Error::Safety { .. })
        ));
        // A V1.12 backup is not a V1.13 image even though it is a valid M-241
        // image: the length is the profile's.
        let v112 = stock_image_of(&PROFILE_V1_12);
        let err = check_image(&PROFILE_V1_13, &v112, &v112).expect_err("wrong length");
        assert!(
            format!("{err}").contains("406312") && format!("{err}").contains("406620"),
            "the refusal names both lengths: {err}"
        );
    }

    #[test]
    fn an_image_that_is_not_a_v1_13_m241_image_is_refused() {
        let backup = patched_image();
        let mut wrong_device = backup.clone();
        wrong_device[DEVICE_TYPE_OFFSET + 6] = b'E';
        assert!(matches!(
            check_image(&PROFILE_V1_13, &backup, &wrong_device),
            Err(Error::Safety { .. })
        ));
        let mut wrong_family = backup.clone();
        wrong_family[FAMILY_OFFSET] = b'X';
        assert!(matches!(
            check_image(&PROFILE_V1_13, &backup, &wrong_family),
            Err(Error::Safety { .. })
        ));
    }

    // -- no vendor or cross-model bypass -----------------------------------

    #[test]
    fn a_vendor_style_image_with_per_unit_fields_different_is_refused() {
        // A vendor release shares the family and device strings, but its
        // per-unit runtime fields differ from any one unit's dump. Those bytes
        // are outside the patch table, and `AGENTS.md` §2.3 says the image must
        // be refused. There is no digest rule to fall back to.
        let backup = stock_image();
        let mut image = backup.clone();
        for offset in [0x1000usize, 0x2000, 0x3000] {
            image[offset] ^= 0xFF;
        }
        let err = check_image(&PROFILE_V1_13, &backup, &image).expect_err("not this unit's backup");
        let text = format!("{err}");
        assert!(
            text.contains("0x1000"),
            "the first difference is named: {text}"
        );
        assert!(
            text.contains("outside the V1.13 patch offsets"),
            "the refusal names the only rule: {text}"
        );
        assert!(
            !text.contains("SHA-256"),
            "there is no digest bypass to describe: {text}"
        );
    }

    #[test]
    fn a_backup_from_another_model_is_refused_by_the_backup_diff() {
        // The image is a valid V1.13 WNRO patch, but the backup names another
        // model. The device field differs outside the patch table, so the only
        // rule refuses it; no consent flag can make it writable.
        let image = patched_image();
        let mut backup = stock_image();
        backup[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + 9].copy_from_slice(b"01017-00E");
        let err = check_image(&PROFILE_V1_13, &backup, &image)
            .expect_err("the backup is another model's");
        let text = format!("{err}");
        assert!(text.contains("Only your own backup"), "got {text}");
    }

    #[test]
    fn an_image_of_the_right_shape_but_a_different_device_is_still_refused() {
        // A V1.12 image checked as V1.13 has the wrong length, which the length
        // check catches. The device-type check is what catches an image of the
        // right length whose header names another model.
        let mut image = stock_image();
        image[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + 9].copy_from_slice(b"01017-00E");
        let err = check_image(&PROFILE_V1_13, &stock_image(), &image)
            .expect_err("the device type is V1.12's");
        let text = format!("{err}");
        assert!(text.contains("01017-00E"), "got {text}");
        assert!(text.contains("01017-00F"), "got {text}");
    }

    // -- the write plan ----------------------------------------------------

    #[test]
    fn a_plan_for_a_patched_v1_13_backup_names_the_profile_and_the_v1_13_packets() {
        let backup = stock_image();
        let image = patched_image();
        let plan = write_plan(&PROFILE_V1_13, &backup, &image).expect("acceptable");
        assert_eq!(plan.profile, &PROFILE_V1_13);
        assert_eq!(plan.size, PROFILE_V1_13.size);
        assert_eq!(plan.device_type, "01017-00F");
        assert_eq!(plan.write_length, V1_13_WRITE_LENGTH);
        assert_eq!(plan.start, FLASH_WRITE_START);
        assert_eq!(plan.packets, V1_13_WRITE_LENGTH.div_ceil(PACKET_SIZE));
        assert_eq!(plan.packets, 1_589);
        assert_eq!(plan.differing_bytes, Some(3));
        assert_eq!(plan.differing_offsets, PATCH_OFFSETS.to_vec());
    }

    #[test]
    fn a_plan_for_a_v1_12_patched_backup_uses_the_v1_12_length() {
        let backup = stock_image_of(&PROFILE_V1_12);
        let image = patched_image_of(&PROFILE_V1_12);
        let plan = write_plan(&PROFILE_V1_12, &backup, &image).expect("acceptable");
        assert_eq!(plan.profile, &PROFILE_V1_12);
        assert_eq!(plan.write_length, V1_12_WRITE_LENGTH);
        assert_ne!(plan.write_length, V1_13_WRITE_LENGTH);
        assert_eq!(plan.packets, V1_12_WRITE_LENGTH.div_ceil(PACKET_SIZE));
        assert_eq!(plan.device_type, "01017-00E");
    }

    #[test]
    fn a_v1_14_image_plans_under_the_v1_13_profile() {
        // A patched V1.13 unit reports `V1.14`; `identify` accepts the marker,
        // and `check_image` sees the same three-byte diff a V1.13 patch makes.
        let backup = stock_image();
        let image = patched_image();
        let marker = FirmwareImage::new(&image).expect("header").field(
            PROFILE_V1_13.version_marker,
            m241_core::firmware::profiles::VERSION_MARKER_LEN,
        );
        assert_eq!(marker, "V1.14");
        let plan = write_plan(&PROFILE_V1_13, &backup, &image).expect("acceptable");
        assert_eq!(plan.profile.name, "V1.13");
    }

    #[test]
    fn a_cross_model_plan_is_refused() {
        // The image is a valid V1.12 WNRO patch, but the backup names another
        // model. The device field differs outside the patch table, so
        // `write_plan` refuses it; there is no consent flag that reopens it.
        let image = patched_image_of(&PROFILE_V1_12);
        let mut backup = stock_image_of(&PROFILE_V1_12);
        backup[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + 9].copy_from_slice(b"01017-00F");
        let err = write_plan(&PROFILE_V1_12, &backup, &image).expect_err("cross model");
        let text = format!("{err}");
        assert!(text.contains("Only your own backup"), "got {text}");
    }

    #[test]
    fn a_backup_of_another_length_is_refused() {
        // A real V1.12 image and a real V1.13 backup are 308 bytes apart. The
        // length check refuses that pair: the backup is not the dump this image
        // was planned against, and re-flashing a unit with an image that does
        // not match its own dump's length is exactly the write nobody can undo.
        let backup = stock_image();
        let image = stock_image_of(&PROFILE_V1_12);
        let err = write_plan(&PROFILE_V1_12, &backup, &image).expect_err("lengths differ");
        let text = format!("{err}");
        assert!(
            text.contains("406620") && text.contains("406312"),
            "got {text}"
        );
    }

    #[test]
    fn a_plan_reports_the_settled_start_and_does_not_reopen_the_question() {
        // The 2026-11-21 read settled the DA-side base at 0, so the plan names
        // it. The gate that still has to pass is `check_layout`, and the list
        // the gate refuses on must stay empty: putting a candidate back here
        // would make `check_layout` refuse every write again.
        let plan = write_plan(&PROFILE_V1_13, &stock_image(), &patched_image()).expect("plan");
        assert_eq!(plan.start, FLASH_WRITE_START);
        assert_eq!(plan.start, 0);
        assert!(UNRESOLVED_STARTS.is_empty());
        check_layout().expect("the plan's start is the one the gate accepts");
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
        assert_eq!(
            Progress::NothingWritten.bytes(WIRE_START, FLASH_WRITE_LENGTH),
            0
        );
        assert_eq!(
            Progress::UpTo(WIRE_START).bytes(WIRE_START, FLASH_WRITE_LENGTH),
            0
        );
        assert_eq!(
            Progress::UpTo(WIRE_START + PACKET_SIZE as u32).bytes(WIRE_START, FLASH_WRITE_LENGTH),
            PACKET_SIZE
        );
        assert_eq!(
            Progress::UpTo(0xFFFF_FFFF).bytes(WIRE_START, FLASH_WRITE_LENGTH),
            FLASH_WRITE_LENGTH
        );
        // The cap is the range's length, not the V1.13 constant: a V1.12 range
        // must not report 308 bytes it does not cover.
        assert_eq!(
            Progress::UpTo(0xFFFF_FFFF).bytes(WIRE_START, V1_12_WRITE_LENGTH),
            V1_12_WRITE_LENGTH
        );
    }

    #[test]
    fn the_recovery_paragraph_names_progress_and_the_restart() {
        let partial = PartialWrite {
            progress: Progress::UpTo(WIRE_START + 0x4000),
            start: WIRE_START,
            length: FLASH_WRITE_LENGTH,
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
        // And the total is the range's own length: a V1.12 write must report
        // 406312, not the V1.13 number the paragraph used to hard-code.
        let v112 = PartialWrite {
            length: V1_12_WRITE_LENGTH,
            ..partial
        };
        assert!(
            v112.recovery().contains("of 406312 bytes"),
            "got {}",
            v112.recovery()
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
    fn the_restart_does_not_read_a_da_reply() {
        // §4.5, §8 item 4: `0xD9` never returns and sends no DA reply. The fake
        // boots its firmware on the restart, so a read after it returns the
        // firmware's NMEA stream, not an acknowledgement: a client that waited
        // for an ACK would consume `$` as one and desynchronise.
        let backup = stock_image();
        let mut da = DownloadAgent::new(fake_in_da(&backup));
        da.restart().expect("0xD9 goes out");
        assert!(da.transport_mut().restarted(), "0xD9 must have been sent");
        let mut byte = [0u8; 1];
        let read = da.transport_mut().read(&mut byte).expect("read");
        assert_eq!(read, 1, "the booted firmware streams NMEA");
        assert_eq!(
            byte[0], b'$',
            "the first byte after 0xD9 is NMEA, never a DA ACK/NAK"
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
        assert!(
            text.contains("the read-back did not confirm it"),
            "a mismatch must be reported as the read-back disagreeing: {text}"
        );
        assert!(
            !text.contains("could not be reached"),
            "the read-back ran, so it was not unreachable: {text}"
        );
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
    fn the_shipped_device_check_refuses_a_backup_that_is_not_the_pinned_length() {
        // `verify_device` reads the shipped V1.13 span. A backup of another
        // length must be refused before the first read rather than compared
        // against fewer bytes than the write range covers.
        let backup = stock_image();
        let short = backup[..FLASH_WRITE_LENGTH - 2].to_vec();
        let mut rom = MtkBootRom::new(FakeDa::with_firmware(&backup));
        let err = DownloadAgent::verify_device(&mut rom, &short).expect_err("wrong length");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("406618") && text.contains("406620"),
            "the refusal names both lengths: {text}"
        );
        assert_eq!(
            rom.transport_mut().read_bytes(),
            0,
            "the length guard runs before any 0xA2 frame"
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

#[cfg(test)]
mod loader_tests {
    //! The DA load and the `0xD6` read, driven through the fake.
    //!
    //! Separate from the write tests above because the question is different:
    //! those are about whether a `0xD5` erases the right range, these are about
    //! whether a path that must never reach `0xD3`/`0xD5`/`0xD9` can.
    //! [`FakeDa::opcodes`] is where that is answered; every test here checks the
    //! set of opcodes the fake saw.

    use super::*;
    use crate::simulator::{FakeDa, Modelled};

    /// Where the fixture puts the fake's banner inside the DA image.
    ///
    /// The fake does not execute a loaded image: it answers `0xA8` with the
    /// banner of the DA it models. Putting that banner's bytes into the fixture
    /// is what lets the read-back compare the image the client sent with the RAM
    /// the fake kept, and what makes the report the client parses the report the
    /// image carries.
    const DA_REPORT_OFFSET: usize = 0x100;

    /// The twenty bytes the fake answers `0xA8` with.
    fn report_bytes() -> Vec<u8> {
        let mut report = Vec::with_capacity(DA_REPORT_SIZE);
        report.push(DA_REPORT_MARKER);
        report.extend_from_slice(&DA_VERSION.to_be_bytes());
        report.push(DA_REGION_ID);
        report.extend_from_slice(&0x0008_0000u32.to_be_bytes());
        report.extend_from_slice(&DA_FLASH_MANUFACTURER.to_be_bytes());
        report.extend_from_slice(&DA_FLASH_DEVICE.to_be_bytes());
        report.extend_from_slice(&0u16.to_be_bytes());
        report.extend_from_slice(&0u16.to_be_bytes());
        report.extend_from_slice(&0u32.to_be_bytes());
        report
    }

    /// A DA image of the right size carrying the fake's banner, with a distinct
    /// byte pattern everywhere else so a shifted load is visible.
    fn da_image() -> Vec<u8> {
        let mut image: Vec<u8> = (0..DA_IMAGE_SIZE)
            .map(|index| (index % 251) as u8)
            .collect();
        for (slot, byte) in image
            .iter_mut()
            .take(DA_IMAGE_HEAD.len())
            .zip(DA_IMAGE_HEAD)
        {
            *slot = byte;
        }
        for (slot, byte) in image
            .iter_mut()
            .skip(DA_REPORT_OFFSET)
            .zip(report_bytes().iter())
        {
            *slot = *byte;
        }
        image
    }

    /// A chip in the BootROM, past the sync, with nothing in flash.
    fn bootrom() -> FakeDa {
        let mut device = FakeDa::with_firmware(&[]);
        // `0xAF` reads RAM above the DA base; below it the fake's `read_flash`
        // would shift by `BASE_ADDR_FIRMWARE`, and every byte of the DA image is
        // above the base, so nothing is lost by leaving the window empty.
        device.set_modelled(Modelled::BootRom);
        device
    }

    /// Load `image` on `device`.
    fn load(device: &mut FakeDa, image: &[u8]) -> Result<DaLoadId> {
        let mut loader = DaLoader::new(device).with_reply_timeout(Duration::from_millis(50));
        loader.load(Confirmation::granted(), image)
    }

    #[test]
    fn the_loader_constants_are_pinned_to_the_documents_and_the_read_crate() {
        assert_eq!(DA_IMAGE_SIZE, 14_896);
        assert_eq!(DA_IMAGE_SIZE, 0x3A30);
        assert_eq!(DA_LOAD_ADDRESS, BASE_ADDR_DA);
        assert_eq!(DA_LOAD_ADDRESS, 0x0010_0800);
        assert_eq!(
            DA_IMAGE_HEAD,
            [0x00, 0x00, 0xA0, 0xE1],
            "the vendor DA_v3.8.4.bin starts 00 00 A0 E1 (mov r0, r0)"
        );
        assert_ne!(
            DA_IMAGE_HEAD, VECTOR,
            "the DA's first instruction is not the firmware's: the two images share only the \
             address 0x00100800"
        );
        assert_eq!(
            DA_JUMP_COUNT, 0,
            "0xA8's count is undocumented; the only measured client sent zero \
             (python/src/m241/protocol/brom.py, BootRom.jump)"
        );
        assert_eq!(DA_VERSION, 0x0308);
        assert_eq!(
            (DA_REGION_ID, DA_FLASH_MANUFACTURER, DA_FLASH_DEVICE),
            (0x49, 0x00C2, 0x22BA)
        );
        assert_eq!(PROBE_ADDRESSES, [0x0000_0000, 0x0020_0000]);
        assert!(
            UNRESOLVED_STARTS.is_empty(),
            "the start was settled at 0 by the 2026-11-21 run"
        );
        assert_eq!(
            PROBE_ADDRESSES[0], FLASH_WRITE_START,
            "the probe's first candidate is the settled DA-side base"
        );
        assert_eq!(PROBE_PAGE_SIZE, 64);
        assert_eq!((DA_REPORT_SIZE, DA_REPORT_MARKER), (20, 0xC0));
        assert_eq!(
            DA_IMAGE_SIZE % DA_LOAD_CHUNK,
            6,
            "14,896 is not a multiple of 10; the last 0xA1 frame carries 6 bytes. This is why \
             `write_image` must accept a short final chunk: if the image size changes, the \
             frame count and that short frame change with it, and the echo rule 5 (N−1 bytes \
             per frame) has to be re-read"
        );
        assert!(DA_IMAGE_SIZE.is_multiple_of(4), "0xAF reads dwords");
    }

    #[test]
    fn the_load_range_does_not_touch_the_bootrom_stack() {
        // `docs/reference/01-da-protocol.md` §9 and `docs/dev/03-hardware-facts.md`
        // "DA load address": `0x00100000`–`0x001007FF` is the BootROM's own
        // stack, and writing there kills the chip. The load covers
        // `0x00100800`–`0x0010422F`; this states the gap as an interval, so a
        // load that grew downwards or started lower fails here even if the
        // constants above are edited together.
        const STACK_START: u32 = 0x0010_0000;
        const STACK_END: u32 = 0x0010_07FF;
        let length = u32::try_from(DA_IMAGE_SIZE).expect("fits");
        let load_end = DA_LOAD_ADDRESS.saturating_add(length).saturating_sub(1);
        let overlaps = DA_LOAD_ADDRESS <= STACK_END && STACK_START <= load_end;
        assert!(
            !overlaps,
            "the load range 0x{DA_LOAD_ADDRESS:08X}–0x{load_end:08X} must not overlap the \
             BootROM stack 0x{STACK_START:08X}–0x{STACK_END:08X}"
        );
        assert_eq!(
            DA_LOAD_ADDRESS - STACK_END,
            1,
            "0x00100800 is the first address past the stack"
        );
        assert_eq!(load_end, 0x0010_422F, "the image ends where §9 says");
    }

    #[test]
    fn the_jump_frame_carries_count_zero_and_the_fake_refuses_any_other() {
        // The only value `0xA8`'s count may carry is zero: the protocol page
        // gives it no meaning and the one measured client sent zero. The fake
        // counts any other value, and a load must leave that count at zero.
        let image = da_image();
        let mut device = bootrom();
        load(&mut device, &image).expect("the DA starts");
        assert!(device.started());
        assert_eq!(
            device.bad_jump_counts(),
            0,
            "the loader's 0xA8 must carry count 0"
        );

        // And the same load with a count of 1 — what `Frame::for_bytes` would
        // produce from a one-byte length — is seen by the fake.
        let mut device = bootrom();
        let frame = Frame {
            opcode: OPCODE_JUMP,
            address: DA_LOAD_ADDRESS,
            count: 1,
        };
        device.write_all(&frame.to_bytes()).expect("jump");
        assert_eq!(device.bad_jump_counts(), 1);
    }

    #[test]
    fn the_loader_writes_the_whole_image_to_the_da_base_and_starts_it() {
        let image = da_image();
        let mut device = bootrom();
        let id = load(&mut device, &image).expect("the DA starts");

        assert_eq!(id.version, DA_VERSION);
        assert_eq!(id.region, DA_REGION_ID);
        assert_eq!(id.manufacturer, DA_FLASH_MANUFACTURER);
        assert_eq!(id.device, DA_FLASH_DEVICE);
        assert_eq!(
            device.ram().get(..DA_IMAGE_SIZE).map(<[u8]>::to_vec),
            Some(image.clone()),
            "every byte of the image must be in RAM at the DA base"
        );
        assert_eq!(
            device.bootrom_writes(),
            DA_IMAGE_SIZE.div_ceil(DA_LOAD_CHUNK),
            "one 0xA1 frame per chunk, the last one short"
        );
        assert!(device.started(), "0xA8 to the DA base must have been taken");
        let opcodes = device.opcodes();
        assert_eq!(opcodes.first().copied(), Some(OPCODE_WRITE16));
        assert_eq!(opcodes.last().copied(), Some(OPCODE_JUMP));
        assert_eq!(
            opcodes
                .iter()
                .filter(|byte| **byte == OPCODE_READ32)
                .count(),
            1,
            "exactly one 0xAF read-back"
        );
        assert!(
            opcodes.iter().all(|byte| *byte == OPCODE_WRITE16
                || *byte == OPCODE_READ32
                || *byte == OPCODE_JUMP),
            "the load sends 0xA1, 0xAF and 0xA8 and nothing else: {opcodes:02X?}"
        );
    }

    #[test]
    fn a_drain_that_fails_is_reported_instead_of_a_successful_load() {
        // `write_image` ends by draining the chip's last echo byte. The port can
        // be gone by then, and a loader that discarded that error would report
        // "the DA started" for a port that had already failed. The error has to
        // reach the caller, and nothing that looks like a successful load may
        // have gone out: no `0xAF` read-back, no `0xA8` jump.
        let image = da_image();
        let mut device = bootrom();
        device.failing_clear_input();
        let err =
            load(&mut device, &image).expect_err("the drain failed, so the load did not happen");
        assert!(matches!(err, Error::Io { .. }), "got {err}");
        assert!(
            format!("{err}").contains("went away"),
            "the port's own error is carried: {err}"
        );
        assert!(!device.started(), "a failed load must not start the DA");
        assert!(
            device.opcodes().iter().all(|byte| *byte == OPCODE_WRITE16),
            "the failure happened before the read-back: {:02X?}",
            device.opcodes()
        );
    }

    #[test]
    fn the_loader_refuses_an_image_that_is_not_exactly_the_da_size() {
        let full = da_image();
        let mut long = da_image();
        long.extend_from_slice(&[0u8; 4]);
        let cases: [(&str, Vec<u8>); 3] = [
            (
                "two short",
                full.iter().copied().take(DA_IMAGE_SIZE - 2).collect(),
            ),
            (
                "one short",
                full.iter().copied().take(DA_IMAGE_SIZE - 1).collect(),
            ),
            ("long", long),
        ];
        for (label, image) in cases {
            let mut device = bootrom();
            let err = load(&mut device, &image).expect_err("a wrong size is refused");
            assert!(matches!(err, Error::Safety { .. }), "{label}: got {err}");
            assert!(
                format!("{err}").contains("14896"),
                "{label}: the refusal names the size: {err}"
            );
            assert_eq!(device.opcodes(), &[] as &[u8], "{label}: nothing was sent");
            assert_eq!(device.bootrom_writes(), 0, "{label}");
        }
    }

    #[test]
    fn the_loader_refuses_a_da_that_is_empty() {
        // Zero bytes is the shortest wrong size there is, and the one a mistyped
        // `--da` path with a placeholder file produces.
        let mut device = bootrom();
        let err = load(&mut device, &[]).expect_err("an empty file is not a DA");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(device.opcodes(), &[] as &[u8], "nothing was sent");
        assert_eq!(device.bootrom_writes(), 0);
    }

    #[test]
    fn the_loader_refuses_without_a_confirmation_and_sends_nothing() {
        let err = Confirmation::of_probe_command_line(false)
            .err()
            .unwrap_or_else(|| panic!("no flag must be refused"));
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(text.contains("--yes"), "got {text}");
        assert!(
            !text.contains("\"LOAD\""),
            "fw probe-da has no typed prompt, so the refusal must not ask for a word nobody \
             reads: {text}"
        );

        let mut device = bootrom();
        let mut loader = DaLoader::new(&mut device);
        let err = loader
            .load(Confirmation { given: false }, &da_image())
            .expect_err("a token that was never granted is refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(device.opcodes(), &[] as &[u8]);
    }

    #[test]
    fn the_loader_refuses_an_image_whose_first_bytes_are_not_the_da() {
        // The size-only check let any 14,896-byte file through to a RAM write
        // and a jump. `load` documents a first-bytes check; this is what makes
        // that true, and the sizes below are all exactly right.
        assert!(check_da_image(&da_image()).is_ok(), "the fixture is the DA");

        let wrong_head = {
            let mut image = da_image();
            image[..DA_IMAGE_HEAD.len()].copy_from_slice(&VECTOR);
            image
        };
        // The firmware's vector at the DA's address: same length, right size,
        // and an image the old check would have loaded.
        let err = check_da_image(&wrong_head).expect_err("the firmware head is not the DA");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("A0 E1") && text.contains("F0, 9F, E5"),
            "the refusal names both the bytes it found and the ones it wanted: {text}"
        );
        // And through `load`: nothing is sent and nothing is written.
        let mut device = bootrom();
        let err = load(&mut device, &wrong_head).expect_err("refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(device.opcodes(), &[] as &[u8], "nothing goes on the wire");
        assert_eq!(device.bootrom_writes(), 0);
        assert!(!device.started());

        // One byte off in the last of the four.
        let mut last_byte = da_image();
        last_byte[DA_IMAGE_HEAD.len() - 1] ^= 0xFF;
        assert!(matches!(
            check_da_image(&last_byte),
            Err(Error::Safety { .. })
        ));
    }

    #[test]
    fn a_da_whose_report_is_not_this_da_is_refused_after_the_load() {
        // The fake answers `0xA8` with the report of the DA it models, whatever
        // the loaded image holds: it does not execute the image. So this drives
        // `parse_report`, which is the check `load` runs on what it read, with
        // one field of a valid report altered at a time.
        let cases: [(&str, usize, [u8; 2], &str); 4] = [
            ("version", 1, [0x03, 0x09], "0x0309"),
            ("region", 3, [0x46, 0x49], "0x46"),
            ("manufacturer", 8, [0x00, 0xC3], "00C3"),
            ("device id", 10, [0x22, 0xBB], "22BB"),
        ];
        assert!(
            parse_report(&report_bytes()).is_ok(),
            "the unaltered report is this DA"
        );
        for (label, at, value, expected) in cases {
            let mut report = report_bytes();
            for (slot, byte) in report.iter_mut().skip(at).zip(value.iter()) {
                *slot = *byte;
            }
            let err = parse_report(&report).expect_err("a wrong report is refused");
            assert!(matches!(err, Error::Framing { .. }), "{label}: got {err}");
            assert!(
                format!("{err}")
                    .to_uppercase()
                    .contains(&expected.to_uppercase()),
                "{label}: the refusal names the field value: {err}"
            );
        }
        // And a report of the wrong length never reaches a field.
        let full_report = report_bytes();
        let short = full_report.get(..19).unwrap();
        let err = parse_report(short).expect_err("a short report is refused");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
    }

    #[test]
    fn the_read_back_is_what_the_loader_decides_on_before_it_jumps() {
        // The fake's RAM starts zeroed, so a loader that skipped `read_back`
        // would pass as long as the write landed; this drives the comparison
        // directly and checks the jump cannot follow a mismatch.
        let image = da_image();
        let mut altered = image.clone();
        let offset = DA_LOAD_CHUNK + 3;
        if let Some(slot) = altered.get_mut(offset) {
            *slot ^= 0xFF;
        }
        let mut device = bootrom();
        let mut loader = DaLoader::new(&mut device).with_reply_timeout(Duration::from_millis(50));
        loader.write_image(&image).expect("write");
        let err = loader
            .read_back(&altered)
            .expect_err("the read-back differs");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(
            format!("{err}").contains(&format!("0x{offset:X}")),
            "the first differing offset is named: {err}"
        );
        assert!(!device.started(), "no jump after a mismatch");
    }

    #[test]
    fn a_silent_port_after_the_jump_is_a_timeout_not_a_framing_error() {
        // A chip that never answers `0xA8` leaves the loader waiting for a
        // banner that is not coming. A port that stays silent is a timeout: no
        // bytes arrived, so nothing says the image was wrong. `Framing` is the
        // other case, handled by
        // `bytes_that_are_not_a_report_end_the_scan_as_a_framing_error`.
        let mut device = bootrom();
        // No jump, so the fake stays in the BootROM and offers nothing: the
        // report read has to give up rather than spin.
        let mut loader = DaLoader::new(&mut device).with_reply_timeout(Duration::from_millis(20));
        let err = loader
            .read_report()
            .expect_err("nothing is queued before the jump");
        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(
            format!("{err}").contains("first report byte"),
            "the timeout names what it waited for: {err}"
        );
    }

    #[test]
    fn bytes_that_are_not_a_report_end_the_scan_as_a_framing_error() {
        // The other ending: the chip sends something and none of it starts a
        // `0xC0` report within the scan's bound. That is `Framing` — the chip is
        // talking, just not the DA this crate drives. The bytes come from a thin
        // transport rather than the fake because the fake would parse them as a
        // BootROM frame; the scan is what is under test here.
        #[derive(Debug)]
        struct Noise {
            left: usize,
        }

        impl Transport for Noise {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                // A chip that keeps printing: every read is answered with the
                // same byte, so the scan reaches its own bound instead of
                // timing out first.
                let take = self.left.min(buf.len());
                buf[..take].fill(0x7D);
                self.left -= take;
                Ok(take)
            }

            fn write_all(&mut self, _buf: &[u8]) -> Result<()> {
                Ok(())
            }

            fn flush(&mut self) -> Result<()> {
                Ok(())
            }

            fn clear_input(&mut self) -> Result<()> {
                Ok(())
            }

            fn set_baud(&mut self, _baud: u32) -> Result<()> {
                Ok(())
            }

            fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
                Ok(())
            }
        }

        // `0x7D` is the one byte the DA sends while it waits; a long run of it
        // never starts a report.
        let mut noise = Noise {
            left: REPORT_SCAN_LIMIT,
        };
        let mut loader = DaLoader::new(&mut noise).with_reply_timeout(Duration::from_millis(20));
        let err = loader
            .read_report()
            .expect_err("no C0 report in the stream");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(
            format!("{err}").contains("0xC0 report"),
            "the refusal says what was missing: {err}"
        );
    }

    // -- 0xD6 --------------------------------------------------------------

    /// A chip's flash window: 512 KiB, every byte `fill`.
    fn flash(fill: u8) -> Vec<u8> {
        vec![fill; 0x0008_0000]
    }

    /// Put `bytes` at `address` in a window.
    fn place(window: &mut [u8], address: usize, bytes: &[u8]) {
        if let Some(slot) = window.get_mut(address..address + bytes.len()) {
            slot.copy_from_slice(bytes);
        }
    }

    /// A chip in the DA, with `window` as the flash the DA sees.
    ///
    /// Built by hand rather than with [`crate::simulator::enter_download_agent`]:
    /// that helper's jump consumes only one of the two effects a real jump has,
    /// and the read tests are about what happens *after* the DA is available.
    fn da_with(window: &[u8]) -> FakeDa {
        let mut device = FakeDa::with_firmware(window);
        enter_da(&mut device);
        device
    }

    /// Jump to the DA and drain the banner, leaving the fake in its command loop.
    fn enter_da(device: &mut FakeDa) {
        let jump = Frame {
            opcode: OPCODE_JUMP,
            address: BASE_ADDR_DA,
            count: 0,
        };
        device.write_all(&jump.to_bytes()).expect("jump");
        let mut report = [0u8; DA_REPORT_SIZE];
        let mut filled = 0usize;
        while filled < report.len() {
            let read = device.read(&mut report[filled..]).expect("the banner");
            assert_ne!(read, 0, "the banner stopped {filled} bytes in");
            filled += read;
        }
        assert_eq!(report.first().copied(), Some(DA_REPORT_MARKER));
    }

    /// `0xD6` through the crate's own reader.
    ///
    /// `DownloadAgent` owns its transport (as it does for the write path), so
    /// the fake is moved in and handed back for the assertions.
    fn read(device: FakeDa, address: u32, length: usize) -> (Result<Vec<u8>>, FakeDa) {
        let mut da = DownloadAgent::new(device).with_reply_timeout(Duration::from_millis(50));
        let result = da.read_range(address, length);
        (result, da.into_transport())
    }

    /// The opcodes sent *after* the jump [`enter_da`] makes, which is what the
    /// read tests are about.
    #[must_use]
    fn read_opcodes(device: &FakeDa) -> &[u8] {
        device
            .opcodes()
            .split_first()
            .filter(|(opcode, _)| **opcode == OPCODE_JUMP)
            .map_or(device.opcodes(), |(_, rest)| rest)
    }

    #[test]
    fn the_read_returns_the_window_at_the_address_asked_for() {
        let mut window = flash(0x00);
        let data: Vec<u8> = (0..PROBE_PAGE_SIZE)
            .map(|index| 0xA0 + index as u8)
            .collect();
        place(&mut window, 0x100, &data);
        let (page, device) = read(da_with(&window), 0x100, PROBE_PAGE_SIZE);
        assert_eq!(page.expect("0xD6"), data);
        assert_eq!(device.read_requested(), Some((0x100, PROBE_PAGE_SIZE)));
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE],
            "0xD6 and nothing else: the advance byte is part of the page \
             conversation, not a command"
        );
        assert_eq!(
            device.output_len(),
            0,
            "the DA sends nothing when the range is complete"
        );
    }

    #[test]
    fn four_bytes_left_over_from_the_banner_do_not_shift_the_first_page() {
        let mut window = flash(0x00);
        let data: Vec<u8> = (0..PROBE_PAGE_SIZE)
            .map(|index| 0xB0u8.wrapping_add(index as u8))
            .collect();
        place(&mut window, 0, &data);
        let mut device = da_with(&window);
        // Defensive case (⚠️ 未検証 in `docs/dev/05-verification.md`): if stale
        // bytes sit on the line after the twenty-byte banner, they remain ahead
        // of `0xD6` until the pre-read drain clears them.
        device.queue_output(&[0x7D, 0x00, 0x00, 0x00]);

        let (page, device) = read(device, 0, PROBE_PAGE_SIZE);
        assert_eq!(
            page.expect("0xD6 after a leftover prefix"),
            data,
            "the read must start at the page, not four bytes into it"
        );
        assert_eq!(
            device.retries_sent(),
            0,
            "the pre-read drain must take the leftovers without needing a retry"
        );
        assert_eq!(
            device.output_len(),
            0,
            "the drain that starts the read took the leftovers"
        );
    }

    #[test]
    fn a_page_that_arrives_behind_four_bytes_is_asked_for_again_and_read_clean() {
        let mut window = flash(0x00);
        let data: Vec<u8> = (0..PROBE_PAGE_SIZE)
            .map(|index| 0xC0u8.wrapping_add(index as u8))
            .collect();
        place(&mut window, 0, &data);
        let mut device = da_with(&window);
        // Defensive case (⚠️ 未検証 in `docs/dev/05-verification.md`): leading
        // bytes arrive after the command, in front of the page, so the clear
        // that starts the read cannot see them.
        device.prefix_next_page(&[0x7D, 0x00, 0x00, 0x00]);

        let (page, device) = read(device, 0, PROBE_PAGE_SIZE);
        assert_eq!(
            page.expect("0xD6 after a page with a prefix"),
            data,
            "the retry must drop the leftovers and start at the page"
        );
        assert_eq!(device.retries_sent(), 1, "exactly one re-request");
        assert_eq!(
            device.output_len(),
            0,
            "nothing is left once the clean page has been read"
        );
    }

    #[test]
    fn a_multi_page_read_resynchronises_after_a_prefix_on_the_first_page() {
        let mut window = flash(0x00);
        let expected: Vec<u8> = (0..2 * PROBE_PAGE_SIZE)
            .map(|index| 0xD0u8.wrapping_add(index as u8))
            .collect();
        place(&mut window, 0, &expected);
        let mut device = da_with(&window);
        device.prefix_next_page(&[0x7D, 0x00, 0x00, 0x00]);

        let (pages, device) = read(device, 0, 2 * PROBE_PAGE_SIZE);
        assert_eq!(
            pages.expect("two pages after a prefix on page 1"),
            expected,
            "page 1 retries cleanly and page 2 aligns with PAGE_ADVANCE"
        );
        assert_eq!(device.retries_sent(), 1, "only page 1 needed a retry");
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE],
            "one 0xD6; the retry and PAGE_ADVANCE bytes stay inside the page conversation"
        );
        assert_eq!(
            device.output_len(),
            0,
            "no leftover bytes remain after the second page"
        );
    }

    #[test]
    fn a_failing_pre_read_drain_propagates_the_error_without_sending_read_range() {
        let window = flash(0x00);
        let mut device = da_with(&window);
        device.failing_clear_input();

        let (result, device) = read(device, 0, PROBE_PAGE_SIZE);
        let err = result.expect_err("the pre-read drain failed");
        assert!(matches!(err, Error::Io { .. }), "got {err}");
        assert!(
            format!("{err}").contains("went away"),
            "the port's own error is carried: {err}"
        );
        assert_eq!(
            read_opcodes(&device),
            &[] as &[u8],
            "0xD6 must not be sent when clearing the line fails"
        );
    }

    #[test]
    fn a_failing_retry_drain_propagates_the_error_without_sending_retry() {
        /// Lets the pre-read `clear_input` succeed, then arms `failing_clear_input`
        /// so the retry-side `clear_input` fails before `0xA5` goes out.
        #[derive(Debug)]
        struct FailRetryDrain(FakeDa);

        impl Transport for FailRetryDrain {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                self.0.read(buf)
            }

            fn write_all(&mut self, buf: &[u8]) -> Result<()> {
                self.0.write_all(buf)
            }

            fn flush(&mut self) -> Result<()> {
                self.0.flush()
            }

            fn clear_input(&mut self) -> Result<()> {
                let result = self.0.clear_input();
                self.0.failing_clear_input();
                result
            }

            fn set_baud(&mut self, baud: u32) -> Result<()> {
                self.0.set_baud(baud)
            }

            fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
                self.0.set_control_lines(asserted)
            }
        }

        let window = flash(0x00);
        let mut device = da_with(&window);
        device.prefix_next_page(&[0x7D, 0x00, 0x00, 0x00]);
        let mut da = DownloadAgent::new(FailRetryDrain(device))
            .with_reply_timeout(Duration::from_millis(50));
        let err = da
            .read_range(0, PROBE_PAGE_SIZE)
            .expect_err("the retry drain failed");
        assert!(matches!(err, Error::Io { .. }), "got {err}");
        assert!(
            format!("{err}").contains("went away"),
            "the port's own error is carried: {err}"
        );
        let device = da.into_transport().0;
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE],
            "0xD6 was sent before the page checksum failed"
        );
        assert_eq!(
            device.retries_sent(),
            0,
            "0xA5 must not be sent when clearing the line on retry fails"
        );
    }

    #[test]
    fn a_multi_page_read_advances_once_per_page_and_sends_nothing_after_the_last() {
        let mut window = flash(0x5A);
        place(&mut window, 0x200, &[0xFF; 4]);
        let (page, device) = read(da_with(&window), 0x200, 4 * PROBE_PAGE_SIZE);
        let page = page.expect("four pages");
        assert_eq!(page.len(), 4 * PROBE_PAGE_SIZE);
        assert_eq!(page.get(..4), Some(&[0xFFu8; 4][..]));
        assert_eq!(
            page.get(4..PROBE_PAGE_SIZE),
            Some(&[0x5Au8; PROBE_PAGE_SIZE - 4][..]),
            "the rest is the window, page by page, in order"
        );
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE],
            "one 0xD6: the advance bytes are the page conversation, not commands"
        );
        assert_eq!(
            device.output_len(),
            0,
            "the last advance draws no reply; an extra command would have been counted"
        );
    }

    #[test]
    fn a_page_with_a_bad_checksum_is_asked_for_again_and_the_retry_is_returned() {
        let window = flash(0x00);
        let (page, device) = read(
            da_with(&window).corrupting_page(1),
            0x400,
            3 * PROBE_PAGE_SIZE,
        );
        let page = page.expect("the retry succeeds");
        assert_eq!(page.len(), 3 * PROBE_PAGE_SIZE);
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE],
            "one 0xD6: the retry and the advances are the page conversation"
        );
        assert_eq!(
            page.get(PROBE_PAGE_SIZE..2 * PROBE_PAGE_SIZE),
            Some(&[0u8; PROBE_PAGE_SIZE][..]),
            "the page kept is the one that passed the checksum"
        );
    }

    #[test]
    fn a_page_that_stays_wrong_is_refused_after_a_bounded_number_of_retries() {
        let window = flash(0x00);
        let mut device = da_with(&window);
        device.set_always_corrupt_page(0);
        let (page, device) = read(device, 0, PROBE_PAGE_SIZE);
        let err = page.expect_err("never correct");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(
            format!("{err}").contains("checksum"),
            "the refusal names the checksum: {err}"
        );
        // The retries are the client's `0xA5` bytes, which the page
        // conversation consumes rather than treating as commands; the fake
        // counts them separately.
        assert_eq!(
            device.retries_sent(),
            READ_RETRIES,
            "exactly the documented re-requests"
        );
    }

    #[test]
    fn a_read_length_that_is_not_whole_pages_is_refused_before_anything_is_sent() {
        let window = flash(0x00);
        for length in [0usize, 1, PROBE_PAGE_SIZE - 1, PROBE_PAGE_SIZE + 1] {
            let (result, device) = read(da_with(&window), 0, length);
            let err = result.expect_err("not a whole page");
            assert!(matches!(err, Error::State(_)), "{length}: got {err}");
            assert_eq!(
                read_opcodes(&device),
                &[] as &[u8],
                "{length}: nothing sent"
            );
        }
        let (result, _) = read(da_with(&window), u32::MAX - 32, PROBE_PAGE_SIZE);
        let err = result.expect_err("overflow");
        assert!(matches!(err, Error::State(_)), "got {err}");
    }

    #[test]
    fn the_read_path_never_sends_a_write_opcode() {
        let window = flash(0x00);
        let (first, device) = read(da_with(&window), 0, PROBE_PAGE_SIZE);
        first.expect("the candidate at 0");
        let (second, device) = read(device, 0x0020_0000, PROBE_PAGE_SIZE);
        second.expect("the candidate at 0x0020_0000");
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE, OPCODE_READ_RANGE],
            "two reads and nothing else: {:02X?}",
            device.opcodes()
        );
        assert_eq!(device.recorded_range(), (0, 0), "no range was recorded");
        assert!(!device.wrote_an_empty_range());
        assert!(!device.restarted());
    }

    #[test]
    fn the_probe_path_reads_both_candidates_and_nothing_else() {
        // `read_candidates` is the only non-test caller of `read_range`, and
        // this is where its two addresses come from: `PROBE_ADDRESSES`, in
        // order, with no parameter to widen them. A probe reduced to one
        // candidate, or pointed at a third address, fails here.
        let window = flash(0x00);
        let mut da =
            DownloadAgent::new(da_with(&window)).with_reply_timeout(Duration::from_millis(50));
        let results = read_candidates(&mut da);
        assert_eq!(results.len(), 2);
        let device = da.into_transport();
        assert_eq!(
            device.read_requested(),
            Some((PROBE_ADDRESSES[1], PROBE_PAGE_SIZE)),
            "the second read is the second candidate, at the page size the probe asks for"
        );
        assert_eq!(
            read_opcodes(&device),
            &[OPCODE_READ_RANGE, OPCODE_READ_RANGE],
            "one 0xD6 per candidate, in order: {:02X?}",
            device.opcodes()
        );
        assert_eq!(device.recorded_range(), (0, 0), "no range was recorded");
        assert!(!device.wrote_an_empty_range());
        assert!(!device.restarted());
        // Both were read: each result is a whole page.
        for result in &results {
            assert_eq!(
                result
                    .as_ref()
                    .map(Vec::len)
                    .expect("both candidates were read"),
                PROBE_PAGE_SIZE,
                "a candidate that the DA answers is a whole page"
            );
        }
    }

    #[test]
    fn the_fake_pads_a_partial_page_by_default_and_can_send_it_short() {
        // §5.5: a read page goes through the same serialiser as `0xD5`, which
        // pads it to the page length. Whether a DA really pads a **partial**
        // page is unmeasured (§7 item 4), so the fake can do either. Note what
        // `read_range` guarantees: it refuses a length that is not a whole page
        // (`0xD6 reads whole 64-byte pages`), so a partial page can only arise
        // from a DA whose page size differs from the one asked for. What is
        // pinned here is the fake's own model, so a later test knows which bytes
        // it is comparing.
        let mut window: Vec<u8> = Vec::new();
        window.resize(0x400, 0x11);
        place(&mut window, 0x180, &[0x3C]);
        let total = 40;
        let mut frame = vec![OPCODE_READ_RANGE];
        frame.extend_from_slice(&u32::try_from(PROBE_PAGE_SIZE).unwrap().to_be_bytes());
        frame.extend_from_slice(&0x0100u32.to_be_bytes());
        frame.extend_from_slice(&u32::try_from(total).unwrap().to_be_bytes());

        let first_reply = |device: &mut FakeDa| -> Vec<u8> {
            device.write_all(&frame).expect("0xD6");
            let mut reply = vec![0u8; PROBE_PAGE_SIZE + 2];
            let mut filled = 0usize;
            while filled < reply.len() {
                let read = device.read(&mut reply[filled..]).expect("a page");
                if read == 0 {
                    break;
                }
                filled += read;
            }
            reply.truncate(filled);
            reply
        };

        let mut padded = da_with(&window);
        let whole = first_reply(&mut padded);
        assert_eq!(
            whole.len(),
            PROBE_PAGE_SIZE + 2,
            "the padded fake sends a whole page plus its trailer"
        );
        assert_eq!(
            whole.get(..4),
            Some(&[0x11u8, 0x11, 0x11, 0x11][..]),
            "the page starts at the address the range asked for"
        );
        assert_eq!(
            whole.get(PROBE_PAGE_SIZE - 4..PROBE_PAGE_SIZE),
            Some(&[0u8; 4][..]),
            "the bytes past the range are zero, not the window's filler"
        );

        let mut short = da_with(&window);
        short.without_page_padding();
        let tail = first_reply(&mut short);
        assert_eq!(
            tail.len(),
            total + 2,
            "the unpadded fake sends only the bytes in the range"
        );
        assert_eq!(
            whole.get(..total),
            tail.get(..total),
            "the two modes differ only in the padding that follows"
        );

        // A range that is a whole number of pages is served identically, which
        // is why `read_range`'s refusal of a partial page costs no coverage.
        let aligned = {
            let (page, _device) = read(da_with(&window), 0x0100, 3 * PROBE_PAGE_SIZE);
            page.expect("three whole pages, padded")
        };
        let aligned_short = {
            let mut device = da_with(&window);
            device.without_page_padding();
            let (page, _device) = read(device, 0x0100, 3 * PROBE_PAGE_SIZE);
            page.expect("three whole pages, unpadded")
        };
        assert_eq!(aligned.len(), 3 * PROBE_PAGE_SIZE);
        assert_eq!(
            aligned, aligned_short,
            "a whole-page read must not depend on the fake's padding mode"
        );
        assert_eq!(
            aligned.get(0x80..0x84),
            Some(&[0x3Cu8, 0x11, 0x11, 0x11][..]),
            "the third page starts at the address the range asked for"
        );
    }
}
