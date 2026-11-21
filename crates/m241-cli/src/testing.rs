//! Test doubles: a simulated logger behind the [`crate::link::Links`] seam, and
//! a console that records instead of printing.
//!
//! Only compiled for tests. The point of the two seams is that a command can be
//! run end to end without a serial port and without a terminal, so a test can
//! assert on the exact text a user would see.
//!
//! [`DeviceHandle`] holds a *factory*, not a device: `m241 fw dump` opens the
//! port twice, and the second read is only meaningful if it starts from a fresh
//! boot. Handing a fake device out per `open` is the same thing a power cycle
//! does to a real unit, so [`DeviceHandle::power_cycle`] is what the test writes
//! where the user would remove the battery.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

use m241_core::protocol::port::Transport;
use m241_core::protocol::simulator::FakeDevice;
use m241_core::{Error, Result};
use serde_json::Value;

use crate::link::{Cable, Links, Opened, PortEntry};
use crate::output::Console;

/// A console that keeps what it was told, for assertions.
#[derive(Debug, Default)]
pub struct RecordingConsole {
    stdout: RefCell<String>,
    stderr: RefCell<String>,
}

impl RecordingConsole {
    /// Everything written as machine output.
    #[must_use]
    pub fn stdout(&self) -> String {
        self.stdout.borrow().clone()
    }

    /// Everything written as progress, warnings and notes.
    #[must_use]
    pub fn stderr(&self) -> String {
        self.stderr.borrow().clone()
    }
}

impl Console for RecordingConsole {
    fn result(&self, line: &str) {
        let mut out = self.stdout.borrow_mut();
        out.push_str(line);
        out.push('\n');
    }

    fn text(&self, text: &str) {
        self.stdout.borrow_mut().push_str(text);
    }

    fn json(&self, value: &Value) -> Result<()> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|err| Error::State(format!("could not serialise JSON: {err}")))?;
        self.result(&text);
        Ok(())
    }

    fn progress(&self, done: usize, total: usize) {
        self.stderr
            .borrow_mut()
            .push_str(&format!("{done}/{total} "));
    }

    fn progress_done(&self) {
        self.stderr.borrow_mut().push('\n');
    }

    fn warn(&self, message: &str) {
        self.stderr
            .borrow_mut()
            .push_str(&format!("warning: {message}\n"));
    }

    fn note(&self, message: &str) {
        self.stderr.borrow_mut().push_str(message);
        self.stderr.borrow_mut().push('\n');
    }
}

/// A directory under the system temporary directory, removed on drop.
///
/// Every command's tests need scratch files. One type rather than the same
/// block in each module: the directory is named after the role the test gives it
/// and the process, so two tests that run in parallel do not share files, and a
/// test that panics halfway leaves a directory the next run clears first.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// Create an empty scratch directory for the test called `role`.
    #[must_use]
    pub fn new(role: &str) -> Self {
        let path = std::env::temp_dir().join(format!("m241-cli-{}-{role}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        Self(path)
    }

    /// The directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the scratch directory.
    #[must_use]
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A transport that replays queued reply bytes and lets a closure answer a command.
///
/// `answer` is called with every complete line written to the port — the
/// request the command sent — and may push the reply into the same queue. Two
/// test modules used to carry a private copy of this; they exist to prove a
/// command sees a *particular* reply (a refusal, a corrupt frame) that the
/// simulated chip would never send.
pub struct ScriptedTransport {
    output: VecDeque<u8>,
    pending: Vec<u8>,
    baud: Mutex<u32>,
    answer: Answer,
}

/// What a [`ScriptedTransport`] does with one request line: it may push reply
/// bytes into the queue the transport reads from.
type Answer = Box<dyn FnMut(&str, &mut VecDeque<u8>)>;

impl ScriptedTransport {
    /// A transport that answers each command line with `answer`.
    #[must_use]
    pub fn new(answer: impl FnMut(&str, &mut VecDeque<u8>) + 'static) -> Self {
        Self {
            output: VecDeque::new(),
            pending: Vec::new(),
            baud: Mutex::new(0),
            answer: Box::new(answer),
        }
    }

    /// A transport that only replays `lines`, byte for byte, then reports idle.
    #[must_use]
    pub fn replaying(lines: &[String]) -> Self {
        let mut output = VecDeque::new();
        for line in lines {
            output.extend(line.as_bytes());
        }
        Self {
            output,
            pending: Vec::new(),
            baud: Mutex::new(0),
            answer: Box::new(|_line, _output| {}),
        }
    }

    /// The baud rate the command asked for when it opened the port.
    #[must_use]
    pub fn baud(&self) -> u32 {
        *self.baud.lock().expect("baud cell")
    }
}

impl std::fmt::Debug for ScriptedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedTransport")
            .field("queued", &self.output.len())
            .field("baud", &self.baud())
            .finish_non_exhaustive()
    }
}

