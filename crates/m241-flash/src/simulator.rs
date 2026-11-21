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
};
use m241_core::protocol::frame::swap_pairs;
use m241_core::protocol::port::Transport;

use crate::da::{ACK, CONT, NAK, OPCODE_MEM_CMD, OPCODE_RESTART, OPCODE_WRITE};

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
            restarted: false,
            refuse_ranges: false,
            fail_first_erase: false,
            erase_failed: false,
            wrote_an_empty_range: false,
            last_read_address: None,
            echo_frame: true,
            bootrom_sync: 0,
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
        written
    }

    // -- the state machine -------------------------------------------------

    fn consume(&mut self, byte: u8) {
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
        if self.echo_frame {
            self.queue_bytes(&[byte]);
        }
        self.bootrom_line.push(byte);
        if self.bootrom_line.len() < 9 {
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
            self.mode = Mode::DownloadAgent;
            self.da_stage = DaStage::Command;
            let report = self.da_info_report();
            self.queue_bytes(&report);
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
            // `0xAF` counts dwords and reverses each 32-bit word.
            OPCODE_READ32 => {
                self.last_read_address = Some(address);
                let data = self.read_flash(address, count as usize * 4);
                let mut wire = Vec::with_capacity(data.len());
                for word in data.chunks(4) {
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
            OPCODE_WRITE => 5,
            OPCODE_RESTART => 1,
            _ => 1,
        };
        if self.line.len() < needed {
            return;
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
