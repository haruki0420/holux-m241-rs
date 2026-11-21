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
//! ([`FakeDa::rejecting_packets`]).

use std::collections::VecDeque;

use m241_core::Result;
use m241_core::protocol::brom::{
    BASE_ADDR_DA, BASE_ADDR_FIRMWARE, BOOT_REPLY, BOOT_SYNC, OPCODE_READ16, OPCODE_READ32,
};
use m241_core::protocol::frame::swap_pairs;
use m241_core::protocol::port::Transport;

use crate::da::{OPCODE_MEM_CMD, OPCODE_RESTART, OPCODE_WRITE};

/// Status byte the fake DA returns after a successful `0xD3`.
pub const STATUS_OK: u8 = 0x00;

/// Acknowledge byte used by the download agent.
pub const ACK: u8 = 0x5A;

/// Negative acknowledge byte.
pub const NAK: u8 = 0xA5;

/// "Continue": the DA accepted one packet and wants the next.
pub const CONT: u8 = 0x69;

/// Opcode that hands the chip from the BootROM to the DA.
pub const OPCODE_JUMP: u8 = 0xA8;

/// Payload bytes per DA packet, as the vendor tool sends them.
pub const WRITE_PACKET_LEN: usize = 0x100;

/// Bytes of flash the fake DA reports, and the span a `0xD3` range may cover.
///
/// `Measured:` the retail unit's 20-byte report carries `0x0008_0000` in its
/// total-bytes field; the DA's own linear view of the part is therefore
/// `0x0000_0000`–`0x0007_FFFF` (`docs/reference/01-da-protocol.md` §4.2).
pub const FLASH_SIZE: u32 = 0x0008_0000;

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
pub const FLASH_SECTOR_SIZE: u32 = 0x4000;

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
    /// Force the next `count` packets to be refused.
    ///
    /// Kept for the existing tests, which arm a refusal at packet zero.
    reject_packets: u32,
    /// The first packet index [`FakeDa::refusing_packets_from`] refuses.
    refuse_from: usize,
    /// How many packets from `refuse_from` on are refused.
    refuse_count: usize,
    /// How many packets this session accepted, which a test compares with the
    /// client's own `0x69` count.
    accepted_packets: usize,
    /// Whether the DA has seen `0xD9`.
    restarted: bool,
    /// Whether every `0xD3` is refused, as an unmappable range would be.
    refuse_ranges: bool,
    /// Whether a `0xD5` arrived while the recorded range was still `(0, 0)`.
    ///
    /// That is the command that erases the reset vector; recording it lets a
    /// test assert the client never sent one.
    wrote_an_empty_range: bool,
    /// Whether the BootROM frame currently being assembled is echoed.
    ///
    /// Reads are echoed; `0xA8` is not, because it hands control over
    /// part-way through (measured).
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
            reject_packets: 0,
            refuse_from: usize::MAX,
            refuse_count: 0,
            accepted_packets: 0,
            restarted: false,
            refuse_ranges: false,
            wrote_an_empty_range: false,
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

    /// Arm the same refusal on an existing fake, as tests that already own one
    /// need.
    pub fn set_refusing_packets_from(&mut self, skip: usize, count: usize) {
        self.refuse_from = skip;
        self.refuse_count = count;
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

    /// How many packets the fake accepted and programmed.
    #[must_use]
    pub fn accepted_packets(&self) -> usize {
        self.accepted_packets
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

    /// Damage one byte of the flash window, so a read-back disagrees.
    ///
    /// Call it after the `0xD5` conversation: it models a half-word that failed
    /// to program without teaching the fake a programming failure mode it has
    /// no evidence for.
    pub fn damage(&mut self, offset: usize, mask: u8) {
        if let Some(slot) = self.memory.get_mut(offset) {
            *slot ^= mask;
        }
    }

    /// Let a test move the chip between the three things it can emulate.
    ///
    /// After `0xD9` a real unit runs its firmware; a read-back then needs the
    /// BootROM again, and this is where a test says so.
    pub fn set_modelled(&mut self, mode: Modelled) {
        self.line.clear();
        self.output.clear();
        match mode {
            Modelled::Firmware => {
                self.mode = Mode::Firmware;
            }
            Modelled::BootRom => {
                self.mode = Mode::BootRom;
                self.bootrom_line.clear();
            }
            Modelled::DownloadAgent => {
                self.mode = Mode::DownloadAgent;
                self.da_stage = DaStage::Command;
                self.queue_bytes(&self.da_info_report());
            }
        }
    }

    /// The flash window, in the DA's linear view.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    /// The flash window, mutably, for a simulated programming failure.
    pub fn memory_mut(&mut self) -> &mut [u8] {
        &mut self.memory
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
            // arrives (measured). The DA then answers with its report, so a
            // client that expects nine echo bytes before the report
            // desynchronises.
            self.mode = Mode::DownloadAgent;
            self.da_stage = DaStage::Command;
            self.queue_bytes(&self.da_info_report());
            return;
        }
        match opcode {
            // Counts are in 16-bit units (`docs/dev/03-hardware-facts.md`
            // "Count units"); the wire order swaps each pair (§4.4).
            OPCODE_READ16 => {
                let data = self.read_flash(address, count as usize * 2);
                self.queue_bytes(&swap_pairs(&data));
            }
            // `0xAF` counts dwords and reverses each 32-bit word.
            OPCODE_READ32 => {
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
    /// its offset. Reads past the window are zero-filled: the chip answers with
    /// whatever the bus returns and the client's length checks catch a request
    /// that was too long.
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
    fn da_info_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(20);
        report.push(0xC0); // report marker
        report.extend_from_slice(&0x0308u16.to_be_bytes()); // DA version 3.8
        report.push(0x49); // flash device id
        report.extend_from_slice(&FLASH_SIZE.to_be_bytes()); // 512 KiB
        report.extend_from_slice(&0x00C2u16.to_be_bytes()); // manufacturer
        report.extend_from_slice(&0x22BAu16.to_be_bytes()); // device code
        report.extend_from_slice(&0x22u16.to_be_bytes());
        report.extend_from_slice(&0xBAu16.to_be_bytes());
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
                // Measured: `0xD3` records the range and touches no flash at
                // all -- the handler (`0x00101F98`-`0x00102034`) calls no flash
                // driver. The erase is in `0xD5`, immediately before the data
                // is programmed (`docs/reference/01-da-protocol.md` §4.6).
                let Some(range) = accepted_da_range(start, end).filter(|_| !self.refuse_ranges)
                else {
                    // Measured: a refusal is a **single** `A5`; the success
                    // path's ACK and status byte are both skipped. A client that
                    // always reads two bytes consumes the first byte of the next
                    // reply, and one that always reads one leaves the `0x00` of
                    // a success behind.
                    //
                    // A refused command records nothing, so the ranges from an
                    // earlier `0xD3` stay in place.
                    self.queue_bytes(&[NAK]);
                    return;
                };
                self.da_range = range;
                self.da_cursor = start;
                // Measured: on success `0xD3` answers with **two** bytes, ACK
                // then status. A client that reads one leaves the status byte in
                // the buffer and shifts every later frame.
                self.queue_bytes(&[ACK, STATUS_OK]);
            }
            OPCODE_WRITE => {
                self.da_packet_len = be_u32(&frame, 1)
                    .and_then(|length| usize::try_from(length).ok())
                    .filter(|length| *length > 0)
                    .unwrap_or(WRITE_PACKET_LEN);
                // Measured: the erase happens here, not at `0xD3` time. The DA
                // erases the sector holding the first range's start before it
                // sends the second `0x5A` (`0x0010251C`-`0x00102560`), and every
                // later sector as the write reaches it
                // (`0x0010268C`-`0x001026BC`). The fake erases the whole
                // recorded range as soon as the command is accepted, so a packet
                // can never be programmed into a sector that was not erased
                // first; when a sector is erased within the command is not
                // visible on the wire.
                //
                // With no `0xD3` accepted first the recorded range is still
                // `(0, 0)`, so a real DA would erase the sector at address
                // zero. The fake **records** that the client did it instead of
                // damaging the window: destroying the fixture would make the
                // failure land somewhere other than the assertion that is
                // about it, and `wrote_an_empty_range` is what the safety tests
                // assert stays false.
                let (start, end) = self.da_range;
                if self.da_range == (0, 0) {
                    self.wrote_an_empty_range = true;
                } else {
                    self.erase_range(start, end);
                }
                self.da_cursor = start;
                self.da_stage = DaStage::Packet;
                self.queue_bytes(&[ACK, ACK]);
            }
            OPCODE_RESTART => {
                // Measured: no argument, no reply. The real chip then boots its
                // firmware; a read-back needs the BootROM again, which a test
                // asks for with `set_modelled`.
                self.restarted = true;
                self.mode = Mode::Firmware;
                self.line.clear();
            }
            _ => self.queue_bytes(&[NAK]),
        }
    }

    fn dispatch_da_packet(&mut self) {
        let needed = self.da_packet_len.saturating_add(2);
        if self.line.len() < needed {
            return;
        }
        let frame = std::mem::take(&mut self.line);
        let Some((payload, trailer)) = frame.split_at_checked(self.da_packet_len) else {
            return;
        };
        if self.reject_packets > 0 {
            self.reject_packets -= 1;
            self.queue_bytes(&[NAK]);
            return;
        }
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
        // sum of the **payload bytes only**, big-endian.
        if checksum != byte_sum(payload) {
            self.queue_bytes(&[NAK]);
            return;
        }
        // The last packet of a range is short on the wire: the DA zero-fills it
        // to the command's length before programming (`0x001019C0`–`0x001019D8`,
        // §4.4), so the tail of the range is zeros, not the old flash content.
        // ⚠️ 未検証 for the *write* direction: §4.4 reads that padding off the
        // read serialiser, and `docs/dev/05-verification.md` §1 did not record
        // whether the final 92-byte packet's tail is padded. The hardware note
        // that "every packet was accepted" holds either way, because the
        // padding is inside the payload the sum covers.
        let leftover = self
            .da_range
            .1
            .saturating_sub(self.da_cursor)
            .saturating_add(1);
        let room = usize::try_from(leftover).unwrap_or(usize::MAX);
        // The DA addresses flash from zero (`BASE_ADDR_FIRMWARE_M3318`), so its
        // cursor is already an offset into this window; `m241_core`'s fake maps
        // the same bytes at the BootROM's base instead.
        let offset = self.da_cursor as usize;
        for (index, byte) in payload.iter().take(room).enumerate() {
            if let Some(slot) = self.memory.get_mut(offset.saturating_add(index)) {
                *slot = *byte;
            }
        }
        let advance = payload.len().min(room);
        self.da_cursor = self.da_cursor.saturating_add(advance as u32);
        self.accepted_packets += 1;
        self.queue_bytes(&[CONT]);
        // The transfer is complete once the cursor has passed the `end` the
        // accepted `0xD3` recorded -- the real DA counts packets from the range
        // table, not from the amount of data the host chose to send.
        if self.da_cursor > self.da_range.1 {
            self.da_stage = DaStage::Command;
            self.queue_bytes(&[ACK, ACK]);
        }
    }

    /// Erase every modelled sector the range covers, as `0xD5` does before it
    /// programs data.
    ///
    /// The real erase is per sector, resolved from the DA's flash descriptor;
    /// the fake aligns to [`FLASH_SECTOR_SIZE`] because that layout is not
    /// decoded (`docs/reference/01-da-protocol.md` §6).
    fn erase_range(&mut self, start: u32, end: u32) {
        let first = start / FLASH_SECTOR_SIZE;
        let last = end / FLASH_SECTOR_SIZE;
        for sector in first..=last {
            let base = sector.saturating_mul(FLASH_SECTOR_SIZE);
            for at in base..base.saturating_add(FLASH_SECTOR_SIZE) {
                if let Some(slot) = self.memory.get_mut(at as usize) {
                    *slot = 0xFF;
                }
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

/// The 16-bit sum of the payload bytes, big-endian, as `0xD5` expects it.
///
/// `docs/reference/01-da-protocol.md` §4.4: `FUN_001019F0` compares the two-byte
/// trailer that follows the payload with the accumulated byte sum, and the
/// trailer goes high byte first like every other field. The tests compute the
/// checksum with this function rather than a literal so a change in the
/// arithmetic cannot silently agree with itself — the two halves of the
/// protocol are pinned to `docs/reference/01-da-protocol.md` by the citation,
/// not by sharing an assumption.
#[must_use]
pub fn byte_sum(payload: &[u8]) -> u16 {
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
pub fn enter_download_agent(device: &mut FakeDa) -> Vec<u8> {
    let jump = [OPCODE_JUMP, 0x00, 0x10, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00];
    if device.write_all(&jump).is_err() {
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
        // A correct packet: the rejection must come from `rejecting_packets`,
        // not from a checksum error, or the test would pass for the wrong reason.
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
    fn the_simulator_helpers_build_wire_ready_bytes() {
        assert_eq!(byte_sum(&[0x01, 0x02, 0x03]), 6);
    }
}