impl Transport for ScriptedTransport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut count = 0usize;
        while count < buf.len() {
            let Some(byte) = self.output.pop_front() else {
                break;
            };
            buf[count] = byte;
            count += 1;
        }
        Ok(count)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.pending.extend_from_slice(buf);
        while let Some(pos) = self.pending.iter().position(|&byte| byte == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&raw);
            (self.answer)(text.trim(), &mut self.output);
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
        *self.baud.lock().expect("baud cell") = baud;
        Ok(())
    }

    fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
        Ok(())
    }
}

/// A [`Links`] implementation with one plausible port that hands out a fresh
/// [`ScriptedTransport`] on every `open`.
pub struct ScriptedLinks {
    factory: Box<dyn Fn() -> ScriptedTransport>,
}

impl ScriptedLinks {
    /// A link whose every `open` returns `factory()`.
    #[must_use]
    pub fn new(factory: impl Fn() -> ScriptedTransport + 'static) -> Self {
        Self {
            factory: Box::new(factory),
        }
    }
}

impl std::fmt::Debug for ScriptedLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedLinks").finish_non_exhaustive()
    }
}

impl Links for ScriptedLinks {
    fn ports(&self) -> Result<Vec<PortEntry>> {
        Ok(vec![PortEntry {
            name: String::from(FAKE_PORT),
            cable: Some(Cable::Cp210x),
            likely: true,
            description: None,
        }])
    }

    fn open(&self, _name: &str, baud: u32) -> Result<Opened> {
        let mut transport = (self.factory)();
        transport.set_baud(baud)?;
        Ok(Opened::new(Box::new(transport)))
    }
}

/// A confirmation prompt that never waits and changes nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoConfirm;

impl crate::Confirm for AutoConfirm {
    fn pause(&self, _message: &str) -> Result<()> {
        Ok(())
    }
}

/// A simulated chip that survives being closed and reopened.
///
/// The factory rebuilds it: opening the port again after [`Self::power_cycle`]
/// reaches a freshly booted unit, which is exactly the state the second dump
/// pass needs.
#[derive(Clone)]
pub struct DeviceHandle {
    factory: Rc<dyn Fn() -> FakeDevice>,
    current: Rc<RefCell<FakeDevice>>,
    stream: Rc<RefCell<Vec<u8>>>,
    open_baud: Rc<Cell<Option<u32>>>,
}

impl std::fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceHandle")
            .field("mode", &self.current.borrow().mode())
            .finish_non_exhaustive()
    }
}

impl DeviceHandle {
    /// Build a handle whose every power cycle runs `factory`.
    #[must_use]
    pub fn new(factory: impl Fn() -> FakeDevice + 'static) -> Self {
        Self {
            current: Rc::new(RefCell::new(factory())),
            factory: Rc::new(factory),
            stream: Rc::new(RefCell::new(Vec::new())),
            open_baud: Rc::new(Cell::new(None)),
        }
    }

    /// Put `lines` on the wire as a raw NMEA stream, independent of `FakeDevice`.
    ///
    /// `FakeDevice` models a unit that answers commands; a live `$GPRMC` stream
    /// is a different thing, and `nmea proxy` only reads it. The lines are
    /// replayed byte for byte by every transport this handle hands out, after
    /// any bytes the fake device itself wants to send; once they run out the
    /// port reports idle (a read of 0), which is what a real port does between
    /// sentences.
    pub fn set_stream(&self, lines: &[String]) {
        let mut stream = self.stream.borrow_mut();
        stream.clear();
        for line in lines {
            stream.extend_from_slice(line.as_bytes());
        }
    }

    /// The baud rate the last `open` asked for, or `None` before any open.
    #[must_use]
    pub fn open_baud(&self) -> Option<u32> {
        self.open_baud.get()
    }

    /// Replace the chip with a freshly booted one.
    pub fn power_cycle(&self) {
        *self.current.borrow_mut() = (self.factory)();
    }

    /// Whether the fake firmware is currently recording.
    #[must_use]
    pub fn logging_enabled(&self) -> bool {
        self.current.borrow().logging_enabled()
    }

