//! A fake M-241, good enough to drive the real clients in a test.
//!
//! The simulator is not a mock: it speaks the same wire protocol as the chip, so
//! [`crate::protocol::PmtkClient`] and [`crate::protocol::MtkBootRom`] run their
//! **real** code against it. A protocol regression therefore fails a test
//! instead of corrupting a unit, which is the whole reason this module is
//! shipped rather than hidden in `tests/`.
//!
//! # What it models
//!
//! * the running firmware: PMTK command/reply framing, NMEA streaming, the
//!   `$PMTK182` log-transfer family and `$PMTK180` handing the chip to BootROM;
//! * the BootROM: the four-probe sync, the nine-byte command frames, echo, the
//!   two read widths and their different byte orders, and the `0xA4` checksum.
//!
//! It stops at the Download Agent on purpose. The DA is what erases and programs
//! flash, it lives in `m241-flash`, and a fake that modelled it would put the
//! destructive command set back inside the crate that must stay free of it, in
//! the one place a source gate cannot see it — a test fixture that ships. The
//! DA's model, including the measured range and write commands, is
//! `m241_flash::simulator`; this fake answers a jump with what the wire actually
//! carries and changes no state.
//!
//! # What it deliberately does not model
//!
//! Timing. A real read has a delay before its data arrives; here everything is
//! immediate. That keeps tests fast and deterministic, and it means a test can
//! never pass *because* of a delay — the client's timeout handling is exercised
//! by leaving a reply out, not by making one slow.

use std::collections::VecDeque;

use crate::error::{Error, Result};
use crate::protocol::brom::{
    BASE_ADDR_FIRMWARE, BOOT_REPLY, BOOT_SYNC, OPCODE_CHECKSUM, OPCODE_JUMP, OPCODE_READ16,
    OPCODE_READ32,
};
use crate::protocol::frame::{checksum_xor16, swap_pairs};
use crate::protocol::pmtk::{checksum, line as pmtk_line};
use crate::protocol::port::Transport;

/// The firmware base the fake chip maps its flash window at.
pub const FLASH_BASE: u32 = BASE_ADDR_FIRMWARE;

/// `$PMTK180*3B`: the only way into BootROM from a running firmware.
const PMTK180_BODY: &str = "PMTK180";

/// What the fake chip is currently emulating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Firmware running: NMEA and PMTK.
    Running,
    /// BootROM: the four-probe sync, then nine-byte frames.
    ///
    /// A jump (`0xA8`) leaves the chip here: the fake does not model the
    /// Download Agent, which is the write crate's subject.
    BootRom,
}

/// Where the BootROM state machine has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootRomStage {
    /// Waiting for the next sync probe byte.
    Sync { index: usize },
    /// Accepting nine-byte frames.
    Command,
}

/// A fake M-241 that implements [`Transport`].
///
/// Build it with [`FakeDevice::new`], load a firmware image and a log, hand it to
/// a client, and read the state back through that client's `transport_mut`.
#[derive(Debug)]
pub struct FakeDevice {
    /// The flash window, addressed from [`FLASH_BASE`].
    memory: Vec<u8>,
    /// The track log as the PMTK182 family sees it (addressed from zero).
    log: Vec<u8>,
    /// Bytes waiting to be read by the host.
    output: VecDeque<u8>,
    /// The NMEA line being assembled.
    line: Vec<u8>,
    /// Command bodies the host sent, in order (for "which commands, in what
    /// order" assertions in tests).
    commands: Vec<String>,
    mode: Mode,
    bootrom_stage: BootRomStage,
    /// Whether the firmware is currently streaming NMEA sentences.
    streaming: bool,
    /// Whether the firmware is currently writing to its log.
    logging_enabled: bool,
    /// The baud rate the host believes it is using.
    baud: u32,
    /// Whether the reset lines were last asserted.
    control_lines: bool,
    /// The `$PMTK705` identity this unit reports.
    release: String,
    build: String,
    model: String,
    sdk: String,
    /// Number of upcoming log reads the device will answer only after the
    /// client's timeout (zero means "answer normally").
    drop_log_replies: u32,
    /// Whether the frame currently being assembled is echoed back.
    ///
    /// Reads, writes and checksums are echoed byte for byte; a jump is not,
    /// because the chip hands control over part-way through the frame (measured:
    /// only part of a jump's echo ever arrives). Requiring the echo there would
    /// desynchronise the next command, so the simulator does not produce one.
    echo_frame: bool,
}

