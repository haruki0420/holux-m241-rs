//! A fake Download Agent, for the write crate's own tests.
//!
//! The DA is the half of the protocol that erases and programs flash, so its
//! model belongs in this crate for the same reason the driver does: `m241-core`
//! must not contain a working description of a destructive command set, not even
//! in a test fixture that ships. `m241_core::protocol::simulator` stops at the
//! BootROM and answers a `0xA8` jump with what the wire carries, without changing
//! state; this fake picks the jump up and speaks the DA's own byte stream.
//!
//! # What it models, and from where
//!
//! Everything here is taken from `docs/reference/01-da-protocol.md`:
//!
//! * the twenty-byte info report after `0xA8` (§2);
//! * `0xD3`: a count byte and count pairs of `(start, end)`. Success is `5A 00`,
//!   a refusal is a **single** `A5`, and a refused command records nothing, so
//!   the ranges of an earlier accepted command stay in place (§4.1, §5.3);
//! * `0xD5`: a length `u32`, two `5A`, then per packet `length` bytes plus a
//!   16-bit sum, answered with `0x69` and finally two `5A` (§5.6);
//! * `0xD9`: no argument and **no reply** (§5).
//!
//! Two facts the earlier notes got wrong are modelled deliberately, each with a
//! test that fails if it regresses:
//!
//! * **`0xD3` erases nothing.** Its handler calls no flash driver; it records the
//!   range and resets the iterators (§4.6).
//! * **The erase is inside `0xD5`, per sector, before that sector is
//!   programmed** (`0x0010251C`–`0x00102560`). The fake erases the whole recorded
//!   range when the command is accepted, so a packet can never be programmed
//!   into a sector that was not erased first; *when* within the command is not
//!   visible on the wire.
//!
//! # What it deliberately does not model
//!
//! The firmware mode a `0xD9` boots into — that is PMTK territory and belongs to
//! `m241_core`'s fake. Here a restart leaves the chip ready for a fresh BootROM
//! sync, which is what a write client has to do next anyway. Also absent:
//! timing, the progress frames `0xD4` emits, and the flash driver's own error
//! codes. The tests that need a failure ask for one explicitly
//! ([`FakeDa::refusing_packets_from`], [`FakeDa::set_failing_first_erase`]).
//!
//! Nothing here has been checked against a unit: the fake is a model of
//! `docs/reference/01-da-protocol.md`, not of measured behaviour.

use std::collections::VecDeque;

use m241_core::Result;
use m241_core::protocol::brom::{
    BASE_ADDR_DA, BASE_ADDR_FIRMWARE, BOOT_REPLY, BOOT_SYNC, Frame, OPCODE_READ16, OPCODE_READ32,
    OPCODE_WRITE16,
};
use m241_core::protocol::frame::swap_pairs;
use m241_core::protocol::port::Transport;

use crate::da::{
    ACK, CONT, DA_JUMP_COUNT, NAK, OPCODE_MEM_CMD, OPCODE_READ_RANGE, OPCODE_RESTART, OPCODE_WRITE,
    PROBE_PAGE_SIZE,
};

/// Status byte the fake DA returns after a successful `0xD3`.
const STATUS_OK: u8 = 0x00;

/// Opcode that hands the chip from the BootROM to the DA.
const OPCODE_JUMP: u8 = 0xA8;

/// Default payload bytes per DA packet.
///
/// Only the fallback when a `0xD5` carries a zero length; the client's value is
/// [`crate::da::PACKET_SIZE`].
const WRITE_PACKET_LEN: usize = 0x100;

/// Bytes of flash the fake DA reports, and the span a `0xD3` range may cover.
///
/// `measured elsewhere:` the retail unit's 20-byte report carries `0x0008_0000`
/// in its total-bytes field; the DA's own linear view of the part is therefore
/// `0x0000_0000`–`0x0007_FFFF` (`docs/reference/01-da-protocol.md` §4.2).
const FLASH_SIZE: u32 = 0x0008_0000;

/// Erase granularity the fake chip models, in bytes.
///
/// The real erase is per sector, resolved by the DA from the descriptor the
/// region entry points at (`0x00103EA0` on the retail part). That descriptor's
/// fields are not decoded by `docs/reference/01-da-protocol.md` §6, so no exact
/// size can be cited and this one value is the model's own choice: the retail
/// MX29LV400CB is a bottom-boot part, whose sectors are not all the same size,
/// and the fake collapses that to one uniform sector. What the model enforces,
/// and the tests pin, is the property a client has to live with: `0xD5` erases
/// whole sectors, so the bytes around a written range are destroyed too.
const FLASH_SECTOR_SIZE: u32 = 0x4000;

/// Bytes of DA RAM the fake models, from [`BASE_ADDR_DA`].
///
/// `DA_v3.8.4.bin` covers `0x00100800`–`0x0010422F`, so the image is
/// [`crate::da::DA_IMAGE_SIZE`] bytes and the runtime cells the banner's last
/// word reads (`0x00104330`, `docs/reference/01-da-protocol.md` §2) sit just
/// past it. The window is one 16 KiB page, which holds both with room to spare
/// and makes a write past the image visible as a byte that changed rather than
/// as a silently truncated load.
const DA_RAM_SIZE: usize = 0x4000;

/// The mode the fake chip is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Running firmware: bytes are dropped, which is what a BootROM read gets
    /// from a unit that has not been handed over yet.
    Firmware,
    /// BootROM: nine-byte frames, echoed. A jump leaves this mode.
    BootRom,
    /// Download agent: the info report has been sent, commands are accepted.
    DownloadAgent,
}

/// Which chip writes to the fake should be addressed to.
///
/// The fake is one state machine with three modes and no way to tell from the
/// bytes alone which one a test means next, since the DA and the BootROM share
/// the wire. [`FakeDa::set_modelled`] is the switch; [`Mode`] is what it set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modelled {
    /// The running firmware: nothing is answered.
    Firmware,
    /// The BootROM: nine-byte frames, every byte echoed, `0xA8` a no-op.
    BootRom,
    /// The download agent: the info report, `0xD3` / `0xD5` / `0xD9`.
    DownloadAgent,
}

/// Where the DA state machine has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaStage {
    /// Accepting a command frame.
    Command,
    /// Accepting one packet: payload plus a two-byte sum.
    Packet,
}

/// A fake chip that answers the BootROM jump and then the DA's commands.
#[derive(Debug)]
pub struct FakeDa {
    /// The flash window, in the DA's own linear view: address 0 is its first
    /// byte. `m241_core`'s fake maps the same flash at the BootROM's base; the
    /// two views are the same bytes at different addresses, which is why the
    /// client's addresses are used here without adjustment.
    memory: Vec<u8>,
    /// Bytes waiting to be read by the host.
    output: VecDeque<u8>,
    /// The frame being assembled while the chip is in the DA.
    line: Vec<u8>,
    /// The frame being assembled while the chip is in the BootROM.
    ///
    /// A separate buffer because `0xA8` hands the chip over part-way through
    /// its own frame and leaves nothing half-read behind.
    bootrom_line: Vec<u8>,
    mode: Mode,
    da_stage: DaStage,
    /// The range `0xD3` recorded, and the write cursor `0xD5` advances in it.
    ///
    /// The fake starts at `(0, 0)`: "nothing recorded". A `0xD5` sent without a
    /// preceding `0xD3` therefore erases and programs the sector at address
    /// zero, which is the destructive sequence the safety rules exist for, and
    /// no client may send it. Recording a range does not erase it, and a refused
    /// `0xD3` does not replace it: only a following `0xD5` erases, and only an
    /// accepted `0xD3` assigns.
    da_range: (u32, u32),
    da_cursor: u32,
    da_packet_len: usize,
    /// The first packet index [`FakeDa::refusing_packets_from`] refuses.
    refuse_from: usize,
    /// How many packets from `refuse_from` on are refused.
    refuse_count: usize,
    /// How many packets this session accepted, which a test compares with the
    /// client's own `0x69` count.
    accepted_packets: usize,
    /// Every payload a `0xD5` packet carried, in arrival order.
    ///
    /// Recorded so a test can see what the client actually put on the wire —
    /// the zero padding of the short final packet, and its checksum — rather
    /// than inferring it from the flash window.
    packets: Vec<Vec<u8>>,
    /// The DA's own RAM, addressed from zero, so a `0xA1` BootROM write before
    /// the jump lands somewhere and a `0xAF` read can see it.
    ///
    /// Beyond the model of `DA_v3.8.4.bin` itself: the fake does not execute
    /// the loaded image, it marks the jump as taken once the image arrived. The
    /// window is what makes "the write landed" and "the read-back agrees"
    /// observable, which is the part of the loader the client owns.
    ram: Vec<u8>,
    /// How many `0xA1` frames landed in `ram`.
    bootrom_writes: usize,
    /// Whether a `0xA8` to [`BASE_ADDR_DA`] was taken (the loader's jump).
    started: bool,
    /// Which page of the current `0xD6`, and where it starts in the window.
    read_page: usize,
    /// How many pages the current `0xD6` range has (§5.5's
    /// `udiv(total + page - 1, page)`).
    read_pages: usize,
    read_data_debug: (u32, usize),
    read_start: u32,
    read_page_len: usize,
    /// The `0xD6` result still to be sent, page by page.
    read_data: Vec<u8>,
    /// The bytes of the last `0xD6` range a test can compare with the window.
    read_requested: Option<(u32, usize)>,
    /// A page the fake should send with a deliberately wrong checksum, once.
    corrupt_page: Option<usize>,
    /// A page the fake should always send with a wrong checksum.
    always_corrupt_page: Option<usize>,
    /// Whether a page shorter than the requested one is zero-filled over the
    /// wire or sent short.
    ///
    /// §4.4 says the *serialiser* pads a short payload, and the read path uses
    /// the same one (§5.5). A plain `true` would make the client and the fake
    /// agree by construction; the flag lets a test set it to `false` and check
    /// that the client tolerates a short page as well.
    pad_pages: bool,
    /// How many `0xA1` frames arrived with `count != 0`.
    bad_jump_counts: usize,
    /// How many times the host sent `0xA5` to ask for a page again.
    retries_sent: usize,
    /// Every opcode the host sent, in order: the BootROM's frames and the DA's.
    opcodes: Vec<u8>,
    /// The two-byte trailer of each packet, as it arrived, parallel to
    /// `packets`.
    trailers: Vec<[u8; 2]>,
    /// Whether a `0xD9` has been seen.
    restarted: bool,
    /// Whether every `0xD3` is refused, as an unmappable range would be.
    refuse_ranges: bool,
    /// Whether the first sector erase of the next `0xD5` fails.
    ///
    /// `0x0010253C`–`0x0010254C`: the DA answers a single `0xA5` there instead
    /// of the second `0x5A` (`docs/reference/01-da-protocol.md` §5.6).
    fail_first_erase: bool,
    /// Whether the sector erase inside a `0xD5` has failed.
    erase_failed: bool,
    /// Whether a `0xD5` arrived while the recorded range was still `(0, 0)`.
    ///
    /// That is the command that erases the reset vector; recording it lets a
    /// test assert the client never sent one.
    wrote_an_empty_range: bool,
    /// The address of the last BootROM read, `None` before the first.
    ///
    /// A test compares it with `BASE_ADDR_FIRMWARE` to see which address the
    /// client asked for, instead of relying on this fake's own shift.
    last_read_address: Option<u32>,
    /// Whether the BootROM frame currently being assembled is echoed.
    ///
    /// Reads are echoed; `0xA8` is not, because it hands control over
    /// part-way through (`measured elsewhere`).
    echo_frame: bool,
    /// How many sync probes have been answered since the last complete sync,
    /// or `0` when no sync is in progress.
    bootrom_sync: usize,
    /// Bytes the host has taken off the wire, in total.
    ///
    /// A read returns what is buffered and `Ok(0)` once it is empty, so an extra
    /// read leaves no trace in the bytes that follow — the next reply simply
    /// starts at the same place. This half of the pair says *how much left*.
    read_bytes: usize,
    /// How many times the host has called `read`, empty results included.
    ///
    /// The other half of the pair: an extra read that finds nothing moves
    /// `read_bytes` by zero but still shows up here.
    read_polls: usize,
    /// Whether every `clear_input` fails, as a port that went away does.
    clear_input_fails: bool,
}

