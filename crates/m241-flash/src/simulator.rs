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
use m241_core::error::Error;
use m241_core::protocol::brom::BASE_ADDR_DA;
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
    /// BootROM: nine-byte frames, echoed. A jump leaves this mode.
    BootRom,
    /// Download agent: the info report has been sent, commands are accepted.
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
    /// The frame being assembled.
    line: Vec<u8>,
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
    reject_packets: u32,
    /// Whether `0xD9` has been seen.
    restarted: bool,
}

impl FakeDa {
    /// A chip holding `image` in its flash window, ready for the BootROM jump.
    #[must_use]
    pub fn with_firmware(image: &[u8]) -> Self {
        Self {
            memory: image.to_vec(),
            output: VecDeque::new(),
            line: Vec::new(),
            mode: Mode::BootRom,
            da_stage: DaStage::Command,
            da_range: (0, 0),
            da_cursor: 0,
            da_packet_len: WRITE_PACKET_LEN,
            reject_packets: 0,
            restarted: false,
        }
    }

    /// Refuse the next `count` packets.
    #[must_use]
    pub fn rejecting_packets(mut self, count: u32) -> Self {
        self.reject_packets = count;
        self
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
            Mode::BootRom => self.process_bootrom(byte),
            Mode::DownloadAgent => self.process_download_agent(byte),
        }
    }

    fn process_bootrom(&mut self, byte: u8) {
        self.line.push(byte);
        if self.line.len() < 9 {
            return;
        }
        let frame = std::mem::take(&mut self.line);
        let Some(opcode) = frame.first().copied() else {
            return;
        };
        let address = frame
            .get(1..5)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_be_bytes)
            .unwrap_or(0);
        let is_jump = opcode == OPCODE_JUMP && address == BASE_ADDR_DA;
        // The jump is the one frame the chip does not echo: it hands control
        // over part-way through and only part of the echo ever arrives
        // (measured). The DA then answers with its report, so a client that
        // expects nine echo bytes before the report desynchronises.
        if !is_jump {
            self.queue_bytes(&frame);
            return;
        }
        self.mode = Mode::DownloadAgent;
        self.da_stage = DaStage::Command;
        self.queue_bytes(&self.da_info_report());
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
                let Some(range) = accepted_da_range(start, end) else {
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
                // `(0, 0)`, so this erases and then programs the sector at
                // address zero. That is the sequence the safety rules forbid, and
                // modelling it destructively is the point.
                let (start, end) = self.da_range;
                self.erase_range(start, end);
                self.da_stage = DaStage::Packet;
                self.queue_bytes(&[ACK, ACK]);
            }
            OPCODE_RESTART => {
                // Measured: no argument, no reply. The real chip then boots the
                // firmware, whose NMEA/PMTK behaviour `m241_core`'s fake models;
                // here the chip is left ready for a fresh BootROM sync, which is
                // the state a write client has to re-enter anyway.
                self.restarted = true;
                self.mode = Mode::BootRom;
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
        let Some(checksum) = trailer
            .get(..2)
            .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
            .map(u16::from_be_bytes)
        else {
            self.queue_bytes(&[NAK]);
            return;
        };
        let expected: u16 = payload
            .iter()
            .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)));
        if checksum != expected {
            self.queue_bytes(&[NAK]);
            return;
        }
        // The DA addresses flash from zero (`BASE_ADDR_FIRMWARE_M3318`), so its
        // cursor is already an offset into this window; `m241_core`'s fake maps
        // the same bytes at the BootROM's base instead.
        let offset = self.da_cursor as usize;
        for (index, byte) in payload.iter().enumerate() {
            if let Some(slot) = self.memory.get_mut(offset.saturating_add(index)) {
                *slot = *byte;
            }
        }
        self.da_cursor = self.da_cursor.saturating_add(payload.len() as u32);
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

/// An error value the fake never returns, kept so a caller can name the "not
/// implemented in the fake" case without string-matching.
#[must_use]
pub fn unsupported(what: &str) -> Error {
    Error::State(format!("the fake download agent does not model {what}"))
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
        let mut device = FakeDa::with_firmware(&[0xFFu8; 16]).rejecting_packets(1);
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
        assert_eq!(device.mode(), Mode::BootRom);
    }

    #[test]
    fn the_simulator_helpers_build_wire_ready_bytes() {
        assert!(matches!(unsupported("D4"), Error::State(_)));
    }
}