impl FakeDevice {
    /// A device with V1.13 identity strings and an empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::with_identity("B-core_1.20", "0043", "01017-00F", "1.0")
    }

    /// A device reporting a specific `$PMTK705` identity.
    #[must_use]
    pub fn with_identity(release: &str, build: &str, model: &str, sdk: &str) -> Self {
        Self {
            memory: Vec::new(),
            log: Vec::new(),
            output: VecDeque::new(),
            line: Vec::new(),
            commands: Vec::new(),
            mode: Mode::Running,
            bootrom_stage: BootRomStage::Sync { index: 1 },
            // A live logger streams NMEA from the moment it is powered on, which
            // is exactly the condition `$PMTK180` needs.
            streaming: true,
            logging_enabled: true,
            baud: 115_200,
            control_lines: false,
            release: release.to_owned(),
            build: build.to_owned(),
            model: model.to_owned(),
            sdk: sdk.to_owned(),
            drop_log_replies: 0,
            echo_frame: true,
        }
    }

    /// Load a firmware image into the flash window.
    #[must_use]
    pub fn with_firmware(mut self, image: &[u8]) -> Self {
        self.memory = image.to_vec();
        self
    }

    /// Load a track log.
    #[must_use]
    pub fn with_log(mut self, log: &[u8]) -> Self {
        self.log = log.to_vec();
        self
    }

    /// Start with NMEA not streaming, e.g. because the unit is already in the
    /// BootROM or powered off.
    #[must_use]
    pub fn without_nmea(mut self) -> Self {
        self.streaming = false;
        self
    }

    /// Put the chip in BootROM mode past the sync, accepting nine-byte frames.
    ///
    /// The sync itself is not skipped by the state machine: a command-stage chip
    /// still answers a fresh `0xA0` probe sequence, because `0xA0` is not a
    /// BootROM opcode and the real chip restarts the handshake on it. That is
    /// what makes `sync()` work here as well.
    ///
    /// Tests that are about the frames themselves use this instead of replaying
    /// the entry sequence; the entry sequence itself is covered by a test that
    /// runs the real client against this simulator.
    #[must_use]
    pub fn in_bootrom(mut self) -> Self {
        self.streaming = false;
        self.output.clear();
        self.mode = Mode::BootRom;
        self.bootrom_stage = BootRomStage::Command;
        self
    }

    /// Stay silent for the next `count` log reads, which is what a device with a
    /// flaky cable looks like to the client.
    #[must_use]
    pub fn dropping_log_replies(mut self, count: u32) -> Self {
        self.drop_log_replies = count;
        self
    }

    /// The flash window.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    /// The track log.
    #[must_use]
    pub fn log(&self) -> &[u8] {
        &self.log
    }

    /// Whether the firmware is currently recording.
    #[must_use]
    pub fn logging_enabled(&self) -> bool {
        self.logging_enabled
    }

    /// Whether NMEA is currently streaming.
    #[must_use]
    pub fn streaming(&self) -> bool {
        self.streaming
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The baud rate the host last asked for.
    #[must_use]
    pub fn baud(&self) -> u32 {
        self.baud
    }

    /// Whether the reset lines were last asserted.
    #[must_use]
    pub fn control_lines(&self) -> bool {
        self.control_lines
    }

    /// The command bodies the host sent, in order.
    #[must_use]
    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    /// A device whose NMEA output is already flowing, as it is on a powered-on
    /// logger.
    ///
    /// A client that probes for the stream before sending `$PMTK180` needs to see
    /// one; staging it here keeps that condition explicit instead of relying on
    /// some earlier command having left bytes behind.
    #[must_use]
    pub fn with_nmea(mut self, count: usize) -> Self {
        self.queue_nmea(count);
        self
    }

    /// Queue `count` NMEA sentences so a stream probe sees a live stream.
    ///
    /// The real logger emits one sentence per second; queuing them lets a test
    /// answer "is it streaming?" without waiting a second per sentence.
    pub fn queue_nmea(&mut self, count: usize) {
        for _ in 0..count {
            self.queue_line(
                "$GPGGA,221320.00,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00",
            );
        }
    }

    /// The firmware's own log-transfer commands the host has sent, in order.
    #[must_use]
    pub fn log_commands(&self) -> Vec<&str> {
        self.commands
            .iter()
            .filter(|body| body.starts_with("PMTK182"))
            .map(String::as_str)
            .collect()
    }

    // -- byte plumbing -----------------------------------------------------

    fn queue_line(&mut self, text: &str) {
        for byte in text.bytes() {
            self.output.push_back(byte);
        }
        self.output.push_back(b'\r');
        self.output.push_back(b'\n');
    }

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

    // -- the firmware ------------------------------------------------------

    /// Consume one byte while the firmware is running.
    fn process_running(&mut self, byte: u8) {
        if byte == b'\n' {
            let raw = std::mem::take(&mut self.line);
            let text = String::from_utf8_lossy(&raw).trim().to_owned();
            if !text.is_empty() {
                self.handle_pmtk(&text);
            }
            return;
        }
        if byte == b'\r' {
            return;
        }
        // Bound the line so a desynchronised host cannot grow it without limit.
        if self.line.len() < 512 {
            self.line.push(byte);
        } else {
            self.line.clear();
        }
    }

    fn handle_pmtk(&mut self, text: &str) {
        let Some(rest) = text.strip_prefix('$') else {
            return;
        };
        let Some((body, carried)) = rest.split_once('*') else {
            return;
        };
        // A sentence with a bad checksum is dropped in silence: that is what the
        // firmware does, and it is why the client reports a timeout rather than
        // a rejection for a sentence that never arrived intact.
        if !carried.eq_ignore_ascii_case(&format!("{:02X}", checksum(body))) {
            return;
        }
        self.commands.push(body.to_owned());

        if body == PMTK180_BODY {
            // The stream stops and the chip starts listening for the BootROM
            // sync. Anything already queued belonged to the old mode.
            self.streaming = false;
            self.output.clear();
            self.mode = Mode::BootRom;
            self.bootrom_stage = BootRomStage::Sync { index: 0 };
            return;
        }

        let mut fields = body.split(',');
        if fields.next() != Some("PMTK182") {
            if body == "PMTK605" {
                self.queue_line(&pmtk_line(&format!(
                    "PMTK705,{},{},{},{}",
                    self.release, self.build, self.model, self.sdk
                )));
            }
            return;
        }
        let Some(subcommand) = fields.next() else {
            return;
        };
        match subcommand {
            "2" => self.handle_log_query(fields.next().unwrap_or("")),
            "5" => {
                self.logging_enabled = false;
                self.queue_line(&pmtk_line("PMTK001,182,5,3"));
            }
            "4" => {
                self.logging_enabled = true;
                self.queue_line(&pmtk_line("PMTK001,182,4,3"));
            }
            // `$PMTK182,6` erases the log. The fake does not answer it, because
            // answering it would mean modelling a destructive command in this
            // crate; the erase client and its fake are `m241_flash`'s.
            "7" => self.handle_log_read(fields.next(), fields.next()),
            _ => {}
        }
    }

    /// `$PMTK182,2,<n>`: the status queries.
    fn handle_log_query(&mut self, which: &str) {
        match which {
            // Bytes used, as eight hexadecimal digits with no `0x`.
            "8" => {
                let reply = format!("PMTK182,3,8,{:08X}", self.log.len());
                self.queue_line(&pmtk_line(&reply));
            }
            "1" => {
                let reply = format!("PMTK182,3,1,{}", if self.logging_enabled { 1 } else { 0 });
                self.queue_line(&pmtk_line(&reply));
            }
            "2" => {
                self.queue_line(&pmtk_line("PMTK182,3,2,8000003D"));
            }
            _ => {}
        }
    }

    /// `$PMTK182,7,<hex addr>,<hex len>`: one chunk of the log.
    fn handle_log_read(&mut self, address: Option<&str>, length: Option<&str>) {
        if self.drop_log_replies > 0 {
            self.drop_log_replies -= 1;
            return;
        }
        let (Some(address), Some(length)) = (address, length) else {
            return;
        };
        let (Some(address), Some(length)) = (
            usize::from_str_radix(address, 16).ok(),
            usize::from_str_radix(length, 16).ok(),
        ) else {
            return;
        };
        let chunk = match self.log.get(address..address.saturating_add(length)) {
            Some(chunk) => chunk,
            // A real unit answers a read past the end with what it has; the
            // client's address check catches a reply for the wrong offset.
            None => &[],
        };
        let mut reply = format!("PMTK182,8,{address:08X},");
        for byte in chunk {
            reply.push_str(&format!("{byte:02X}"));
        }
        self.queue_line(&pmtk_line(&reply));
    }

    // -- the BootROM -------------------------------------------------------

    fn process_bootrom(&mut self, byte: u8) {
        match self.bootrom_stage {
            BootRomStage::Sync { index } => {
                let Some(expected) = BOOT_SYNC.get(index) else {
                    self.bootrom_stage = BootRomStage::Command;
                    return;
                };
                if byte != *expected {
                    return;
                }
                if let Some(reply) = BOOT_REPLY.get(index) {
                    self.queue_bytes(&[*reply]);
                }
                let next = index + 1;
                self.bootrom_stage = if next >= BOOT_SYNC.len() {
                    BootRomStage::Command
                } else {
                    BootRomStage::Sync { index: next }
                };
            }
            BootRomStage::Command => {
                // 0xA0 is not a BootROM opcode, so the real chip treats it as the
                // start of another sync dance. Modelling that is what lets a
                // second `sync()` work on an already-synced chip.
                if byte == BOOT_SYNC[0] && self.line.is_empty() {
                    self.bootrom_stage = BootRomStage::Sync { index: 1 };
                    if let Some(reply) = BOOT_REPLY.first() {
                        self.queue_bytes(&[*reply]);
                    }
                    return;
                }
                if self.line.is_empty() {
                    // The first byte of a frame decides whether this frame is
                    // echoed at all: a jump is not, everything else is.
                    self.echo_frame = byte != OPCODE_JUMP;
                }
                self.line.push(byte);
                if self.echo_frame {
                    self.queue_bytes(&[byte]);
                }
                if self.line.len() < 9 {
                    return;
                }
                let frame = std::mem::take(&mut self.line);
                self.echo_frame = true;
                self.dispatch_bootrom(&frame);
            }
        }
    }

    fn dispatch_bootrom(&mut self, frame: &[u8]) {
        let Some(opcode) = frame.first().copied() else {
            return;
        };
        let address = frame
            .get(1..5)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_be_bytes)
            .unwrap_or(0);
        let count = frame
            .get(5..9)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_be_bytes)
            .unwrap_or(0);

        match opcode {
            OPCODE_READ16 => {
                let data = self.read_flash(address, count as usize * 2);
                // 0xA2 answers with each 16-bit pair swapped on the wire.
                self.queue_bytes(&swap_pairs(&data));
            }
            OPCODE_READ32 => {
                let data = self.read_flash(address, count as usize * 4);
                // 0xAF answers with each 32-bit word byte-reversed.
                let mut wire = Vec::with_capacity(data.len());
                for word in data.chunks(4) {
                    wire.extend(word.iter().rev().copied());
                }
                self.queue_bytes(&wire);
            }
            OPCODE_CHECKSUM => {
                let data = self.read_flash(address, count as usize * 2);
                self.queue_bytes(&checksum_xor16(&data).to_be_bytes());
            }
            // A jump hands the chip to the Download Agent, which this fake does
            // not model: the DA erases and programs flash, and putting that
            // state machine back here would defeat the crate split. The chip
            // stops echoing part-way through the frame, so nothing is queued and
            // no state changes; a read path never sends a jump, because reading
            // never leaves the BootROM.
            _ => {}
        }
    }

    /// Copy `length` bytes of the flash window, zero-filled past its end.
    ///
    /// Reads past the image are not an error on the wire: the chip answers with
    /// whatever the bus returns, and the client's own length checks are what
    /// catch a request that was too long.
    fn read_flash(&self, address: u32, length: usize) -> Vec<u8> {
        let offset = address.saturating_sub(FLASH_BASE) as usize;
        let mut out = vec![0u8; length];
        for (index, slot) in out.iter_mut().enumerate() {
            if let Some(byte) = offset.checked_add(index).and_then(|at| self.memory.get(at)) {
                *slot = *byte;
            }
        }
        out
    }

    /// Push one byte into the state machine, whichever mode is active.
    fn consume(&mut self, byte: u8) {
        match self.mode {
            Mode::Running => self.process_running(byte),
            Mode::BootRom => self.process_bootrom(byte),
        }
    }
}

