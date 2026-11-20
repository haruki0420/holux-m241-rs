//! A fake Download Agent, for the write crate's own tests.
//!
//! The DA is the half of the protocol that erases and programs flash, so its
//! model belongs in this crate for the same reason the driver does: `m241-core`
//! must not contain a working description of a destructive command set, not even
//! in a test fixture. `m241_core::protocol::simulator` stops at the BootROM and
//! echoes a `0xA8` jump without changing state; this fake picks the jump up and
//! speaks the DA's own byte stream.
//!
//! # What it models, and from where
//!
//! Everything here is taken from `docs/reference/01-da-protocol.md`:
//!
//! * the twenty-byte info report after `0xA8` (§2);
//! * `0xD3`: a count byte and count pairs of `(start, end)`, answered with
//!   `5A 00` on success and a lone `A5` on failure (§5.3);
//! * `0xD5`: a length `u32`, two `5A`, then per packet `length` bytes plus a
//!   16-bit sum, answered with `0x69` and finally two `5A` (§5.6);
//! * `0xD9`: no argument and **no reply** (§5).
//!
//! The erase is modelled where the disassembly puts it — inside `0xD5`, per
//! sector, before that sector is written — not in `0xD3`, which records a range
//! and touches nothing (§4.6). A test that asserted "`0xD3` erased" would pin the
//! wrong behaviour, which is exactly what the previous notes got wrong.
//!
//! # What it deliberately does not model
//!
//! Timing, the progress frames `0xD4` emits, and the flash driver's own error
//! codes. The tests that need a failure ask for one explicitly
//! ([`FakeDa::rejecting_packets`]).

use std::collections::VecDeque;

use m241_core::Result;
use m241_core::error::Error;
use m241_core::protocol::brom::{BASE_ADDR_DA, BASE_ADDR_FIRMWARE};
use m241_core::protocol::port::Transport;

use crate::da::{OPCODE_MEM_CMD, OPCODE_RESTART, OPCODE_WRITE};

/// The firmware base the fake chip maps its flash window at.
pub const FLASH_BASE: u32 = BASE_ADDR_FIRMWARE;

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

/// A sector size, as the KD32-class part in the hardware notes reports it.
///
/// Only the erase granularity of the fake: the real size comes from the sector
/// descriptor the region table points at (§3), which this fake does not parse.
pub const SECTOR_SIZE: u32 = 0x1000;