impl FakeDa {
    /// A chip holding `image` in its flash window, ready for the BootROM jump.
    #[must_use]
    pub fn with_firmware(image: &[u8]) -> Self {
        Self {
            memory: image.to_vec(),
            output: VecDeque::new(),
            line: Vec::new(),
            bootrom_line: Vec::new(),
            mode: Mode::BootRom,
            da_stage: DaStage::Command,
            da_range: (0, 0),
            da_cursor: 0,
            da_packet_len: WRITE_PACKET_LEN,
            refuse_from: usize::MAX,
            refuse_count: 0,
            accepted_packets: 0,
            packets: Vec::new(),
            trailers: Vec::new(),
            ram: vec![0u8; DA_RAM_SIZE],
            bootrom_writes: 0,
            started: false,
            read_page: 0,
            read_pages: 1,
            read_data_debug: (0, 0),
            read_start: 0,
            read_page_len: PROBE_PAGE_SIZE,
            read_data: Vec::new(),
            read_requested: None,
            corrupt_page: None,
            always_corrupt_page: None,
            pad_pages: true,
            bad_jump_counts: 0,
            retries_sent: 0,
            opcodes: Vec::new(),
            restarted: false,
            refuse_ranges: false,
            fail_first_erase: false,
            erase_failed: false,
            wrote_an_empty_range: false,
            last_read_address: None,
            echo_frame: true,
            bootrom_sync: 0,
            read_bytes: 0,
            read_polls: 0,
            clear_input_fails: false,
        }
    }

    /// Refuse `count` packets starting at packet `skip` (zero-based).
    ///
    /// `skip` is how many packets are accepted before the first refusal, so a
    /// test can reach "the write failed two packets in" with one switch. Every
    /// packet from `skip` on is refused.
    #[must_use]
    pub fn refusing_packets_from(mut self, skip: usize, count: usize) -> Self {
        self.refuse_from = skip;
        self.refuse_count = count;
        self
    }

    /// Make the first sector erase of the next `0xD5` fail.
    ///
    /// `0x0010253C`–`0x0010254C`: the DA answers `0xA5` where the second `0x5A`
    /// would be, and the command ends there (`docs/reference/01-da-protocol.md`
    /// §5.6). ⚠️ 未検証 on hardware: read from the instruction sequence.
    pub fn set_failing_first_erase(&mut self, fail: bool) {
        self.fail_first_erase = fail;
    }

    /// Refuse every `0xD3`, as an unmappable range would be refused.
    ///
    /// The client's own range is pinned and valid, so this is how the
    /// one-byte refusal path is reached at all.
    #[must_use]
    pub fn refusing_ranges(mut self, refuse: bool) -> Self {
        self.refuse_ranges = refuse;
        self
    }

    /// Stop refusing `0xD3` on a fake that already exists.
    ///
    /// Used to check that a client which met a single-byte refusal can still
    /// drive the same fake with a well-formed command afterwards.
    pub fn set_accepting_ranges(&mut self) {
        self.refuse_ranges = false;
    }

    /// Start refusing every `0xD3` on a fake that already exists.
    ///
    /// The counterpart of [`FakeDa::set_accepting_ranges`], so one fake can
    /// serve an accepted range, a refusal, and an accepted range again.
    pub fn set_refusing_ranges(&mut self, refuse: bool) {
        self.refuse_ranges = refuse;
    }

    /// How many packets the fake accepted and programmed.
    #[must_use]
    pub fn accepted_packets(&self) -> usize {
        self.accepted_packets
    }

    /// Every `0xD5` payload the fake received, in arrival order.
    ///
    /// Each entry holds `da_packet_len` bytes as the client sent them,
    /// padding included. Empty before the first packet.
    #[must_use]
    pub fn packets(&self) -> &[Vec<u8>] {
        &self.packets
    }

    /// The trailing two bytes of the last packet: the 16-bit sum, on the wire.
    ///
    /// `None` before any packet. This is the client's checksum as received, not
    /// one recomputed here, so a test can compare it with a literal.
    #[must_use]
    pub fn last_packet_trailer(&self) -> Option<[u8; 2]> {
        self.trailers.last().copied()
    }

    /// The two-byte trailer of packet `index`, as it arrived.
    #[must_use]
    pub fn trailer_of(&self, index: usize) -> Option<[u8; 2]> {
        self.trailers.get(index).copied()
    }

    /// The address the last BootROM read asked for, if any.
    #[must_use]
    pub fn last_read_address(&self) -> Option<u32> {
        self.last_read_address
    }

    /// How many bytes the host has taken off the wire since the fixture was
    /// created.
    ///
    /// Paired with [`FakeDa::read_polls`], this is how a test observes what the
    /// client *read* rather than what the chip offered.
    #[must_use]
    pub fn read_bytes(&self) -> usize {
        self.read_bytes
    }

    /// How many times the host has called `read`, empty results included.
    ///
    /// `read_bytes` counts only bytes that left, so a read that finds an empty
    /// buffer moves it by zero. This counts the attempt itself.
    #[must_use]
    pub fn read_polls(&self) -> usize {
        self.read_polls
    }

    /// How many reply bytes are still waiting to be read.
    #[must_use]
    pub fn output_len(&self) -> usize {
        self.output.len()
    }

    /// The DA's RAM as the loader left it.
    #[must_use]
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// How many `0xA1` frames the BootROM side accepted.
    #[must_use]
    pub fn bootrom_writes(&self) -> usize {
        self.bootrom_writes
    }

    /// Whether a `0xA8` jump to [`BASE_ADDR_DA`] was taken.
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    /// Every opcode the host sent, in order: a BootROM frame's first byte, or a
    /// DA command's first byte.
    #[must_use]
    pub fn opcodes(&self) -> &[u8] {
        &self.opcodes
    }

    /// The last `0xD6` range, as `(start, length)`.
    #[must_use]
    pub fn read_requested(&self) -> Option<(u32, usize)> {
        self.read_requested
    }

    /// Send page `page` of the next `0xD6` with a wrong checksum, once.
    #[must_use]
    pub fn corrupting_page(mut self, page: usize) -> Self {
        self.corrupt_page = Some(page);
        self
    }

    /// Send page `page` with a wrong checksum on **every** attempt.
    ///
    /// The other half of [`Self::corrupting_page`]: this one is for proving the
    /// client's retry is bounded, so it must not clear itself after one page.
    pub fn set_always_corrupt_page(&mut self, page: usize) {
        self.always_corrupt_page = Some(page);
    }

    /// How many times the host sent `0xA5` to ask for a page again.
    #[must_use]
    pub fn retries_sent(&self) -> usize {
        self.retries_sent
    }

    /// Send every `0xD6` page exactly as long as the range's remaining bytes,
    /// not zero-filled to the page length.
    ///
    /// The fake pads by default, which is what §4.4's serialiser does. This is
    /// the other reading a test needs: a DA that sends the last page short.
    pub fn without_page_padding(&mut self) {
        self.pad_pages = false;
    }