impl Default for FakeDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for FakeDevice {
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

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.baud = baud;
        Ok(())
    }

    fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
        self.control_lines = asserted;
        Ok(())
    }
}

/// NMEA checksum over `body`, for building replies in tests.
#[must_use]
pub fn nmea_checksum(body: &str) -> u8 {
    crate::protocol::pmtk::checksum(body)
}

/// A one-line helper: `$body*HH\r\n`.
#[must_use]
pub fn nmea_line(body: &str) -> String {
    pmtk_line(body)
}

/// Build a byte stream that is not a protocol message, for "desynchronised line"
/// tests.
#[must_use]
pub fn garbage(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| (index as u8).wrapping_mul(37))
        .collect()
}

/// An error value the simulator never returns, kept so a caller can name the
/// "not implemented in the fake" case without string-matching.
#[must_use]
pub fn unsupported(what: &str) -> Error {
    Error::State(format!("the fake device does not model {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::brom::MtkBootRom;
    use crate::protocol::pmtk::PmtkClient;

    fn client() -> PmtkClient<FakeDevice> {
        PmtkClient::new(FakeDevice::new())
            .with_stream_window(std::time::Duration::from_millis(20))
            .with_reply_timeout(std::time::Duration::from_millis(200))
    }

    #[test]
    fn the_fake_device_identifies_itself_like_the_measured_unit() {
        let mut client = client();
        let info = client.request_identity().expect("identity reply");
        assert_eq!(info.release, "B-core_1.20");
        assert_eq!(info.build, "0043");
        assert_eq!(info.model, "01017-00F");
        assert_eq!(info.sdk.as_deref(), Some("1.0"));
    }

    #[test]
    fn the_identity_exchange_writes_the_measured_sentence() {
        let mut client = client();
        client.request_identity().expect("identity reply");
        let commands = client.transport_mut().commands();
        assert_eq!(commands.first().map(String::as_str), Some("PMTK605"));
    }

    #[test]
    fn a_stream_probe_sees_queued_nmea_and_reports_streaming() {
        let mut client = client();
        client.transport_mut().queue_nmea(1);
        assert_eq!(
            client.probe_stream().expect("read"),
            crate::protocol::pmtk::StreamState::Streaming
        );
    }

    #[test]
    fn a_silent_unit_reports_silence_not_an_error() {
        let mut client = PmtkClient::new(FakeDevice::new().without_nmea())
            .with_stream_window(std::time::Duration::from_millis(20));
        assert_eq!(
            client.probe_stream().expect("read"),
            crate::protocol::pmtk::StreamState::Silent
        );
    }

    #[test]
    fn a_sentence_with_a_bad_checksum_is_ignored_like_the_firmware_does() {
        let mut device = FakeDevice::new();
        device.write_all(b"$PMTK605*00\r\n").expect("write");
        assert!(device.commands().is_empty(), "a bad checksum is dropped");
    }

    #[test]
    fn pmtk180_stops_the_stream_and_moves_the_chip_to_the_bootrom() {
        let mut client = client();
        client.transport_mut().queue_nmea(1);
        client.enter_bootrom().expect("the stream is live");
        let device = client.transport_mut();
        assert_eq!(device.mode(), Mode::BootRom);
        assert!(!device.streaming());
        assert_eq!(device.baud(), crate::protocol::brom::DEFAULT_BOOTROM_BAUD);
    }

    #[test]
    fn pmtk180_is_refused_when_no_stream_is_running() {
        let mut client = PmtkClient::new(FakeDevice::new().without_nmea())
            .with_stream_window(std::time::Duration::from_millis(20))
            .with_reply_timeout(std::time::Duration::from_millis(20));
        let err = client.enter_bootrom().expect_err("silent unit");
        assert!(format!("{err}").contains("no NMEA stream"), "got {err}");
        // Refusing must not have sent the sentence: at a silent unit it would be
        // ignored anyway, and sending it blind is the habit that later becomes a
        // pulse at a streaming firmware.
        assert!(client.transport_mut().commands().is_empty());
    }

    #[test]
    fn the_bootrom_sync_gets_the_measured_replies_through_the_real_client() {
        let mut device = FakeDevice::new();
        device.write_all(b"$PMTK180*3B\r\n").expect("enter bootrom");
        let mut brom = MtkBootRom::new(device);
        brom.sync().expect("the four documented replies");
    }

    #[test]
    fn a_read16_through_the_fake_returns_memory_order_bytes() {
        let image: Vec<u8> = (0..64u8).map(|value| value.wrapping_mul(3)).collect();
        let mut device = FakeDevice::new().with_firmware(&image);
        device.write_all(b"$PMTK180*3B\r\n").expect("enter bootrom");
        let mut brom = MtkBootRom::new(device);
        brom.sync().expect("sync");
        let data = brom.read16(FLASH_BASE, 64).expect("read");
        assert_eq!(data, image);
    }

    #[test]
    fn a_read32_through_the_fake_returns_memory_order_bytes() {
        let image: Vec<u8> = (0..96u8).map(|value| value.wrapping_mul(7)).collect();
        let mut device = FakeDevice::new().with_firmware(&image);
        device.write_all(b"$PMTK180*3B\r\n").expect("enter bootrom");
        let mut brom = MtkBootRom::new(device);
        brom.sync().expect("sync");
        let data = brom.read32(FLASH_BASE, 96).expect("read");
        assert_eq!(data, image);
    }

    #[test]
    fn the_log_transfer_goes_through_the_real_downloader() {
        let log: Vec<u8> = (0..0x1200u32).map(|value| (value % 251) as u8).collect();
        let mut client = PmtkClient::new(FakeDevice::new().with_log(&log))
            .with_reply_timeout(std::time::Duration::from_millis(200));
        let mut progress = Vec::new();
        let mut callback = |done: usize, total: usize| progress.push((done, total));
        let data = client.download_log(&mut callback).expect("download");
        assert_eq!(data, log);
        assert!(!progress.is_empty());
        assert_eq!(progress.last().copied(), Some((log.len(), log.len())));
    }

    #[test]
    fn the_downloader_pauses_and_resumes_logging_around_the_transfer() {
        let log = vec![0x42u8; 0x900];
        let mut client = PmtkClient::new(FakeDevice::new().with_log(&log))
            .with_reply_timeout(std::time::Duration::from_millis(200));
        let mut callback = |_done: usize, _total: usize| {};
        client.download_log(&mut callback).expect("download");
        let device = client.transport_mut();
        // Order matters: usage, stop, reads, resume. A transfer that read while
        // the device was still writing would return inconsistent blocks.
        let commands = device.log_commands();
        assert_eq!(commands.first().copied(), Some("PMTK182,2,8"));
        assert_eq!(commands.get(1).copied(), Some("PMTK182,5"));
        assert_eq!(commands.last().copied(), Some("PMTK182,4"));
        assert!(device.logging_enabled(), "logging must be resumed");
    }

    #[test]
    fn logging_is_resumed_even_when_the_transfer_fails() {
        // A unit left not recording loses the next trip, which is worse than a
        // failed download; the resume must hold on the error path too.
        let log = vec![0x11u8; 0x900];
        let mut client = PmtkClient::new(FakeDevice::new().with_log(&log).dropping_log_replies(1))
            .with_reply_timeout(std::time::Duration::from_millis(60));
        let mut callback = |_done: usize, _total: usize| {};
        let err = client.download_log(&mut callback).expect_err("silent read");
        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(
            client.transport_mut().logging_enabled(),
            "logging must be resumed after a failure"
        );
        let commands = client.transport_mut().log_commands();
        assert_eq!(commands.last().copied(), Some("PMTK182,4"));
    }

    #[test]
    fn a_jump_frame_is_echoed_part_way_and_changes_no_state() {
        // The fake stops at the BootROM: the Download Agent is the write crate's
        // subject, and modelling it here would put the erase/program state
        // machine back inside the read-only crate, in a file that ships. A jump
        // therefore leaves the chip in the BootROM and queues no report — a read
        // path never sends one, because reading never leaves the BootROM.
        let mut device = FakeDevice::new().with_firmware(&[0u8; 16]).in_bootrom();
        let jump = [OPCODE_JUMP, 0x00, 0x10, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00];
        device.write_all(&jump).expect("write the jump frame");
        assert_eq!(device.mode(), Mode::BootRom, "a jump must not enter the DA");
        assert_eq!(device.memory().len(), 16, "a jump must not touch the flash");
        let mut nothing = [0u8; 1];
        assert_eq!(
            device.read(&mut nothing).expect("no data follows"),
            0,
            "a jump must not queue a download-agent report"
        );
    }

    #[test]
    fn the_control_lines_are_recorded_so_a_test_can_prove_they_were_not_pulsed() {
        let mut device = FakeDevice::new();
        assert!(!device.control_lines());
        device.set_control_lines(true).expect("assert");
        assert!(device.control_lines());
    }

    #[test]
    fn the_simulator_helpers_build_wire_ready_bytes() {
        assert_eq!(nmea_checksum("PMTK605"), 0x31);
        assert_eq!(nmea_line("PMTK605"), "$PMTK605*31\r\n");
        assert_eq!(garbage(4).len(), 4);
        assert!(matches!(unsupported("x"), Error::State(_)));
    }
}