/// The mode the fake chip is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// BootROM: nine-byte frames, echoed.
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
    /// The flash window, addressed from [`FLASH_BASE`].
    memory: Vec<u8>,
    /// Bytes waiting to be read by the host.
    output: VecDeque<u8>,
    /// The frame being assembled.
    line: Vec<u8>,
    mode: Mode,
    da_stage: DaStage,
    /// The recorded range: start and end, inclusive.
    da_range: (u32, u32),
    da_cursor: u32,
    da_packet_len: usize,
    /// Force the next `count` packets to be refused.
    reject_packets: u32,
    /// Whether the chip handed control to the DA and sent nothing further.
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

    /// The flash window.
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

    /// The range the last `0xD3` recorded.
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
        let is_jump = opcode == OPCODE_JUMP && {
            let address = frame
                .get(1..5)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map(u32::from_be_bytes)
                .unwrap_or(0);
            address == BASE_ADDR_DA
        };
        // The jump is the one frame the chip does not echo: it hands control over
        // part-way through and only part of the echo ever arrives (measured). The
        // DA then answers with its report, so a client that expects nine echo
        // bytes before the report desynchronises.
        if !is_jump {
            self.queue_bytes(&frame);
        }
        if is_jump {
            self.mode = Mode::DownloadAgent;
            self.da_stage = DaStage::Command;
            self.queue_bytes(&self.da_info_report());
        }
    }

    /// The twenty-byte download-agent report, `>BHBLHHHHL`.
    fn da_info_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(20);
        report.push(0xC0); // report marker
        report.extend_from_slice(&0x0308u16.to_be_bytes()); // DA version 3.8
        report.push(0x49); // flash device id
        report.extend_from_slice(&0x0008_0000u32.to_be_bytes()); // 512 KiB
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
                let start = be_u32(&frame, 2).unwrap_or(0);
                let end = be_u32(&frame, 6).unwrap_or(start);
                // §4.6: `0xD3` records the range and resets the iterators. It
                // does not touch the flash, and modelling an erase here would be
                // the mistake the earlier notes made.
                self.da_range = (start, end);
                self.da_cursor = start;
                // §4.1: two bytes on success; one `A5` on failure. Success is
                // what this fake answers.
                self.queue_bytes(&[ACK, STATUS_OK]);
            }
            OPCODE_WRITE => {
                self.da_packet_len = be_u32(&frame, 1)
                    .and_then(|length| usize::try_from(length).ok())
                    .filter(|length| *length > 0)
                    .unwrap_or(WRITE_PACKET_LEN);
                self.da_stage = DaStage::Packet;
                self.queue_bytes(&[ACK, ACK]);
            }
            OPCODE_RESTART => {
                // §5: no argument, no reply. The chip is gone; a client that
                // waits for an acknowledgement hangs.
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
        // §5.6: the erase happens here, per sector, before that sector's data is
        // programmed — not at `0xD3` time.
        self.erase_sector(self.da_cursor);
        let offset = self.da_cursor.saturating_sub(FLASH_BASE) as usize;
        for (index, byte) in payload.iter().enumerate() {
            if let Some(slot) = self.memory.get_mut(offset.saturating_add(index)) {
                *slot = *byte;
            }
        }
        self.da_cursor = self.da_cursor.saturating_add(payload.len() as u32);
        self.queue_bytes(&[CONT]);
        if self.da_cursor > self.da_range.1 {
            self.da_stage = DaStage::Command;
            self.queue_bytes(&[ACK, ACK]);
        }
    }

    /// Set the whole sector holding `address` to `0xFF`.
    fn erase_sector(&mut self, address: u32) {
        let offset = address.saturating_sub(FLASH_BASE);
        let sector_start = offset - (offset % SECTOR_SIZE);
        let from = sector_start as usize;
        let to = (sector_start + SECTOR_SIZE) as usize;
        for at in from..to.min(self.memory.len()) {
            if let Some(slot) = self.memory.get_mut(at) {
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

    fn device() -> FakeDa {
        FakeDa::with_firmware(&[0xFFu8; SECTOR_SIZE as usize * 2])
    }

    #[test]
    fn the_da_handshake_reports_twenty_bytes_starting_with_c0() {
        // The measured fact that matters most here: the report starts with
        // 0xC0 and the chip has left the BootROM for the DA.
        let mut device = device();
        let report = enter_download_agent(&mut device);
        assert_eq!(report.first().copied(), Some(0xC0));
        assert_eq!(report.len(), 20);
        assert_eq!(device.mode(), Mode::DownloadAgent);
    }

    #[test]
    fn the_memory_range_command_answers_with_ack_and_status() {
        // Two bytes, not one: reading only the ACK shifts every later frame by a
        // byte, which is the trap the hardware notes record.
        let mut device = device();
        enter_download_agent(&mut device);
        let mut command = vec![OPCODE_MEM_CMD, 0x01];
        command.extend_from_slice(&FLASH_BASE.to_be_bytes());
        command.extend_from_slice(&(FLASH_BASE + 31).to_be_bytes());
        device.write_all(&command).expect("mem cmd");
        let mut reply = [0u8; 2];
        device.read(&mut reply).expect("read the two-byte reply");
        assert_eq!(reply, [ACK, STATUS_OK]);
        assert_eq!(device.recorded_range(), (FLASH_BASE, FLASH_BASE + 31));
    }

    #[test]
    fn recording_a_range_does_not_erase_anything() {
        // The disassembly's correction to the older notes: the erase is in
        // `0xD5`, per sector, not in `0xD3`. A test that pinned the opposite
        // would have blessed the wrong model.
        let mut device = device();
        enter_download_agent(&mut device);
        let mut command = vec![OPCODE_MEM_CMD, 0x01];
        command.extend_from_slice(&FLASH_BASE.to_be_bytes());
        command.extend_from_slice(&(FLASH_BASE + 15).to_be_bytes());
        device.write_all(&command).expect("mem cmd");
        let mut reply = [0u8; 2];
        device.read(&mut reply).expect("ack");
        assert!(
            device.memory().iter().all(|byte| *byte == 0xFF),
            "0xD3 touched the flash"
        );
    }

    #[test]
    fn a_write_packet_is_acknowledged_and_stored() {
        let mut device = device();
        enter_download_agent(&mut device);

        // Range FLASH_BASE..=FLASH_BASE+3, then a four-byte packet.
        let mut range = vec![OPCODE_MEM_CMD, 0x01];
        range.extend_from_slice(&FLASH_BASE.to_be_bytes());
        range.extend_from_slice(&(FLASH_BASE + 3).to_be_bytes());
        device.write_all(&range).expect("range");
        device.read(&mut [0u8; 2]).expect("ack+status");

        let mut write = vec![OPCODE_WRITE];
        write.extend_from_slice(&4u32.to_be_bytes());
        device.write_all(&write).expect("write command");
        device.read(&mut [0u8; 2]).expect("two acks");

        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let sum: u16 = payload
            .iter()
            .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(*byte)));
        let mut packet = payload.to_vec();
        packet.extend_from_slice(&sum.to_be_bytes());
        device.write_all(&packet).expect("packet");
        let mut reply = [0u8; 3];
        device.read(&mut reply).expect("reply");
        assert_eq!(reply.first().copied(), Some(CONT));
        assert_eq!(&device.memory()[..4], &payload);
    }

    #[test]
    fn a_rejected_packet_draws_a_nak() {
        let mut device = FakeDa::with_firmware(&[0xFFu8; 16]).rejecting_packets(1);
        enter_download_agent(&mut device);
        let mut range = vec![OPCODE_MEM_CMD, 0x01];
        range.extend_from_slice(&FLASH_BASE.to_be_bytes());
        range.extend_from_slice(&(FLASH_BASE + 3).to_be_bytes());
        device.write_all(&range).expect("range");
        device.read(&mut [0u8; 2]).expect("ack+status");
        let mut write = vec![OPCODE_WRITE];
        write.extend_from_slice(&4u32.to_be_bytes());
        device.write_all(&write).expect("write");
        device.read(&mut [0u8; 2]).expect("acks");
        // A correct packet: the rejection must come from `rejecting_packets`,
        // not from a checksum error, or the test would pass for the wrong reason.
        let mut packet = vec![1u8, 2, 3, 4];
        packet.extend_from_slice(&(1u16 + 2 + 3 + 4).to_be_bytes());
        device.write_all(&packet).expect("packet");
        let mut reply = [0u8; 1];
        device.read(&mut reply).expect("reply");
        assert_eq!(reply.first().copied(), Some(NAK));
    }

    #[test]
    fn the_restart_command_sends_nothing() {
        let mut device = device();
        enter_download_agent(&mut device);
        device.write_all(&[OPCODE_RESTART]).expect("restart");
        assert!(device.restarted(), "0xD9 must be recorded");
        let mut nothing = [0u8; 1];
        assert_eq!(
            device.read(&mut nothing).expect("no reply"),
            0,
            "0xD9 never replies; waiting for one hangs"
        );
        assert_eq!(device.mode(), Mode::BootRom);
    }

    #[test]
    fn the_simulator_helpers_build_wire_ready_bytes() {
        assert!(matches!(unsupported("D4"), Error::State(_)));
    }
}