    /// Make every `clear_input` fail with [`Error::Io`], as a vanished USB
    /// device does.
    ///
    /// The loader drains the chip's last echo byte at the end of the image
    /// write. A caller that discards that error would report a successful load
    /// for a port that had already gone away, so a test needs a port that can
    /// be made to fail exactly there.
    pub fn failing_clear_input(&mut self) {
        self.clear_input_fails = true;
    }

    /// The last `0xD6` range as `(start, bytes the fake prepared)`.
    ///
    /// `bytes` exceeds the requested length when the fake padded a range that
    /// was not a whole number of pages.
    #[must_use]
    pub fn read_data_debug(&self) -> (u32, usize) {
        self.read_data_debug
    }

    /// How many `0xA8` frames arrived with a count other than zero.
    ///
    /// The jump frame's count word has no meaning in
    /// `docs/reference/01-da-protocol.md`; the only measured client sent zero.
    /// A fake that took any count would let a wrong one reach a unit unnoticed.
    #[must_use]
    pub fn bad_jump_counts(&self) -> usize {
        self.bad_jump_counts
    }

    /// Whether a `0xD5` arrived before any `0xD3` was accepted.
    ///
    /// That command erases and programs the sector at address zero, which is the
    /// destructive sequence `AGENTS.md` §2.2 exists for; the safety tests assert
    /// this stays `false`.
    #[must_use]
    pub fn wrote_an_empty_range(&self) -> bool {
        self.wrote_an_empty_range
    }

    /// Damage one byte of the flash window at a DA address, so a read-back
    /// disagrees.
    ///
    /// Call it after the `0xD5` conversation: it models a half-word that failed
    /// to program without teaching the fake a programming failure mode it has
    /// no evidence for. The address is the DA's, like every other address this
    /// fake records; use the range the write used.
    pub fn damage(&mut self, da_address: u32, mask: u8) {
        if let Some(slot) = self.memory.get_mut(da_address as usize) {
            *slot ^= mask;
        }
    }

    /// Let a test move the chip between the three things it can emulate.
    ///
    /// After `0xD9` a real unit runs its firmware; a read-back then needs the
    /// BootROM again, and this is where a test says so.
    ///
    /// What is cleared: both partial-frame buffers, the bytes waiting to be
    /// read, the sync probe count, and — in the DA mode — the packet stage.
    /// What deliberately **stays**: the flash window, `da_range`, `da_cursor`,
    /// `accepted_packets` and `restarted`. Those are the chip's own state, and a
    /// fake that forgot them on a mode change would hide a test that re-entered
    /// the DA and then wrote to a range it thought it had just recorded.
    pub fn set_modelled(&mut self, mode: Modelled) {
        self.line.clear();
        self.bootrom_line.clear();
        self.output.clear();
        self.bootrom_sync = 0;
        match mode {
            Modelled::Firmware => {
                self.mode = Mode::Firmware;
            }
            Modelled::BootRom => {
                self.mode = Mode::BootRom;
            }
            Modelled::DownloadAgent => {
                self.mode = Mode::DownloadAgent;
                self.da_stage = DaStage::Command;
                let report = self.da_info_report();
                self.queue_bytes(&report);
            }
        }
    }

    /// The flash window, in the DA's linear view.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether `0xD9` has been seen.
    #[must_use]
    pub fn restarted(&self) -> bool {
        self.restarted
    }

    /// The range the last accepted `0xD3` recorded.
    #[must_use]
    pub fn recorded_range(&self) -> (u32, u32) {
        self.da_range
    }

    // -- byte plumbing -----------------------------------------------------

    fn queue_bytes(&mut self, bytes: &[u8]) {
        self.output.extend(bytes.iter().copied());
    }

    fn take_output(&mut self, buf: &mut [u8]) -> usize {
        self.read_polls = self.read_polls.saturating_add(1);
        let mut written = 0usize;
        for slot in buf.iter_mut() {
            match self.output.pop_front() {
                Some(byte) => {
                    *slot = byte;
                    written += 1;
                }
                None => break,
            }
        }
        self.read_bytes = self.read_bytes.saturating_add(written);
        written
    }

    // -- the state machine -------------------------------------------------

    fn consume(&mut self, byte: u8) {
        // A byte that arrives while the fake is waiting for a `0xD6` advance is
        // that advance, not a new command: `0xD6` is a conversation that runs
        // one page at a time and `0x5A` / `0xA5` are the only bytes in it that
        // are not framed.
        if self.mode == Mode::DownloadAgent && !self.read_data.is_empty() {
            self.answer_page(byte);
            return;
        }
        match self.mode {
            // A unit that is running its firmware answers a BootROM frame with
            // silence: the bytes go nowhere.
            Mode::Firmware => {}
            Mode::BootRom => self.process_bootrom(byte),
            Mode::DownloadAgent => self.process_download_agent(byte),
        }
    }

    fn process_bootrom(&mut self, byte: u8) {
        // `0xA0` starts the four-probe sync; the chip answering it is what lets
        // a caller run `MtkBootRom::sync()` after `0xD9` has restarted it
        // (`docs/dev/03-hardware-facts.md` "Sync sequence"). Anything that is
        // not the first probe is a frame byte.
        if self.bootrom_line.is_empty() && byte == BOOT_SYNC[0] {
            self.bootrom_sync = 1;
            if let Some(reply) = BOOT_REPLY.first() {
                self.queue_bytes(&[*reply]);
            }
            return;
        }
        if self.bootrom_sync > 0 {
            let index = self.bootrom_sync;
            if BOOT_SYNC.get(index).copied() == Some(byte) {
                if let Some(reply) = BOOT_REPLY.get(index) {
                    self.queue_bytes(&[*reply]);
                }
                self.bootrom_sync = index + 1;
            }
            if self.bootrom_sync >= BOOT_SYNC.len() {
                self.bootrom_sync = 0;
            }
            return;
        }
        // A read is answered as echo + data, in that order, with **no**
        // checksum trailer: `docs/reference/01-da-protocol.md` §4.3 and
        // `docs/dev/03-hardware-facts.md` "Reads carry no trailer". The echo is
        // nine bytes then the payload, so a frame is echoed as it arrives and
        // the data follows the ninth byte.
        if self.bootrom_line.is_empty() {
            self.echo_frame = byte != OPCODE_JUMP;
        }
        // Every frame's opcode, so a test can assert the *set* of opcodes a
        // client sent — the check that the loader and the reader never reach a
        // destructive one. Recorded before the frame is consumed, because a
        // jump leaves the chip part-way through it.
        if self.bootrom_line.is_empty() {
            self.opcodes.push(byte);
        }
        if self.echo_frame {
            self.queue_bytes(&[byte]);
        }
        self.bootrom_line.push(byte);
        // `0xA1` is a header and then its payload: the count at bytes 5-8 is in
        // half-words, and the loader streams those after the header. Waiting for
        // only nine bytes would swallow the payload as the next frame.
        let needed = if self.bootrom_line.len() >= 9
            && self.bootrom_line.first().copied() == Some(OPCODE_WRITE16)
        {
            let units = self
                .bootrom_line
                .get(5..9)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map(u32::from_be_bytes)
                .and_then(|count| usize::try_from(count).ok())
                .unwrap_or(0);
            9usize.saturating_add(units.saturating_mul(2))
        } else {
            9
        };
        if self.bootrom_line.len() < needed {
            return;
        }
        let frame = std::mem::take(&mut self.bootrom_line);
        self.echo_frame = true;
        let Some(opcode) = frame.first().copied() else {
            return;
        };
        let be = |at: usize| {
            frame
                .get(at..at.saturating_add(4))
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map(u32::from_be_bytes)
                .unwrap_or(0)
        };
        let address = be(1);
        let count = be(5);
        let is_jump = opcode == OPCODE_JUMP && address == BASE_ADDR_DA;
        if is_jump {
            // The jump is the one frame the chip does not echo: it hands
            // control over part-way through and only part of the echo ever
            // arrives (`measured elsewhere`). The DA then answers with its
            // report, so a client that expects nine echo bytes before the
            // report desynchronises.
            //
            // The count word is counted, not obeyed: the real field has no
            // documented meaning (`docs/dev/03-hardware-facts.md` "DA jump"),
            // and the only client that ran it sent zero. A wrong count here is
            // what `bad_jump_counts` reports to a test.
            if count != DA_JUMP_COUNT {
                self.bad_jump_counts = self.bad_jump_counts.saturating_add(1);
            }
            self.started = true;
            self.mode = Mode::DownloadAgent;
            self.da_stage = DaStage::Command;
            self.line.clear();
            let report = self.da_info_report();
            self.queue_bytes(&report);
            return;
        }
        // A jump to anywhere else is not the loader's and the fake does not
        // model it: it takes nothing. Recording `started` for it would let a
        // client that jumped to the wrong address look correct.
        if opcode == OPCODE_JUMP {
            return;
        }
        match opcode {
            // Counts are in 16-bit units (`docs/dev/03-hardware-facts.md`
            // "Count units"); the wire order swaps each pair (§4.4).
            OPCODE_READ16 => {
                self.last_read_address = Some(address);
                let data = self.read_flash(address, count as usize * 2);
                let wire = swap_pairs(&data);
                self.queue_bytes(&wire);
            }
            // `0xA1` writes half-words, pair-swapped like `0xA2` reads, into the
            // DA's RAM. The loader is the only client: what it writes is the
            // image that the following `0xA8` is supposed to start.
            OPCODE_WRITE16 => {
                let payload = swap_pairs(frame.get(9..).unwrap_or_default());
                let offset = address.wrapping_sub(BASE_ADDR_DA) as usize;
                for (index, byte) in payload.iter().enumerate() {
                    if let Some(slot) = self.ram.get_mut(offset.saturating_add(index)) {
                        *slot = *byte;
                    }
                }
                self.bootrom_writes = self.bootrom_writes.saturating_add(1);
            }
            // `0xAF` counts dwords and reverses each 32-bit word.
            OPCODE_READ32 => {
                self.last_read_address = Some(address);
                let source = if address >= BASE_ADDR_DA {
                    self.read_ram(address, count as usize * 4)
                } else {
                    self.read_flash(address, count as usize * 4)
                };
                let mut wire = Vec::with_capacity(source.len());
                for word in source.chunks(4) {
                    wire.extend(word.iter().rev().copied());
                }
                self.queue_bytes(&wire);
            }
            _ => {}
        }
    }