    /// The bytes the fake firmware's flash window holds.
    #[must_use]
    pub fn memory(&self) -> Vec<u8> {
        self.current.borrow().memory().to_vec()
    }

    /// The command bodies the host sent, in order.
    #[must_use]
    pub fn commands(&self) -> Vec<String> {
        self.current.borrow().commands().to_vec()
    }
}

/// The transport a command gets when it opens the simulated port.
#[derive(Debug)]
struct SharedFake(DeviceHandle);

impl Transport for SharedFake {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        // The raw stream first (it is the traffic a *live* unit emits), then
        // whatever the fake device has queued as a command reply.
        let mut from_stream = 0usize;
        {
            let mut stream = self.0.stream.borrow_mut();
            while from_stream < buf.len() {
                let Some(byte) = stream.first().copied() else {
                    break;
                };
                stream.remove(0);
                buf[from_stream] = byte;
                from_stream += 1;
            }
        }
        if from_stream > 0 {
            return Ok(from_stream);
        }
        self.0.current.borrow_mut().read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.0.current.borrow_mut().write_all(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.0.current.borrow_mut().flush()
    }

    fn clear_input(&mut self) -> Result<()> {
        self.0.current.borrow_mut().clear_input()
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.0.current.borrow_mut().set_baud(baud)
    }

    fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
        self.0.current.borrow_mut().set_control_lines(asserted)
    }
}

/// The port the simulated device is reachable as.
pub const FAKE_PORT: &str = "SIM0";

/// A [`Links`] implementation over a [`DeviceHandle`].
#[derive(Debug)]
pub struct FakePorts {
    ports: Vec<PortEntry>,
    device: Option<DeviceHandle>,
}

impl FakePorts {
    /// No ports at all; opening anything fails.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            ports: Vec::new(),
            device: None,
        }
    }

    /// A fixed port list. Opening any of them fails: no device is attached.
    #[must_use]
    pub fn with_ports(ports: Vec<PortEntry>) -> Self {
        Self {
            ports,
            device: None,
        }
    }

    /// One plausible port backed by a handle the test keeps.
    #[must_use]
    pub fn shared(device: DeviceHandle) -> Self {
        Self {
            ports: vec![PortEntry {
                name: String::from(FAKE_PORT),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: Some(String::from("simulated M-241")),
            }],
            device: Some(device),
        }
    }

    /// One plausible port backed by `factory`, which runs on every power cycle.
    #[must_use]
    pub fn single(factory: impl Fn() -> FakeDevice + 'static) -> Self {
        Self::shared(DeviceHandle::new(factory))
    }
}

impl Links for FakePorts {
    fn ports(&self) -> Result<Vec<PortEntry>> {
        Ok(self.ports.clone())
    }

    fn open(&self, name: &str, baud: u32) -> Result<Opened> {
        let Some(device) = &self.device else {
            return Err(Error::Port {
                message: format!("no simulated device behind {name}"),
            });
        };
        // Record the baud rate the command asked for, so a test can assert that
        // it spoke NMEA at 38400 rather than at the BootROM's rate.
        device.open_baud.set(Some(baud));
        device.current.borrow_mut().set_baud(baud)?;
        Ok(Opened::new(Box::new(SharedFake(device.clone()))))
    }
}

/// A synthetic V1.13 image: valid header, stock constants, correct length.
///
/// The length matters. `m241_core::firmware::identify` matches a profile by
/// size *and* device string *and* version marker, so a header alone is not
/// identifiable — a fixture has to be as long as the release. 406 KB of `0xFF`
/// costs nothing in a test and keeps the command on its production code path.
#[must_use]
pub fn synthetic_image() -> Vec<u8> {
    use m241_core::firmware::PROFILE_V1_13;
    use m241_core::firmware::image::{
        DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
        FW_SIZE_OFFSET, GPS_EPOCH_UNIX,
    };

    let profile = PROFILE_V1_13;
    let mut data = vec![0xFFu8; profile.size];
    data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
    data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
    let device = profile.device_type.as_bytes();
    data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
    let marker = profile.name.as_bytes();
    data[profile.version_marker..profile.version_marker + marker.len()].copy_from_slice(marker);
    for offset in EPOCH_CONSTANT_OFFSETS {
        data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
    }
    // The measured image stores its length minus four at 0xF4 (406,616 for the
    // 406,620-byte V1.13 file), so the fixture does too: a command that only
    // accepted `declared == len` would pass a synthetic test and reject a real
    // dump.
    data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4]
        .copy_from_slice(&(profile.size as u32 - 4).to_le_bytes());
    data
}