    /// Copy `length` bytes of the flash window as the BootROM sees it.
    ///
    /// `m241_core` maps the same flash at [`BASE_ADDR_FIRMWARE`], the DA at
    /// zero; this window is the DA's, so a BootROM address is shifted back to
    /// its offset. ⚠️ That shift is the unresolved half of the mapping (see
    /// [`crate::da::FLASH_WRITE_START`]): the fake subtracts exactly as the
    /// client adds, so a test that only checks "the read matched" agrees with
    /// either convention. [`FakeDa::last_read_address`] is what a test compares
    /// with [`BASE_ADDR_FIRMWARE`] instead. Reads past the window are
    /// zero-filled: the chip answers with whatever the bus returns and the
    /// client's length checks catch a request that was too long.
    fn read_flash(&self, address: u32, length: usize) -> Vec<u8> {
        let offset = address.saturating_sub(BASE_ADDR_FIRMWARE);
        (0..length)
            .map(|index| {
                u32::try_from(index)
                    .ok()
                    .and_then(|index| offset.checked_add(index))
                    .and_then(|at| usize::try_from(at).ok())
                    .and_then(|at| self.memory.get(at).copied())
                    .unwrap_or(0)
            })
            .collect()
    }

    /// Copy `length` bytes of the DA's RAM, addressed from [`BASE_ADDR_DA`].
    ///
    /// A read below the DA's base or past the window is zero-filled, like
    /// [`Self::read_flash`]: the client's own length checks are what catch a
    /// request for an address that holds nothing.
    fn read_ram(&self, address: u32, length: usize) -> Vec<u8> {
        let offset = address.saturating_sub(BASE_ADDR_DA);
        (0..length)
            .map(|index| {
                u32::try_from(index)
                    .ok()
                    .and_then(|index| offset.checked_add(index))
                    .and_then(|at| usize::try_from(at).ok())
                    .and_then(|at| self.ram.get(at).copied())
                    .unwrap_or(0)
            })
            .collect()
    }

    // -- `0xD6`: read a range, one page at a time --------------------------

    /// `0xD6` setup, and the first page.
    ///
    /// `docs/reference/01-da-protocol.md` §5.5: three big-endian `u32` — page
    /// length, start, total length. There is no command acknowledgement; the
    /// first thing on the wire after the command is the first page. The range is
    /// read out of the **flash** window in the DA's own view (the same window
    /// `0xD3`/`0xD5` address), so a test can compare what the client got with
    /// what the fixture holds.
    fn begin_read_range(&mut self, frame: &[u8]) {
        let (Some(page_len), Some(start), Some(total)) =
            (be_u32(frame, 1), be_u32(frame, 5), be_u32(frame, 9))
        else {
            self.queue_bytes(&[NAK]);
            return;
        };
        let page_len = usize::try_from(page_len).unwrap_or(0);
        let total = usize::try_from(total).unwrap_or(0);
        if page_len == 0 || total == 0 {
            // A zero length is a division by zero in the DA's page count
            // (`0x00101E9C`); the fake refuses rather than modelling a crash.
            self.queue_bytes(&[NAK]);
            return;
        }
        self.read_page_len = page_len;
        self.read_start = start;
        self.read_page = 0;
        self.read_requested = Some((start, total));
        // The page conversation is not a `0xD5` packet transfer: leaving the
        // stage at `Packet` would parse the host's advance byte as a packet.
        self.da_stage = DaStage::Command;
        // The range is taken from the **flash window**, addressed from zero,
        // without the BootROM's `BASE_ADDR_FIRMWARE` shift: `0xD6` carries the
        // DA's own view, the same one `0xD3`/`0xD5` take, so a read at `0x100`
        // returns flash offset `0x100` and not zero. `read_flash` would shift
        // both candidates down by 2 MiB, and the probe's whole question is
        // which view holds the image.
        // Copied byte by byte from the DA's own address: `start` is a flash
        // offset, so `memory[start]` is the range's first byte. A flat `map`
        // over `memory[start..start + total]` would need a slice whose bounds
        // are checked up front; this keeps the same "zero past the end" rule the
        // rest of the fake uses.
        let mut copied: Vec<u8> = Vec::with_capacity(total);
        let mut i = 0usize;
        while i < total {
            let next = self.byte_at(start, i);
            copied.push(next);
            i += 1;
        }
        self.read_data = copied;
        // A range that is not a whole number of pages leaves bytes over. §4.4's
        // serialiser pads that last page to the page length, and the client
        // requires the whole page; a fake that sent a short one would make every
        // multi-page read time out. The tail is zero **only when padding was
        // asked for**; with [`Self::without_page_padding`] the last page is sent
        // as long as what is left of the range instead.
        let leftover = total % page_len;
        if leftover != 0 && self.pad_pages {
            self.read_data
                .resize(total.saturating_add(page_len - leftover), 0);
        }
        self.read_data_debug = (start, self.read_data.len());
        // The page count follows §5.5's `udiv(total + page - 1, page)`.
        self.read_pages = total.div_ceil(page_len);
        self.send_page();
    }

    /// The window byte at flash offset `start + index`, or `0` past the end.
    ///
    /// A named function rather than a closure inside `begin_read_range`: the
    /// conversion from an index to an address is the one place this file can
    /// silently lose a byte, so it exists once and every reader of the window
    /// (`0xD6`) goes through it.
    fn byte_at(&self, start: u32, index: usize) -> u8 {
        let at = u32::try_from(index).unwrap_or(u32::MAX).wrapping_add(start) as usize;
        self.memory.get(at).copied().unwrap_or(0)
    }

    /// Send the current page: `page_len` bytes, zero-padded, plus the sum.
    ///
    /// The trailer is the independent [`sum_of_bytes`], as for a `0xD5` packet:
    /// the client checks its own copy, and two implementations disagreeing is
    /// the point.
    ///
    /// Written without a slice of `read_data`: a range its own `get` calls
    /// cannot see still serves its first page, which is exactly the kind of
    /// silent miss a test should not have to debug. This indexes byte by byte,
    /// so a page is short only when the data really stops there.
    fn send_page(&mut self) {
        let first = self.read_page.saturating_mul(self.read_page_len);
        let remaining = self
            .read_data
            .len()
            .saturating_sub(first)
            .min(self.read_page_len);
        let len = if self.padding_on() {
            self.read_page_len
        } else {
            remaining
        };
        let mut page = Vec::with_capacity(len);
        for index in 0..len {
            page.push(
                first
                    .checked_add(index)
                    .and_then(|at| self.read_data.get(at).copied())
                    .unwrap_or(0),
            );
        }
        let mut packet = page.clone();
        let mut sum = sum_of_bytes(&page);
        if self.always_corrupt_page == Some(self.read_page) {
            sum = sum.wrapping_add(1);
        } else if self.corrupt_page == Some(self.read_page) {
            // Once only: a second corruption of the same page would make the
            // client's retry accounting unresolvable.
            sum = sum.wrapping_add(1);
            self.corrupt_page = None;
        }
        packet.extend_from_slice(&sum.to_be_bytes());
        self.queue_bytes(&packet);
    }

    /// Whether a page is zero-filled to the page length.
    fn padding_on(&self) -> bool {
        self.pad_pages
    }

    /// What the host sent after a page: advance, retry, or nothing is sent.
    ///
    /// §5.5: `0x5A` advances, `0xA5` sends the same page again, any other byte is
    /// ignored and the DA keeps waiting. After the last page the range is done:
    /// the host's advance draws **no reply**, and the fake is back to its
    /// command loop — anything the client sends next is a command, which is how
    /// an extra byte shows up as an opcode rather than being absorbed.
    fn answer_page(&mut self, byte: u8) {
        if byte == NAK {
            self.retries_sent = self.retries_sent.saturating_add(1);
            self.send_page();
            return;
        }
        if byte != ACK {
            return;
        }
        self.read_page = self.read_page.saturating_add(1);
        if self.read_page >= self.read_pages {
            self.read_data.clear();
            return;
        }
        self.send_page();
    }