/// A one-block track-log dump with `points` records at the documented example
/// coordinate.
///
/// The records are the measured 16-byte layout (UTC + lat + lon + height + XOR
/// byte) under the V1.13 default bitmask.
#[must_use]
pub fn synthetic_dump(points: usize) -> Vec<u8> {
    use m241_core::log::block::{BLOCK_HEADER_SIZE, BLOCK_SIZE, POLICY_LOG_ENABLED, xor_checksum};

    // UTC + lat + lon + height: the measured V1.13 default mask.
    const MASK: u32 = 0x8000_001D;
    const LAT: f32 = 35.681_236;
    const LON: f32 = 139.767_13;
    const START: u32 = 1_700_000_000;

    let mut block = vec![0xFFu8; BLOCK_SIZE];
    block[0..2].copy_from_slice(&(points as u16).to_le_bytes());
    block[2..6].copy_from_slice(&MASK.to_le_bytes());
    block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
    block[8..12].copy_from_slice(&10u32.to_le_bytes());

    let mut cursor = BLOCK_HEADER_SIZE;
    for index in 0..points {
        let mut record = Vec::new();
        record.extend_from_slice(&(START + index as u32).to_le_bytes());
        record.extend_from_slice(&LAT.to_le_bytes());
        record.extend_from_slice(&LON.to_le_bytes());
        // 42 m as the three high bytes of an f32.
        record.extend_from_slice(&[0x00, 0xA0, 0x42]);
        let checksum = xor_checksum(&record);
        record.push(checksum);
        block[cursor..cursor + record.len()].copy_from_slice(&record);
        cursor += record.len();
    }
    block
}

/// A fake chip for the one command that hands the unit to a Download Agent.
///
/// `m241-flash`'s `FakeDa` is `#[cfg(test)]` inside that crate, so a test here
/// cannot reach it. This models only what `fw probe-da` uses: a BootROM that
/// syncs, a firmware that streams NMEA and honours `$PMTK180`, the `0xA1` /
/// `0xAF` / `0xA8` frames, the twenty-byte DA banner and the `0xD6` page
/// conversation.
///
/// It is not a protocol test double — `m241-flash` already has one, and this
/// crate must not become a second opinion on the wire format. Its purpose is to
/// run the *command's own code path* so a test can watch every opcode that goes
/// out, because the safety claim under test is "this command sends no write or
/// erase opcode". Anything else (`0xD3`, `0xD4`, `0xD5`, `0xD9`) is recorded and
/// answered with silence: enough for the record to fail the test.
pub mod fake_da {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use m241_core::Result;
    use m241_core::protocol::brom::{BASE_ADDR_DA, OPCODE_JUMP, OPCODE_READ32, OPCODE_WRITE16};
    use m241_core::protocol::frame::swap_pairs;
    use m241_core::protocol::port::Transport;
    use m241_flash::da::{ACK, NAK, OPCODE_READ_RANGE, byte_sum};

    use crate::link::{Cable, Links, Opened, PortEntry};

    use super::FAKE_PORT;

    /// The BootROM's four probes and their replies, as `m241-core` sends them.
    const BOOT_SYNC: [u8; 4] = [0xA0, 0x0A, 0x50, 0x05];
    const BOOT_REPLY: [u8; 4] = [0x5F, 0xF5, 0xAF, 0xFA];

    /// A live `$GPRMC` line, so the firmware looks like it is streaming.
    const NMEA_LINE: &[u8] =
        b"$GPRMC,000000.000,A,3540.8742,N,13946.0275,E,0.00,0.00,010120,,*00\r\n";

    /// Bytes of flash the fake models, in the DA's linear view.
    ///
    /// Large enough to hold both probe candidates with a page to spare;
    /// `PROBE_ADDRESSES[1]` is `0x0020_0000`, which is past the retail part's
    /// 512 KiB, so a window sized for the real chip would answer the second
    /// candidate with zeros.
    const VECTOR_WINDOW: usize = 0x0020_0000 + 0x1000;

    /// Which chip the fake is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        /// A running firmware: streams NMEA, answers BootROM frames with
        /// silence, and goes quiet into the BootROM when `$PMTK180` arrives.
        Firmware,
        /// The BootROM: nine-byte frames, every byte echoed except `0xA8`.
        BootRom,
        /// The download agent: the banner has been sent, `0xD6` is accepted.
        DownloadAgent,
    }

    /// Everything the fake saw, kept outside every transport instance so a test
    /// can read it after the command has closed the port.
    #[derive(Debug, Default)]
    pub struct Record {
        /// Every opcode received, in order. A BootROM frame contributes its
        /// first byte; a DA command (including the `0x5A` / `0xA5` of the page
        /// conversation) contributes one byte each.
        pub opcodes: Vec<u8>,
        /// Every `0xA1` frame as `(address, payload in memory order)`.
        pub writes: Vec<(u32, Vec<u8>)>,
        /// Every `0xA8` frame as `(address, count)`.
        pub jumps: Vec<(u32, u32)>,
        /// Every `0xD6` command as `(page length, start, total)`.
        pub reads: Vec<(u32, u32, u32)>,
        /// Whether `$PMTK180` arrived while the firmware streamed.
        pub pmtk180: bool,
        /// How many NMEA lines the fake put on the wire before `$PMTK180` and
        /// after it.
        ///
        /// A second `probe_stream` — what `enter_bootrom` does when the caller
        /// did not pass its state — pulls considerably more before the sentence
        /// and is what the real unit rejected.
        pub nmea_before: usize,
        /// Lines emitted after `$PMTK180`; on a successful switch this stays 0.
        pub nmea_after: usize,
        /// Whether any reset line was asserted (`set_control_lines(true)`).
        pub control_lines_asserted: bool,
        /// How many times a port was opened. A dry run must leave this at zero.
        pub opens: usize,
        /// Whether the serial line was ever read or written.
        pub bytes_moved: usize,
    }

    impl Record {
        /// The opcodes as hex, for an assertion message.
        #[must_use]
        pub fn opcodes_hex(&self) -> String {
            self.opcodes
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }

    /// The chip. Clones share one [`Record`] and one mode, so a view held by the
    /// test sees the whole conversation across the port being reopened.
    #[derive(Debug, Clone)]
    pub struct FakeChip {
        record: Rc<RefCell<Record>>,
        flash: Rc<Vec<u8>>,
        mode: Rc<RefCell<Mode>>,
    }

    impl FakeChip {
        /// A chip in `mode` whose flash window is `flash`, in the DA's linear
        /// view (the view `0xD6` addresses).
        #[must_use]
        pub fn new(mode: Mode, flash: Vec<u8>) -> Self {
            Self {
                record: Rc::new(RefCell::new(Record::default())),
                flash: Rc::new(flash),
                mode: Rc::new(RefCell::new(mode)),
            }
        }

        /// A BootROM chip whose flash holds the M-241 vector at `base` and is
        /// `0xFF` everywhere else.
        ///
        /// Pass [`m241_flash::da::PROBE_ADDRESSES`]`[1]` to put the image at the
        /// second candidate and `[0]` for the first.
        #[must_use]
        pub fn with_vector_at(base: u32) -> Self {
            Self::with_vector_head_at(base, &m241_flash::da::VECTOR)
        }

        /// A BootROM chip whose flash starts with `head` at `base` and is `0xFF`
        /// everywhere else.
        ///
        /// A caller passes something that shares only the vector's first byte
        /// (`[0x18, 0x00, 0x00, 0x00]`) to prove the verdict compares all four.
        #[must_use]
        pub fn with_vector_head_at(base: u32, head: &[u8]) -> Self {
            let mut flash = vec![0xFFu8; VECTOR_WINDOW];
            let offset = base as usize;
            if let Some(slot) = flash.get_mut(offset..offset.saturating_add(head.len())) {
                slot.copy_from_slice(head);
            }
            Self::new(Mode::BootRom, flash)
        }

        /// The record, for assertions.
        #[must_use]
        pub fn record(&self) -> Rc<RefCell<Record>> {
            Rc::clone(&self.record)
        }

        /// The mode the chip is in now.
        #[must_use]
        pub fn mode(&self) -> Mode {
            *self.mode.borrow()
        }
    }

    /// The transport one `open` hands out.
    #[derive(Debug)]
    pub struct FakeDaTransport {
        record: Rc<RefCell<Record>>,
        flash: Rc<Vec<u8>>,
        mode: Rc<RefCell<Mode>>,
        /// Bytes the host can read.
        output: VecDeque<u8>,
        /// A BootROM frame or a DA command being assembled.
        line: Vec<u8>,
        /// Whether the bytes of the frame being assembled are echoed.
        echo: bool,
        /// How many sync probes have been answered since the last complete sync.
        sync: usize,
        /// The `0xD6` range being served, when one is running.
        pages: Option<Pages>,
        /// Whether the current segment (since the last `clear_input`) has
        /// already been given its one NMEA sentence.
        served: bool,
    }

    /// A `0xD6` range being served one page at a time.
    #[derive(Debug)]
    struct Pages {
        page_len: usize,
        start: u32,
        total: usize,
        next: usize,
    }

    impl FakeDaTransport {
        fn new(chip: &FakeChip) -> Self {
            let mut transport = Self {
                record: Rc::clone(&chip.record),
                flash: Rc::clone(&chip.flash),
                mode: Rc::clone(&chip.mode),
                output: VecDeque::new(),
                line: Vec::new(),
                echo: true,
                sync: 0,
                pages: None,
                served: false,
            };
            if transport.mode() == Mode::Firmware {
                transport.queue(NMEA_LINE);
            }
            transport
        }

        fn mode(&self) -> Mode {
            *self.mode.borrow()
        }

        fn queue(&mut self, bytes: &[u8]) {
            self.output.extend(bytes.iter().copied());
        }

        fn consume(&mut self, byte: u8) {
            // A running firmware never runs dry: it emits a sentence a second,
            // and a probe that eavesdrops for a window must see one. The
            // sentence is queued by `read` when it finds the buffer empty, not
            // here: queueing on every byte would leave whole sentences in the
            // buffer at the moment `$PMTK180` switches the chip, and the entry's
            // observation window would read them as a live stream.
            if self.mode() == Mode::Firmware {
                self.consume_firmware(byte);
                return;
            }
            match self.mode() {
                Mode::Firmware => {}
                Mode::BootRom => self.consume_bootrom(byte),
                Mode::DownloadAgent => self.consume_da(byte),
            }
        }

        /// A running firmware: nothing answers but the stream, until
        /// `$PMTK180`.
        fn consume_firmware(&mut self, byte: u8) {
            if self.line.is_empty() && byte != b'$' {
                return;
            }
            self.line.push(byte);
            if byte != b'\n' {
                return;
            }
            let line = std::mem::take(&mut self.line);
            if line.starts_with(b"$PMTK180") {
                self.record.borrow_mut().pmtk180 = true;
                *self.mode.borrow_mut() = Mode::BootRom;
            }
        }

        fn consume_bootrom(&mut self, byte: u8) {
            // The four probes go out unframed and are answered in order.
            if self.line.is_empty() && byte == BOOT_SYNC[0] {
                self.sync = 1;
                self.queue(&BOOT_REPLY[..1]);
                return;
            }
            if self.sync > 0 {
                if BOOT_SYNC.get(self.sync).copied() == Some(byte) {
                    if let Some(reply) = BOOT_REPLY.get(self.sync) {
                        self.queue(&[*reply]);
                    }
                    self.sync += 1;
                }
                if self.sync >= BOOT_SYNC.len() {
                    self.sync = 0;
                }
                return;
            }
            if self.line.is_empty() {
                self.echo = byte != OPCODE_JUMP;
                self.record.borrow_mut().opcodes.push(byte);
            }
            if self.echo {
                self.queue(&[byte]);
            }
            self.line.push(byte);
            // `0xA1` is a header and then its payload; the count at 5–8 is in
            // half-words, so waiting for nine bytes only would swallow it.
            let needed =
                if self.line.len() >= 9 && self.line.first().copied() == Some(OPCODE_WRITE16) {
                    let units = be_u32(&self.line, 5).unwrap_or(0);
                    9usize.saturating_add(usize::try_from(units).unwrap_or(0).saturating_mul(2))
                } else {
                    9
                };
            if self.line.len() < needed {
                return;
            }
            let frame = std::mem::take(&mut self.line);
            self.echo = true;
            let Some(opcode) = frame.first().copied() else {
                return;
            };
            let address = be_u32(&frame, 1).unwrap_or(0);
            let count = be_u32(&frame, 5).unwrap_or(0);
            match opcode {
                OPCODE_WRITE16 => {
                    let payload = swap_pairs(frame.get(9..).unwrap_or_default());
                    self.record.borrow_mut().writes.push((address, payload));
                }
                OPCODE_READ32 => {
                    // `0xAF` counts dwords; each word is reversed on the wire.
                    let length = usize::try_from(count).unwrap_or(0).saturating_mul(4);
                    let mut memory = vec![0u8; length];
                    for (index, slot) in memory.iter_mut().enumerate() {
                        *slot = self
                            .ram_byte(address.saturating_add(u32::try_from(index).unwrap_or(0)));
                    }
                    let (words, tail) = memory.as_chunks_mut::<4>();
                    for word in words {
                        word.reverse();
                    }
                    let _ = tail;
                    self.queue(&memory);
                }
                OPCODE_JUMP => {
                    self.record.borrow_mut().jumps.push((address, count));
                    if address == BASE_ADDR_DA {
                        *self.mode.borrow_mut() = Mode::DownloadAgent;
                        self.queue(&banner());
                    }
                }
                _ => {}
            }
        }

        /// The byte an `0xAF` read of `address` returns: what a relayed `0xA1`
        /// wrote, or `0` where nothing did.
        ///
        /// Replaying the writes rather than keeping a RAM buffer makes the
        /// read-back a real check of the loader: a byte a `0xA1` did not carry
        /// reads back as zero and the loader's compare fails.
        fn ram_byte(&self, address: u32) -> u8 {
            let record = self.record.borrow();
            for (base, bytes) in record.writes.iter().rev() {
                let offset = address.wrapping_sub(*base) as usize;
                if let Some(byte) = bytes.get(offset) {
                    return *byte;
                }
            }
            0
        }

        fn consume_da(&mut self, byte: u8) {
            if let Some(page) = self.pages.as_mut() {
                // The advance (`0x5A`) and the retry (`0xA5`) belong to the page
                // conversation, not to a new command, but they are wire traffic.
                self.record.borrow_mut().opcodes.push(byte);
                if byte != ACK {
                    return;
                }
                page.next += 1;
                let pages = page.total.div_ceil(page.page_len.max(1));
                if page.next >= pages {
                    self.pages = None;
                    return;
                }
                let next = page.next;
                let (page_len, start) = (page.page_len, page.start);
                self.send_page(next, page_len, start);
                return;
            }
            if self.line.is_empty() {
                self.record.borrow_mut().opcodes.push(byte);
            }
            self.line.push(byte);
            if self.line.len() < 13 {
                return;
            }
            let frame = std::mem::take(&mut self.line);
            if frame.first().copied() != Some(OPCODE_READ_RANGE) {
                // Any other DA command is recorded above and not answered.
                return;
            }
            let page_len = be_u32(&frame, 1).unwrap_or(0);
            let start = be_u32(&frame, 5).unwrap_or(0);
            let total = be_u32(&frame, 9).unwrap_or(0);
            self.record
                .borrow_mut()
                .reads
                .push((page_len, start, total));
            let (Ok(page_len), Ok(total)) = (usize::try_from(page_len), usize::try_from(total))
            else {
                self.queue(&[NAK]);
                return;
            };
            if page_len == 0 || total == 0 {
                self.queue(&[NAK]);
                return;
            }
            self.pages = Some(Pages {
                page_len,
                start,
                total,
                next: 0,
            });
            self.send_page(0, page_len, start);
        }

        /// One page of the range, zero-padded to the page length, then its sum.
        fn send_page(&mut self, page: usize, page_len: usize, start: u32) {
            let first = u32::try_from(page.saturating_mul(page_len)).unwrap_or(u32::MAX);
            let mut payload = Vec::with_capacity(page_len);
            for index in 0..page_len {
                let at = start
                    .saturating_add(first)
                    .saturating_add(u32::try_from(index).unwrap_or(0));
                payload.push(self.flash.get(at as usize).copied().unwrap_or(0));
            }
            let mut packet = payload.clone();
            packet.extend_from_slice(&byte_sum(&payload).to_be_bytes());
            self.queue(&packet);
        }
    }

    /// The twenty-byte DA banner, `>BHBLHHHHL`
    /// (`docs/reference/01-da-protocol.md` §2): marker, version, region, total
    /// bytes, ids, two zeros, no external SRAM.
    fn banner() -> Vec<u8> {
        let mut report = vec![0xC0u8];
        report.extend_from_slice(&0x0308u16.to_be_bytes());
        report.push(0x49);
        report.extend_from_slice(&0x0008_0000u32.to_be_bytes());
        report.extend_from_slice(&0x00C2u16.to_be_bytes());
        report.extend_from_slice(&0x22BAu16.to_be_bytes());
        report.extend_from_slice(&0x0000u16.to_be_bytes());
        report.extend_from_slice(&0x0000u16.to_be_bytes());
        report.extend_from_slice(&0x0000_0000u32.to_be_bytes());
        report
    }

    fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
        bytes
            .get(at..at.checked_add(4)?)
            .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
            .map(u32::from_be_bytes)
    }

    impl Transport for FakeDaTransport {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let mut count = 0usize;
            while count < buf.len() {
                if self.output.is_empty() && self.mode() == Mode::Firmware {
                    // A running firmware emits a sentence about once a second,
                    // and a probe that eavesdrops for a window must see one. It
                    // is queued here, when a read finds the buffer empty, so the
                    // buffer holds no leftovers at the moment `$PMTK180` switches
                    // the chip.
                    //
                    // Before the sentence, each `clear_input` starts a new
                    // segment and each segment gets exactly one sentence: the
                    // probe reads the one after the port is opened, the drain
                    // reads the one after the first clear. A second probe is not
                    // preceded by a clear, so it finds its segment already spent,
                    // sees silence, and the caller refuses. That is the failure
                    // the split entry avoids, and it is what makes a double probe
                    // visible in a test.
                    let mut refill = false;
                    {
                        let mut record = self.record.borrow_mut();
                        if record.pmtk180 {
                            record.nmea_after += 1;
                            refill = true;
                        } else if !self.served {
                            self.served = true;
                            record.nmea_before += 1;
                            refill = true;
                        }
                    }
                    if refill {
                        self.queue(NMEA_LINE);
                    }
                }
                let Some(byte) = self.output.pop_front() else {
                    break;
                };
                buf[count] = byte;
                count += 1;
            }
            self.record.borrow_mut().bytes_moved += count;
            Ok(count)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.record.borrow_mut().bytes_moved += buf.len();
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
            // A new segment: the drain that follows gets one sentence of its
            // own, so a quiet line during the drain is not mistaken for a dead
            // firmware.
            self.served = false;
            // A half-assembled line belongs to the traffic being dropped: keeping
            // it would prefix the next sentence with stale bytes, and a
            // `$PMTK180` that arrived as `...$PMTK180` would not be recognised.
            self.line.clear();
            Ok(())
        }

        fn set_baud(&mut self, _baud: u32) -> Result<()> {
            Ok(())
        }

        fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
            // The probe reaches the BootROM through the stream or through
            // silence, never through the reset lines (`AGENTS.md` §2.1); a test
            // asserts this stays false.
            self.record.borrow_mut().control_lines_asserted |= asserted;
            Ok(())
        }
    }

    /// A [`Links`] with one plausible port backed by a [`FakeChip`].
    #[derive(Debug, Clone)]
    pub struct FakeDaPorts {
        chip: FakeChip,
    }

    impl FakeDaPorts {
        /// One plausible port backed by `chip`.
        #[must_use]
        pub fn new(chip: FakeChip) -> Self {
            Self { chip }
        }

        /// The record, for assertions after the command returns.
        #[must_use]
        pub fn record(&self) -> Rc<RefCell<Record>> {
            self.chip.record()
        }
    }

    impl Links for FakeDaPorts {
        fn ports(&self) -> Result<Vec<PortEntry>> {
            Ok(vec![PortEntry {
                name: String::from(FAKE_PORT),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: Some(String::from("simulated M-241")),
            }])
        }

        fn open(&self, _name: &str, baud: u32) -> Result<Opened> {
            self.chip.record().borrow_mut().opens += 1;
            let mut transport = FakeDaTransport::new(&self.chip);
            transport.set_baud(baud)?;
            Ok(Opened::new(Box::new(transport)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The fake assembles a line from wire bytes, so a `clear_input` that
        /// kept what was assembled would carry a fragment into the next
        /// sentence.
        ///
        /// The failure is a `$PMTK180` that arrives as `...$PMTK180` on the
        /// wire: the firmware branch ignores bytes before a `$`, but only when
        /// nothing is assembled, so the fragment sticks to the front of the
        /// sentence and it is not recognised. Real hardware drops received bytes
        /// on `reset_input_buffer`, and a fake that kept them would be stricter
        /// than the unit in a way nothing else in the suite would notice.
        #[test]
        fn clear_input_drops_a_half_assembled_line() {
            let chip = FakeChip::new(Mode::Firmware, vec![0u8; 16]);
            let mut transport = FakeDaTransport::new(&chip);

            // A partial line left over from before the hand-over.
            for byte in b"$GPRMC,0000" {
                transport.consume(*byte);
            }
            transport.clear_input().expect("clear");

            for byte in b"$PMTK180*3B\r\n" {
                transport.consume(*byte);
            }
            assert!(
                chip.record().borrow().pmtk180,
                "the stale fragment hid the sentence"
            );
        }
    }
}