    /// The twenty-byte download-agent report, `>BHBLHHHHL`.
    ///
    /// The layout and every field follow `docs/dev/03-hardware-facts.md`'s
    /// table: four middle `H` fields are `00 C2`, `22 BA`, `00 00`, `00 00` —
    /// manufacturer, device id and two zeros, not the device id twice. The
    /// trailing `L` comes from a RAM cell on a unit, so the fake may pick it;
    /// nothing compares it.
    fn da_info_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(20);
        report.push(0xC0); // report marker
        report.extend_from_slice(&0x0308u16.to_be_bytes()); // DA version 3.8
        report.push(0x49); // flash device id
        report.extend_from_slice(&FLASH_SIZE.to_be_bytes()); // 512 KiB
        report.extend_from_slice(&0x00C2u16.to_be_bytes()); // manufacturer
        report.extend_from_slice(&0x22BAu16.to_be_bytes()); // device code
        report.extend_from_slice(&0x0000u16.to_be_bytes());
        report.extend_from_slice(&0x0000u16.to_be_bytes());
        report.extend_from_slice(&0x0000_0000u32.to_be_bytes()); // no ext SRAM
        report
    }

    fn process_download_agent(&mut self, byte: u8) {
        self.line.push(byte);
        // A `0xD6` conversation's advance and retry bytes never reach this
        // function: `consume` routes them to `answer_page` first, so every byte
        // recorded here is a command the client actually issued.
        if self.line.len() == 1 {
            self.opcodes.push(byte);
        }
        match self.da_stage {
            DaStage::Command => self.dispatch_da_command(),
            DaStage::Packet => self.dispatch_da_packet(),
        }
    }

    fn dispatch_da_command(&mut self) {
        let Some(opcode) = self.line.first().copied() else {
            return;
        };
        let needed = match opcode {
            OPCODE_MEM_CMD => 10,
            OPCODE_READ_RANGE => 13,
            // `0xA1` is a header and then `count` half-words, so only the header
            // is known up front; the full length is resolved below.
            OPCODE_WRITE16 => 9,
            OPCODE_WRITE => 5,
            OPCODE_RESTART => 1,
            _ => 1,
        };
        if self.line.len() < needed {
            return;
        }
        if opcode == OPCODE_WRITE16 {
            // A `0xA1` frame is not complete until its payload has arrived: the
            // count at bytes 5–8 is in half-words. Acting on the header alone
            // would program the chunk from bytes that have not been sent.
            let payload = usize::try_from(be_u32(&self.line, 5).unwrap_or(0)).unwrap_or(0) * 2;
            if self.line.len() < 9 + payload {
                return;
            }
        }
        let frame = std::mem::take(&mut self.line);
        match opcode {
            OPCODE_MEM_CMD => {
                let (Some(start), Some(end)) = (be_u32(&frame, 2), be_u32(&frame, 6)) else {
                    self.queue_bytes(&[NAK]);
                    return;
                };
                // From the disassembly: `0xD3` records the range and touches no
                // flash at all -- the handler (`0x00101F98`-`0x00102034`) calls
                // no flash driver. The erase is in `0xD5`, immediately before
                // the data is programmed (`docs/reference/01-da-protocol.md`
                // §4.6).
                let Some(range) = accepted_da_range(start, end).filter(|_| !self.refuse_ranges)
                else {
                    // ⚠️ 未検証 from the disassembly, not a measurement: a
                    // refusal is a **single** `A5`; the success path's ACK and
                    // status byte are both skipped. A client that always reads
                    // two bytes consumes the first byte of the next reply, and
                    // one that always reads one leaves the `0x00` of a success
                    // behind.
                    //
                    // A refused command records nothing, so the ranges from an
                    // earlier `0xD3` stay in place.
                    self.queue_bytes(&[NAK]);
                    return;
                };
                self.da_range = range;
                self.da_cursor = start;
                // `measured elsewhere:` on success `0xD3` answers with **two**
                // bytes, ACK then status. A client that reads one leaves the
                // status byte in the buffer and shifts every later frame.
                self.queue_bytes(&[ACK, STATUS_OK]);
            }
            OPCODE_WRITE => {
                self.da_packet_len = be_u32(&frame, 1)
                    .and_then(|length| usize::try_from(length).ok())
                    .filter(|length| *length > 0)
                    .unwrap_or(WRITE_PACKET_LEN);
                // §5.6, in order: the first `0x5A` (`0x00102504`), the erase of
                // the sector holding the recorded start (`0x0010251C`–
                // `0x00102540`), then either `0xA5` (`0x0010253C`–`0x0010254C`)
                // or the second `0x5A` (`0x00102560`). Nothing is erased at
                // `0xD3` time, so the flash is intact until this point.
                //
                // With no `0xD3` accepted first the recorded range is still
                // `(0, 0)`, so a real DA would erase the sector at address
                // zero. The fake **records** that the client did it instead of
                // damaging the window: destroying the fixture would make the
                // failure land somewhere other than the assertion that is
                // about it, and `wrote_an_empty_range` is what the safety tests
                // assert stays false.
                let (start, _end) = self.da_range;
                self.queue_bytes(&[ACK]);
                self.erase_failed = false;
                if self.da_range == (0, 0) {
                    self.wrote_an_empty_range = true;
                } else if self.fail_first_erase {
                    // The DA restarts after this, so nothing else answers, and
                    // the bytes the host may still have in flight are not a
                    // packet: leaving the stage on `Packet` would parse them as
                    // one and answer again.
                    self.erase_failed = true;
                    self.da_stage = DaStage::Command;
                    self.queue_bytes(&[NAK]);
                    return;
                } else {
                    self.erase_sector(start);
                }
                self.da_cursor = start;
                // The packet count belongs to this command, not to the fake:
                // a `0xD5` after a completed one must start counting from zero
                // again, exactly as the cursor does. Left over, it makes the
                // next command's first packet look like its last.
                self.accepted_packets = 0;
                self.da_stage = DaStage::Packet;
                self.queue_bytes(&[ACK]);
            }
            OPCODE_RESTART => {
                // From the disassembly: no argument, no reply. The real chip
                // then boots its firmware; a read-back needs the BootROM again,
                // which a test asks for with `set_modelled`.
                self.restarted = true;
                self.mode = Mode::Firmware;
                self.line.clear();
            }
            OPCODE_READ_RANGE => {
                self.begin_read_range(&frame);
                // The command's own frame is finished; what follows is the
                // host's page-by-page conversation, which `consume` routes to
                // `answer_page` while `read_data` is non-empty. Clearing the
                // buffer matters: the last command byte was pushed into it just
                // before this arm ran.
                self.line.clear();
            }
            _ => self.queue_bytes(&[NAK]),
        }
    }

    fn dispatch_da_packet(&mut self) {
        if self.erase_failed {
            // After `0xA5` the DA restarted; whatever the host sends next is
            // not part of this command, and the fake must not answer it.
            self.line.clear();
            return;
        }
        let needed = self.da_packet_len.saturating_add(2);
        if self.line.len() < needed {
            return;
        }
        let frame = std::mem::take(&mut self.line);
        let Some((payload, trailer)) = frame.split_at_checked(self.da_packet_len) else {
            return;
        };
        if self.accepted_packets >= self.refuse_from
            && self.accepted_packets - self.refuse_from < self.refuse_count
        {
            self.queue_bytes(&[NAK]);
            return;
        }
        let Some(checksum) = trailer
            .get(..2)
            .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
            .map(u16::from_be_bytes)
        else {
            self.queue_bytes(&[NAK]);
            return;
        };
        // `docs/reference/01-da-protocol.md` §4.4: the trailer is the 16-bit
        // sum of the **payload bytes only**, high byte first. The sum is
        // computed here independently of `crate::da::byte_sum` so a change in
        // the client's arithmetic is a disagreement, not two sides agreeing on
        // the same mistake.
        if checksum != sum_of_bytes(payload) {
            self.queue_bytes(&[NAK]);
            return;
        }
        self.packets.push(payload.to_vec());
        self.trailers.push([
            trailer.first().copied().unwrap_or(0),
            trailer.get(1).copied().unwrap_or(0),
        ]);
        // `0x00101B0C`–`0x00101B34` (§5.6): the DA computes the range's packet
        // count as `ceil((end - start + 1) / length)` and acts on it. Once the
        // computed count has all arrived, the command is over — two `0x5A`.
        // Anything the host sends after that is not part of this command: more
        // data than the range needs is answered `0xA5` and the transfer ends.
        if self.accepted_packets >= self.expected_packets() {
            self.da_stage = DaStage::Command;
            self.queue_bytes(&[NAK]);
            return;
        }
        // The packet is programmed at the cursor. A packet can be shorter than
        // the command length only if the host sent fewer bytes, and then the
        // DA zero-fills it before programming (`0x001019C0`–`0x001019D8`,
        // §4.4); the cursor therefore advances by the command's length, not by
        // what the host happened to send. That is also why the packet count and
        // "the cursor passed `end`" are not the same rule: with a short final
        // packet the cursor overruns the range before the count is reached.
        // ⚠️ 未検証 for the *write* direction: §4.4 reads that padding off the
        // read serialiser, and `docs/dev/05-verification.md` §1 did not record
        // whether the final 92-byte packet's tail is padded.
        let offset = self.da_cursor as usize;
        let mut programmed = vec![0u8; self.da_packet_len];
        programmed
            .iter_mut()
            .zip(payload.iter())
            .for_each(|(slot, byte)| *slot = *byte);
        for (index, byte) in programmed.iter().enumerate() {
            if let Some(slot) = self.memory.get_mut(offset.saturating_add(index)) {
                *slot = *byte;
            }
        }
        let advance = u32::try_from(self.da_packet_len).unwrap_or(u32::MAX);
        self.da_cursor = self.da_cursor.saturating_add(advance);
        self.accepted_packets += 1;
        self.queue_bytes(&[CONT]);
        if self.accepted_packets == self.expected_packets() {
            self.da_stage = DaStage::Command;
            self.queue_bytes(&[ACK, ACK]);
        }
    }

    /// The packet count the DA computes for the recorded range.
    ///
    /// `ceil((end - start + 1) / da_packet_len)`, `0x00101B0C`–`0x00101B34`
    /// (`docs/reference/01-da-protocol.md` §5.6). At least one packet, as the
    /// DA's own arithmetic yields for a range that recorded anything.
    fn expected_packets(&self) -> usize {
        let (start, end) = self.da_range;
        let bytes = usize::try_from(end.saturating_sub(start)).unwrap_or(usize::MAX) + 1;
        bytes.div_ceil(self.da_packet_len.max(1)).max(1)
    }

    /// Erase the modelled sector that holds `address`, as `0xD5` does before it
    /// programs data there.
    ///
    /// The real erase is per sector, resolved from the DA's flash descriptor;
    /// the fake aligns to [`FLASH_SECTOR_SIZE`] because that layout is not
    /// decoded (`docs/reference/01-da-protocol.md` §6). The sector is clamped to
    /// the part, so the fake never touches a byte outside [`FLASH_SIZE`]. The
    /// protocol erases each sector as the write reaches it
    /// (`0x0010268C`–`0x001026BC`); the fake erases the sector the cursor is in
    /// before the packet that lands there, so no byte is ever programmed into a
    /// sector that was not erased first.
    fn erase_sector(&mut self, address: u32) {
        let sector = (address / FLASH_SECTOR_SIZE).min(FLASH_SIZE / FLASH_SECTOR_SIZE - 1);
        let base = sector.saturating_mul(FLASH_SECTOR_SIZE);
        for at in base..base.saturating_add(FLASH_SECTOR_SIZE) {
            if let Some(slot) = self.memory.get_mut(at as usize) {
                *slot = 0xFF;
            }
        }
    }
}

impl Transport for FakeDa {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.take_output(buf))
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        for byte in buf {
            self.consume(*byte);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    fn clear_input(&mut self) -> Result<()> {
        if self.clear_input_fails {
            return Err(m241_core::Error::Io {
                path: Some(String::from("SIM0")),
                source: std::io::Error::other("simulated port went away during the drain"),
            });
        }
        self.output.clear();
        Ok(())
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Ok(())
    }

    fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
        Ok(())
    }
}

/// The 16-bit sum of the payload bytes the fake checks a `0xD5` trailer against.
///
/// `docs/reference/01-da-protocol.md` §4.4: `FUN_001019F0` compares the two-byte
/// trailer that follows the payload with the accumulated byte sum.
///
/// **Independent of [`crate::da::byte_sum`] on purpose.** The client's function
/// produces the trailer; this one decides whether to accept it. Sharing one
/// definition would let a wrong arithmetic agree with itself and keep the fake
/// green. Two implementations plus a literal test on each
/// (`crate::da`'s `the_packet_trailer_is_the_sum_in_big_endian` and
/// `simulator.rs`'s `the_fake_checks_the_sum_independently_of_the_client`)
/// means a change on either side is a disagreement instead of a silent match.
fn sum_of_bytes(payload: &[u8]) -> u16 {
    payload
        .iter()
        .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)))
}

fn be_u32(frame: &[u8], at: usize) -> Option<u32> {
    frame
        .get(at..at.saturating_add(4))
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes)
}

/// The range the DA would record, or `None` when it would refuse the command.
///
/// The DA accepts a range only when the whole span lies inside mapped flash, its
/// `start` is even and its `end` is odd; the parity rule makes the range a whole
/// number of 16-bit half-words, which is what the flash driver programs. An
/// inverted range is refused as well: its length `end - start + 1` cannot be a
/// covered span (`docs/reference/01-da-protocol.md` §4.2 and §4.6).
fn accepted_da_range(start: u32, end: u32) -> Option<(u32, u32)> {
    let inside_flash = start < FLASH_SIZE && end < FLASH_SIZE;
    let half_word_aligned = start.is_multiple_of(2) && !end.is_multiple_of(2);
    if start > end || !inside_flash || !half_word_aligned {
        return None;
    }
    Some((start, end))
}

/// Drive the fake through the BootROM jump and drain the info report.
///
/// Returns the report so a test can assert on it; every DA test needs this
/// sequence first, and repeating it is how the two-byte reply gets miscounted.
///
/// The jump frame is built from [`BASE_ADDR_DA`], which is also what
/// [`FakeDa`] requires for the jump, so the two cannot drift apart.
pub fn enter_download_agent(device: &mut FakeDa) -> Vec<u8> {
    let jump = Frame {
        opcode: OPCODE_JUMP,
        address: BASE_ADDR_DA,
        count: 0,
    };
    if device.write_all(&jump.to_bytes()).is_err() {
        return Vec::new();
    }
    let mut report = vec![0u8; 20];
    match device.read(&mut report) {
        Ok(read) => report.truncate(read),
        Err(_) => report.clear(),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chip already past the BootROM jump, with `memory` as its flash window.
    fn download_agent(memory: &[u8]) -> FakeDa {
        let mut device = FakeDa::with_firmware(memory);
        let report = enter_download_agent(&mut device);
        assert_eq!(report.first().copied(), Some(0xC0), "the DA banner");
        device
    }

    /// A `0xD3` frame with one range: `start`..=`end`, both addresses.
    fn memory_range(start: u32, end: u32) -> Vec<u8> {
        let mut frame = vec![OPCODE_MEM_CMD, 0x01];
        frame.extend_from_slice(&start.to_be_bytes());
        frame.extend_from_slice(&end.to_be_bytes());
        frame
    }

    /// Read the whole queued reply, up to `limit` bytes and stopping at one
    /// short read: a refusal is one byte while a success is two, and the
    /// difference is the point.
    fn read_reply(device: &mut FakeDa, limit: usize) -> Vec<u8> {
        let mut buf = vec![0u8; limit];
        let mut filled = 0usize;
        while filled < limit {
            let read = device.read(&mut buf[filled..]).expect("read the reply");
            if read == 0 {
                break;
            }
            filled += read;
        }
        buf.truncate(filled);
        buf
    }

    /// Send one `0xD5` packet: the payload plus its big-endian 16-bit sum.
    fn write_packet(device: &mut FakeDa, payload: &[u8]) {
        let sum = payload
            .iter()
            .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)));
        let mut packet = payload.to_vec();
        packet.extend_from_slice(&sum.to_be_bytes());
        device.write_all(&packet).expect("write the packet");
    }

    /// Send a `0xD5` command for packets of `length` bytes.
    fn write_command(device: &mut FakeDa, length: u32) {
        let mut command = vec![OPCODE_WRITE];
        command.extend_from_slice(&length.to_be_bytes());
        device.write_all(&command).expect("write command");
    }

    #[test]
    fn the_da_handshake_reports_twenty_bytes_starting_with_c0() {
        // The measured fact that matters most here: the report starts with
        // 0xC0 and the chip has left the BootROM for the DA.
        let mut device = FakeDa::with_firmware(&[0u8; 16]);
        let report = enter_download_agent(&mut device);
        assert_eq!(report.first().copied(), Some(0xC0));
        assert_eq!(report.len(), 20);
        assert_eq!(device.mode(), Mode::DownloadAgent);
    }

    #[test]
    fn the_memory_range_command_answers_with_ack_and_status() {
        // Two bytes, not one: reading only the ACK shifts every later frame by a
        // byte, which is the trap the hardware notes record.
        let mut device = download_agent(&[0xFFu8; 32]);
        device
            .write_all(&memory_range(0, 31))
            .expect("memory range command");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        assert_eq!(device.recorded_range(), (0, 31));
    }

    #[test]
    fn recording_a_memory_range_leaves_the_flash_untouched() {
        // `0xD3`'s handler calls no flash driver: it records the range and
        // resets the iterators, and that is all. A fake that erased here would
        // hide the difference between "recorded and never written" and
        // "erased and written", which is exactly the distinction the flash
        // tooling has to get right.
        let untouched = vec![0x5Au8; 2 * FLASH_SECTOR_SIZE as usize];
        let mut device = download_agent(&untouched);
        device
            .write_all(&memory_range(0, FLASH_SECTOR_SIZE - 1))
            .expect("record the whole first sector");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        assert_eq!(
            device.memory(),
            untouched.as_slice(),
            "0xD3 records the range; it must not erase it"
        );
    }

    #[test]
    fn the_write_command_erases_the_sectors_the_recorded_range_covers_before_it_programs_them() {
        // The erase is on the `0xD5` path (`0x0010251C`-`0x00102560`), and it is
        // sector-granular: a range of four bytes still costs the whole sector it
        // sits in. Checking the flash between the command's acknowledgements and
        // the first packet is what proves the order: erased first, programmed
        // after.
        let fill = 0xAAu8;
        let mut device = download_agent(&vec![fill; 2 * FLASH_SECTOR_SIZE as usize]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);

        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);

        let sector = FLASH_SECTOR_SIZE as usize;
        assert!(
            device.memory()[..sector].iter().all(|byte| *byte == 0xFF),
            "the sector the range touches must be erased before any packet"
        );
        assert!(
            device.memory()[sector..].iter().all(|byte| *byte == fill),
            "a sector outside the range must not be erased"
        );

        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        write_packet(&mut device, &payload);
        assert_eq!(read_reply(&mut device, 3).first().copied(), Some(CONT));
        assert_eq!(&device.memory()[..payload.len()], &payload);
    }

    #[test]
    fn a_refused_memory_range_is_answered_with_one_nak_byte_and_nothing_else() {
        // Success is `5A 00`; a refusal is a single `A5`, because the failure
        // path skips both sends of the success path. A client that always reads
        // two bytes consumes the first byte of the following reply.
        let mut device = download_agent(&[0xFFu8; 16]);
        let refused = [
            // start must be even
            (1u32, 3u32),
            // end must be odd
            (0, 4),
            // start must not be greater than end
            (2, 1),
            // the whole span must lie inside mapped flash
            (FLASH_SIZE, FLASH_SIZE + 1),
        ];
        for (start, end) in refused {
            device
                .write_all(&memory_range(start, end))
                .expect("refused range");
            assert_eq!(
                read_reply(&mut device, 4),
                [NAK],
                "0xD3 for {start:#010X}..={end:#010X} must answer one byte"
            );
        }
    }

    #[test]
    fn a_refused_memory_range_leaves_nothing_behind_for_the_next_reply() {
        // A refused `0xD3` is one byte on the wire. The fixture may need an
        // empty poll first (output is produced while the command's last byte is
        // consumed), so what is invariant is *how much left*: one byte, and an
        // empty queue behind it.
        let mut device = download_agent(&[0xFFu8; 16]);
        let bytes_before = device.read_bytes();
        // `end` must be odd, so this range is refused.
        device
            .write_all(&memory_range(0x0000_4000, 0x0000_40FE))
            .expect("range");
        assert_eq!(device.output_len(), 1, "the refusal is exactly one byte");
        assert_eq!(read_reply(&mut device, 4), [NAK]);
        assert_eq!(
            device.read_bytes() - bytes_before,
            1,
            "one byte left the wire for a refused 0xD3"
        );
        assert_eq!(device.output_len(), 0, "and nothing followed it");

        // A success leaves two, so "one" above is the refusal's own property and
        // not the fixture having gone quiet.
        let bytes_before = device.read_bytes();
        device
            .write_all(&memory_range(0x0000_4000, 0x0000_40FF))
            .expect("range");
        assert_eq!(read_reply(&mut device, 4), [ACK, STATUS_OK]);
        assert_eq!(device.read_bytes() - bytes_before, 2);
    }

    #[test]
    fn a_refused_memory_range_keeps_the_range_recorded_before_it() {
        // The DA returns before it overwrites the range table, so `A5` does not
        // mean "nothing is recorded". The refused range below lies in a *later*
        // sector than the accepted one, so a fake that recorded it anyway would
        // erase the wrong sector and leave the right one intact -- visible here
        // as a write into flash that was never erased.
        let fill = 0xAAu8;
        let accepted = 0x4000u32; // sector 1
        let refused = 0x8000u32; // sector 2
        let mut device = download_agent(&vec![fill; 3 * FLASH_SECTOR_SIZE as usize]);
        device
            .write_all(&memory_range(accepted, accepted + 0xFF))
            .expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);

        // Refused: `end` is even. The accepted range must survive it.
        device
            .write_all(&memory_range(refused, refused))
            .expect("range");
        assert_eq!(read_reply(&mut device, 2), [NAK]);

        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);

        let payload: Vec<u8> = (0..0x100u32).map(|value| value as u8).collect();
        write_packet(&mut device, &payload);
        assert_eq!(read_reply(&mut device, 3).first().copied(), Some(CONT));

        let start = accepted as usize;
        let end = start + payload.len();
        let sector = FLASH_SECTOR_SIZE as usize;
        assert_eq!(
            &device.memory()[start..end],
            payload.as_slice(),
            "the packet must land in the range the accepted 0xD3 recorded"
        );
        assert!(
            device.memory()[sector..start]
                .iter()
                .all(|byte| *byte == 0xFF)
                && device.memory()[end..2 * sector]
                    .iter()
                    .all(|byte| *byte == 0xFF),
            "the accepted range's sector must be erased: the refused 0xD3 did not replace it"
        );
        assert!(
            device.memory()[2 * sector..]
                .iter()
                .all(|byte| *byte == fill),
            "the sector the refused range named must not have been erased"
        );
    }

    #[test]
    fn a_write_packet_is_acknowledged_and_stored() {
        let mut device = download_agent(&[0xFFu8; 16]);

        // Range 0..=3, then a four-byte packet.
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);

        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);

        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        write_packet(&mut device, &payload);
        assert_eq!(read_reply(&mut device, 3).first().copied(), Some(CONT));
        assert_eq!(&device.memory()[..4], &payload);
    }

    #[test]
    fn a_rejected_packet_draws_a_nak() {
        let mut device = FakeDa::with_firmware(&[0xFFu8; 16]).refusing_packets_from(0, 1);
        enter_download_agent(&mut device);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        // A correct packet: the rejection must come from
        // `refusing_packets_from`, not from a checksum error, or the test would
        // pass for the wrong reason.
        write_packet(&mut device, &[1u8, 2, 3, 4]);
        assert_eq!(read_reply(&mut device, 1).first().copied(), Some(NAK));
    }

    #[test]
    fn the_restart_command_sends_nothing() {
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&[OPCODE_RESTART]).expect("restart");
        assert!(device.restarted(), "0xD9 must be recorded");
        assert_eq!(
            read_reply(&mut device, 1),
            Vec::new(),
            "0xD9 never replies; waiting for one hangs"
        );
        // It boots the firmware, not the BootROM: a read-back has to re-enter
        // the BootROM first, and the fake says so rather than pretending.
        assert_eq!(device.mode(), Mode::Firmware);
    }

    #[test]
    fn a_bootrom_read_after_a_restart_returns_the_written_bytes() {
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(read_reply(&mut device, 3).first().copied(), Some(CONT));

        device.write_all(&[OPCODE_RESTART]).expect("restart");
        device.set_modelled(Modelled::BootRom);
        let mut rom = m241_core::protocol::brom::MtkBootRom::new(device);
        rom.sync().expect("sync");
        let read = rom.read16(BASE_ADDR_FIRMWARE, 4).expect("read back");
        assert_eq!(read, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn a_running_firmware_answers_no_bootrom_frame() {
        let mut device = FakeDa::with_firmware(&[0x18u8; 16]);
        device.set_modelled(Modelled::Firmware);
        let frame = [
            OPCODE_READ16,
            0x00,
            0x20,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x02,
        ];
        device.write_all(&frame).expect("write");
        assert_eq!(
            read_reply(&mut device, 8),
            Vec::new(),
            "the firmware does not answer BootROM frames"
        );
    }

    #[test]
    fn the_fake_checks_the_sum_independently_of_the_client() {
        // The fake's threshold arithmetic, pinned by literals rather than by
        // calling the client's `da::byte_sum`: sharing one definition would let
        // a mistake on both sides agree.
        assert_eq!(sum_of_bytes(&[0x01, 0x02, 0x03]), 6);
        assert_eq!(sum_of_bytes(&[]), 0);
        assert_eq!(sum_of_bytes(&[0xFF, 0xFF, 0x02]), 0x0200);
        assert_eq!(sum_of_bytes(&[0xFF; 0x100]), 0xFF00);
        assert_eq!(sum_of_bytes(&[0xFF, 0xFF]), 0x01FE);
    }

    #[test]
    fn a_packet_with_the_wrong_sum_is_refused() {
        // The independent sum is only meaningful if a wrong trailer draws `A5`
        // — otherwise the comparison could be bypassed and nothing would say so.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        let mut packet = vec![0xDE, 0xAD, 0xBE, 0xEF];
        packet.extend_from_slice(&0x0000u16.to_be_bytes());
        device.write_all(&packet).expect("packet");
        assert_eq!(read_reply(&mut device, 1), [NAK]);
        assert_eq!(device.packets(), &[] as &[Vec<u8>]);
    }

    #[test]
    fn one_more_packet_than_the_range_needs_is_refused() {
        // `0x00101B0C`-`0x00101B34`: the DA's packet count is
        // `ceil((end - start + 1) / length)`, so a range of four bytes with a
        // four-byte packet is one packet. A second packet has nowhere to go and
        // is answered `0xA5`, not `0x69`.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);

        write_packet(&mut device, &[1, 2, 3, 4]);
        assert_eq!(read_reply(&mut device, 3), [CONT, ACK, ACK]);

        write_packet(&mut device, &[5, 6, 7, 8]);
        assert_eq!(
            read_reply(&mut device, 1),
            [NAK],
            "a packet past the computed count must be refused"
        );
        assert_eq!(device.memory()[0], 1, "the extra packet was not programmed");
    }

    #[test]
    fn the_packet_count_is_ceil_of_the_range_over_the_length() {
        // Four bytes with two-byte packets: exactly two packets. The fake must
        // announce completion after the second, not after a cursor passes the
        // end, and it must refuse a third.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 2);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[1u8, 2]);
        assert_eq!(read_reply(&mut device, 1), [CONT]);
        write_packet(&mut device, &[3, 4]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT, ACK, ACK],
            "the second packet completes a four-byte range"
        );
        assert_eq!(&device.memory()[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn three_packets_complete_a_six_byte_range() {
        // Six bytes at two bytes per packet is exactly three packets. The fake
        // must not complete after two, and must not answer a fourth.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 5)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 2);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[1u8, 2]);
        assert_eq!(read_reply(&mut device, 1), [CONT]);
        write_packet(&mut device, &[3, 4]);
        assert_eq!(read_reply(&mut device, 1), [CONT]);
        write_packet(&mut device, &[5, 6]);
        assert_eq!(read_reply(&mut device, 3), [CONT, ACK, ACK]);
        assert_eq!(&device.memory()[..6], &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn one_byte_range_with_a_full_size_packet_is_one_packet_not_a_cursor_walk() {
        // The sharpest case where the DA's `ceil((end - start + 1) / length)` and
        // "the cursor passed `end`" are different questions: a *short* range with
        // a long packet is `ceil` = 1, while one `0x100`-byte packet lifts the
        // cursor far past `end`. The DA ends the command on the count, so the fake
        // must finish after exactly one packet and refuse a second — it must not
        // work out "the cursor is somewhere past the end, so keep going" or "the
        // range is short, so take more packets".
        //
        // `(0, 0)` cannot be used: the fake reads it as "nothing recorded" and
        // refuses the `0xD5`, which is the safety property its own test pins.
        let mut device = download_agent(&vec![0xFFu8; 0x200]);
        // Two bytes: `end` odd, span 2, `ceil(2 / 0x100)` = 1.
        device.write_all(&memory_range(0, 1)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);

        write_packet(&mut device, &[0x44; 0x100]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT, ACK, ACK],
            "a range shorter than one packet is one packet"
        );
        assert_eq!(device.accepted_packets(), 1);
        assert_eq!(device.memory()[0], 0x44);

        write_packet(&mut device, &[0x55; 0x100]);
        let after = read_reply(&mut device, 0x100 + 2);
        assert!(
            after.iter().all(|byte| *byte == NAK),
            "a packet past the count is refused, never acknowledged: {after:02X?}"
        );
        assert!(
            !after.contains(&CONT) && !after.contains(&ACK),
            "no 0x69 and no 0x5A may answer it: {after:02X?}"
        );
        assert_eq!(device.accepted_packets(), 1);
        assert_ne!(
            device.memory()[0x100],
            0x55,
            "the extra packet was not programmed"
        );
    }

    #[test]
    fn a_packet_beyond_a_short_range_is_refused_before_it_is_programmed() {
        // A two-byte range at `0x100` per packet is `ceil` = 1: the command is
        // over after the first packet, and the packet stage has ended. The
        // fixture's pre-packet guard is what answers the stray packet with a
        // single `A5` *and keeps it out of flash*; without it the bytes would be
        // parsed as unknown commands, drawing one `A5` per byte and possibly
        // reaching the next write's range.
        let mut device = download_agent(&vec![0xFFu8; 0x200]);
        device.write_all(&memory_range(0, 1)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[0x44; 0x100]);
        assert_eq!(read_reply(&mut device, 3), [CONT, ACK, ACK]);
        assert_eq!(
            device.mode(),
            Mode::DownloadAgent,
            "the transfer ends, it does not restart the chip"
        );

        write_packet(&mut device, &[0x99; 0x100]);
        let after = read_reply(&mut device, 0x102);
        assert!(
            after.iter().all(|byte| *byte == NAK),
            "the stray packet is refused: {after:02X?}"
        );
        assert_eq!(
            device.accepted_packets(),
            1,
            "the stray packet must not count as programmed"
        );
        assert!(
            device.memory()[0x100..0x200]
                .iter()
                .all(|byte| *byte == 0xFF),
            "nothing from the stray packet may reach flash"
        );
    }

    #[test]
    fn a_packet_past_the_two_packet_count_is_refused() {
        // The count rules that must hold once a range needs more than one
        // packet: exactly two complete it, and a third is refused. This is the
        // "extra packet" case the one-packet tests cannot reach, because there
        // the count and the cursor agree after the first.
        let mut device = download_agent(&vec![0xFFu8; 0x400]);
        device.write_all(&memory_range(0, 0x1FF)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[0x11; 0x100]);
        assert_eq!(read_reply(&mut device, 2), [CONT], "one packet of two");
        write_packet(&mut device, &[0x22; 0x100]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT, ACK, ACK],
            "the second packet completes a two-packet range"
        );
        write_packet(&mut device, &[0x33; 0x100]);
        let after = read_reply(&mut device, 0x100 + 2);
        assert!(
            after.iter().all(|byte| *byte == NAK),
            "a third packet is one too many and is refused, not acknowledged: {after:02X?}"
        );
        assert!(
            !after.contains(&CONT) && !after.contains(&ACK),
            "{after:02X?}"
        );
        assert_eq!(device.accepted_packets(), 2);
        assert_eq!(&device.memory()[..0x100], &[0x11u8; 0x100]);
        assert_eq!(&device.memory()[0x100..0x200], &[0x22u8; 0x100]);
        assert_ne!(device.memory()[0x200], 0x33);
    }

    #[test]
    fn a_second_write_command_still_counts_its_own_packets() {
        // The cursor is reset by each `0xD5`, so "the cursor passed `end`" can
        // say a command is finished when it has not started; the accepted count
        // is not reset, so the DA's own rule does not have that blind spot. With
        // the cursor rule, this second command would answer its *first* packet
        // as if it were the last: `5A 5A` straight after one `0x69`, and a real
        // unit would still be waiting for the rest of the range.
        let mut device = download_agent(&vec![0xFFu8; 0x400]);
        device.write_all(&memory_range(0, 0x1FF)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[0x11; 0x100]);
        assert_eq!(read_reply(&mut device, 2), [CONT]);
        write_packet(&mut device, &[0x22; 0x100]);
        assert_eq!(read_reply(&mut device, 3), [CONT, ACK, ACK]);

        // The same range and length again. The cursor is back at 0.
        write_command(&mut device, 0x100);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[0x33; 0x100]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT],
            "the second command's first packet is not its last: exactly one 0x69, no 0x5A"
        );
        write_packet(&mut device, &[0x44; 0x100]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT, ACK, ACK],
            "and its second packet completes it"
        );
        assert_eq!(
            device.accepted_packets(),
            2,
            "the second command counted its own two packets"
        );
        assert_eq!(&device.memory()[..0x100], &[0x33u8; 0x100]);
        assert_eq!(&device.memory()[0x100..0x200], &[0x44u8; 0x100]);
    }

    #[test]
    fn the_packet_count_is_the_range_and_the_final_packet_zfills_the_rest() {
        // Range `0..=9` (10 bytes) at 4 bytes per packet: `ceil(10/4)` = 3
        // packets, and the last carries 2 bytes that the DA zero-fills to 4.
        // A packet is always read as `length + 2` bytes, so the client cannot
        // send a frame shorter than the command length — the fill happens
        // inside the DA, which is why the packet count, not the cursor, is the
        // rule that ends the command.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 9)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        write_command(&mut device, 4);
        assert_eq!(read_reply(&mut device, 2), [ACK, ACK]);
        write_packet(&mut device, &[1, 2, 3, 4]);
        assert_eq!(read_reply(&mut device, 1), [CONT]);
        write_packet(&mut device, &[5, 6, 7, 8]);
        assert_eq!(read_reply(&mut device, 1), [CONT]);
        write_packet(&mut device, &[9, 10, 0, 0]);
        assert_eq!(
            read_reply(&mut device, 3),
            [CONT, ACK, ACK],
            "the third packet completes the range by count"
        );
        assert_eq!(
            &device.memory()[..12],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 0]
        );
    }

    #[test]
    fn a_failed_first_erase_answers_one_nak_and_ends_the_command() {
        // §5.6: `erase failed` at `0x0010253C`-`0x0010254C` answers `0xA5`
        // instead of the second `0x5A`, and the DA restarts.
        let mut device = download_agent(&[0xFFu8; 16]);
        device.write_all(&memory_range(0, 3)).expect("range");
        assert_eq!(read_reply(&mut device, 2), [ACK, STATUS_OK]);
        device.set_failing_first_erase(true);
        write_command(&mut device, 4);
        assert_eq!(
            read_reply(&mut device, 4),
            [ACK, NAK],
            "the first 0x5A, then 0xA5 in place of the second"
        );
        // A packet after the failure must not be acknowledged. The fake is back
        // in its command stage, where those bytes are unknown commands and draw
        // `A5` — what matters is that no `0x69` and no second `0x5A` appear.
        write_packet(&mut device, &[1, 2, 3, 4]);
        let after = read_reply(&mut device, 8);
        assert!(
            !after.contains(&CONT) && !after.contains(&ACK),
            "a packet after a failed erase must not be accepted: {after:02X?}"
        );
        assert_eq!(device.accepted_packets(), 0);
    }
    #[test]
    fn the_info_report_is_the_measured_twenty_bytes() {
        // `docs/dev/03-hardware-facts.md` records the report
        // `C0 03 08 49 00 08 00 00 00 C2 22 BA 00 00 00 00 …`: the device id
        // appears once, and the two `H` fields after it are zeros. The trailing
        // `L` comes from RAM on a unit, so only the first 16 bytes are pinned.
        let mut device = FakeDa::with_firmware(&[0xFFu8; 16]);
        let report = enter_download_agent(&mut device);
        assert_eq!(
            report.get(..16),
            Some(
                [
                    0xC0, 0x03, 0x08, 0x49, 0x00, 0x08, 0x00, 0x00, 0x00, 0xC2, 0x22, 0xBA, 0x00,
                    0x00, 0x00, 0x00
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn the_jump_is_built_from_the_da_load_address() {
        // The fake accepts a jump only at `BASE_ADDR_DA`; the helper must send
        // exactly that, not a literal that happens to match it today.
        let jump = Frame {
            opcode: OPCODE_JUMP,
            address: BASE_ADDR_DA,
            count: 0,
        };
        assert_eq!(
            &jump.to_bytes()[1..5],
            &BASE_ADDR_DA.to_be_bytes(),
            "the jump's address field is the load address"
        );
        let mut other = FakeDa::with_firmware(&[0xFFu8; 16]);
        let wrong = Frame {
            address: BASE_ADDR_DA + 4,
            ..jump
        };
        other.write_all(&wrong.to_bytes()).expect("write");
        assert_eq!(other.mode(), Mode::BootRom, "a wrong address is no jump");
    }
}
