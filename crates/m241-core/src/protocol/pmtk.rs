//! PMTK sentences: the firmware's own command channel, carried on the NMEA port.
//!
//! While the M-241 runs its firmware it streams NMEA at 38400 8N1 and accepts
//! `$PMTK` sentences on the same wire. This module owns three things:
//!
//! * the framing — `$<body>*<two hex digits>\r\n`, where the checksum is the XOR
//!   of every byte between `$` and `*`;
//! * the two sentences that identify a unit and hand it to the BootROM;
//! * the `$PMTK182` log-transfer family.
//!
//! It deliberately does **not** own the transport: every method works through
//! [`Transport`], so the simulator can replay the same code path in tests.
//!
//! # Why each constant is what it is
//!
//! The checksum and the CRLF terminator are not decoration. The firmware parses
//! a command only once it sees a line terminator, and it drops a sentence whose
//! checksum is wrong **without replying** (measured; see
//! `docs/dev/03-hardware-facts.md`). A timeout here therefore usually means the
//! sentence never arrived intact, not that the device is broken, which is why
//! the errors name the sentence that was sent.
//!
//! # Log transfer, and the one rule that matters
//!
//! `$PMTK182,7,<addr>,<len>` carries both numbers as **hexadecimal without a
//! `0x` prefix**: `$PMTK182,7,64,64` reads from address 0x64, not 64. Logging
//! must also be stopped for the transfer and restarted afterwards — a failure
//! that leaves the unit not recording is worse than the failure itself, so
//! [`PmtkClient::download_log`] always resumes logging, even when the transfer
//! fails.

use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::protocol::brom::DEFAULT_BOOTROM_BAUD;
use crate::protocol::port::Transport;
use crate::types::DeviceInfo;

/// Baud rate the firmware speaks while it is running (measured, 8N1).
pub const NMEA_BAUD: u32 = 38_400;

/// Largest `$PMTK182,7` chunk the device accepted during bring-up.
///
/// Measured: a 200 KB transfer at this size is about 100 round trips. A larger
/// request is not known to work, so [`PmtkClient::log_read`] refuses one rather
/// than discovering the limit on a user's unit.
pub const LOG_CHUNK_BYTES: usize = 0x800;

/// Longest NMEA line this client will buffer before dropping the fragment.
///
/// A `$PMTK705` reply is well under 100 bytes, but a `$PMTK182,8` log reply
/// carries its chunk as hexadecimal and is therefore twice the chunk size, plus
/// the sentence around it. The cap exists so a desynchronised line cannot grow
/// without bound; it must not be small enough to throw away a legitimate reply.
pub const MAX_LINE_BYTES: usize = LOG_CHUNK_BYTES * 2 + 64;

/// Status byte the firmware puts in `$PMTK001` when it accepted a command.
///
/// Measured: `$PMTK001,182,5,3` acknowledges "logging stopped". A non-zero,
/// non-3 value is a refusal.
pub const ACK_STATUS_OK: u8 = 3;

/// How long to watch the wire when asked "is NMEA streaming?".
///
/// The firmware emits one sentence per second, so a window slightly longer than
/// a second distinguishes "streaming" from "silent" without a guess.
pub const DEFAULT_STREAM_WINDOW: Duration = Duration::from_millis(1500);

/// How long to wait for a direct reply to one command.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The XOR checksum of a sentence body (the text between `$` and `*`).
#[must_use]
pub fn checksum(body: &str) -> u8 {
    body.as_bytes().iter().fold(0u8, |acc, byte| acc ^ byte)
}

/// `$<body>*<checksum>`, without the line terminator.
#[must_use]
pub fn sentence(body: &str) -> String {
    format!("${body}*{:02X}", checksum(body))
}

/// `$<body>*<checksum>\r\n`, the exact bytes the firmware needs.
#[must_use]
pub fn line(body: &str) -> String {
    let mut text = sentence(body);
    text.push_str("\r\n");
    text
}

/// One parsed NMEA sentence whose checksum has been verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    /// The address field: `PMTK705`, `GPRMC`, `GPGGA`, …
    pub kind: String,
    /// The comma-separated fields after the address, in order.
    pub fields: Vec<String>,
}

impl Sentence {
    /// The field at `index`, or `None` when the sentence is shorter.
    #[must_use]
    pub fn field(&self, index: usize) -> Option<&str> {
        self.fields.get(index).map(String::as_str)
    }
}

/// Parse `$<body>*HH` and verify its checksum.
///
/// # Errors
///
/// [`Error::Nmea`] when the text is not an NMEA sentence, has no checksum, or
/// carries one that does not match. A reply with a broken checksum is treated
/// as absent on purpose: acting on it would be acting on corrupted bytes.
pub fn parse_sentence(text: &str) -> Result<Sentence> {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('$') else {
        return Err(Error::Nmea {
            message: format!("not an NMEA sentence: {trimmed:?}"),
        });
    };
    let Some((body, checksum_text)) = rest.split_once('*') else {
        return Err(Error::Nmea {
            message: format!("sentence has no checksum: {trimmed:?}"),
        });
    };
    let carried = parse_hex_u8(checksum_text).ok_or_else(|| Error::Nmea {
        message: format!("checksum is not two hex digits: {trimmed:?}"),
    })?;
    let computed = checksum(body);
    if carried != computed {
        return Err(Error::Nmea {
            message: format!(
                "checksum mismatch (carried {carried:02X}, computed {computed:02X}) in {trimmed:?}"
            ),
        });
    }

    let mut parts = body.split(',');
    let Some(kind) = parts.next() else {
        return Err(Error::Nmea {
            message: format!("sentence has an empty address: {trimmed:?}"),
        });
    };
    if kind.is_empty() {
        return Err(Error::Nmea {
            message: format!("sentence has an empty address: {trimmed:?}"),
        });
    }
    Ok(Sentence {
        kind: kind.to_owned(),
        fields: parts.map(str::to_owned).collect(),
    })
}

/// Decode the `$PMTK705` reply into a [`DeviceInfo`].
///
/// Measured reply: `$PMTK705,B-core_1.20,0043,01017-00F,1.0*72`, i.e. release,
/// build, model and an optional SDK version.
///
/// # Errors
///
/// [`Error::Nmea`] when the sentence is not `$PMTK705` or is missing a field.
pub fn parse_device_info(sentence: &Sentence) -> Result<DeviceInfo> {
    if sentence.kind != "PMTK705" {
        return Err(Error::Nmea {
            message: format!("expected $PMTK705, got ${}", sentence.kind),
        });
    }
    let required = |index: usize, name: &str| -> Result<String> {
        match sentence.field(index) {
            Some(value) if !value.is_empty() => Ok(value.to_owned()),
            _ => Err(Error::Nmea {
                message: format!("$PMTK705 is missing its {name} field"),
            }),
        }
    };
    Ok(DeviceInfo {
        release: required(0, "release")?,
        build: required(1, "build")?,
        model: required(2, "model")?,
        // Older builds omit the SDK field entirely; an empty one is the same
        // information, so it is reported as absent rather than as an empty name.
        sdk: sentence
            .field(3)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    })
}

/// Decode the `$PMTK182,3,8,<hex bytes used>` reply.
///
/// # Errors
///
/// [`Error::Nmea`] when the sentence is a different PMTK182 reply or the byte
/// count is not hexadecimal.
pub fn parse_log_usage(sentence: &Sentence) -> Result<usize> {
    if sentence.kind != "PMTK182" {
        return Err(Error::Nmea {
            message: format!("expected $PMTK182, got ${}", sentence.kind),
        });
    }
    if sentence.field(0) != Some("3") || sentence.field(1) != Some("8") {
        return Err(Error::Nmea {
            message: format!(
                "expected a $PMTK182,3,8 reply, got ${},{}",
                sentence.kind,
                sentence.field(0).unwrap_or("")
            ),
        });
    }
    let Some(text) = sentence.field(2) else {
        return Err(Error::Nmea {
            message: String::from("$PMTK182,3,8 carries no byte count"),
        });
    };
    parse_hex_usize(text).ok_or_else(|| Error::Nmea {
        message: format!("log usage {text:?} is not hexadecimal"),
    })
}

/// Decode `$PMTK182,8,<addr>,<hex data>` into bytes, checking the address.
///
/// The address check matters: a chunk that belongs to a different offset would
/// splice the log together in the wrong order, and the result would look like a
/// plausible track rather than an error.
///
/// # Errors
///
/// [`Error::Nmea`] for a different reply kind, a non-hex address or payload, or
/// an address that is not the one requested.
pub fn parse_log_chunk(sentence: &Sentence, address: usize) -> Result<Vec<u8>> {
    if sentence.kind != "PMTK182" {
        return Err(Error::Nmea {
            message: format!("expected $PMTK182, got ${}", sentence.kind),
        });
    }
    if sentence.field(0) != Some("8") {
        return Err(Error::Nmea {
            message: String::from("expected a $PMTK182,8 reply"),
        });
    }
    let Some(address_text) = sentence.field(1) else {
        return Err(Error::Nmea {
            message: String::from("$PMTK182,8 carries no address"),
        });
    };
    let Some(returned) = parse_hex_usize(address_text) else {
        return Err(Error::Nmea {
            message: format!("log reply address {address_text:?} is not hexadecimal"),
        });
    };
    if returned != address {
        return Err(Error::Framing {
            what: format!("log reply is for address 0x{returned:X}, requested 0x{address:X}"),
            offset: Some(returned),
        });
    }
    let Some(payload) = sentence.field(2) else {
        return Err(Error::Nmea {
            message: String::from("$PMTK182,8 carries no payload"),
        });
    };
    decode_hex(payload)
}

/// The `$PMTK001` acknowledgement of one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    /// The command number being acknowledged, e.g. `182`.
    pub command: u16,
    /// The status: [`ACK_STATUS_OK`] when the device accepted the command.
    pub status: u8,
}

impl Ack {
    /// Whether the device accepted the command.
    #[must_use]
    pub fn is_ok(self) -> bool {
        self.status == ACK_STATUS_OK
    }
}

/// Decode a `$PMTK001,<command>,…,<status>` acknowledgement.
///
/// # Errors
///
/// [`Error::Nmea`] when the sentence is not a PMTK acknowledgement or either
/// number is missing.
pub fn parse_ack(sentence: &Sentence) -> Result<Ack> {
    if sentence.kind != "PMTK001" {
        return Err(Error::Nmea {
            message: format!("expected $PMTK001, got ${}", sentence.kind),
        });
    }
    let Some(command_text) = sentence.field(0) else {
        return Err(Error::Nmea {
            message: String::from("$PMTK001 carries no command number"),
        });
    };
    let Some(command) = command_text.parse::<u16>().ok() else {
        return Err(Error::Nmea {
            message: format!("$PMTK001 command {command_text:?} is not a number"),
        });
    };
    let Some(status_text) = sentence.fields.last().map(String::as_str) else {
        return Err(Error::Nmea {
            message: String::from("$PMTK001 carries no status"),
        });
    };
    let Some(status) = status_text.parse::<u8>().ok() else {
        return Err(Error::Nmea {
            message: format!("$PMTK001 status {status_text:?} is not a number"),
        });
    };
    Ok(Ack { command, status })
}

/// Whether the firmware was seen talking, which decides how the BootROM is
/// entered.
///
/// This tri-state is kept on purpose: `$PMTK180` is honoured only while the
/// NMEA stream is live, and the reset-line route is only safe when the stream is
/// definitively absent. Collapsing "cannot tell" into either answer is what lets
/// a caller do the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// At least one `$` sentence arrived inside the window.
    Streaming,
    /// Nothing arrived at all.
    Silent,
    /// Bytes arrived, but never a complete sentence — the line is not usable.
    Unknown,
}

/// A client for the firmware's PMTK channel.
///
/// Construct it over a transport that is already open at [`NMEA_BAUD`]. The
/// transport is owned, so a caller that has finished with PMTK code can take it
/// back with [`PmtkClient::into_transport`] and hand it to the BootROM client.
#[derive(Debug)]
pub struct PmtkClient<T: Transport> {
    transport: T,
    reply_timeout: Duration,
    stream_window: Duration,
}

impl<T: Transport> PmtkClient<T> {
    /// Wrap an open transport.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            stream_window: DEFAULT_STREAM_WINDOW,
        }
    }

    /// Override how long a direct reply may take.
    #[must_use]
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Override how long the stream probe watches the wire.
    #[must_use]
    pub fn with_stream_window(mut self, window: Duration) -> Self {
        self.stream_window = window;
        self
    }

    /// Borrow the transport (tests inspect what was written through this).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Take the transport back, for example to hand it to [`crate::protocol::MtkBootRom`].
    #[must_use]
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Watch the wire for one NMEA sentence and report what was seen.
    ///
    /// # Errors
    ///
    /// Whatever the transport reports. Silence is not an error: it is
    /// [`StreamState::Silent`].
    pub fn probe_stream(&mut self) -> Result<StreamState> {
        let deadline = Instant::now() + self.stream_window;
        let mut saw_partial = false;
        while let Some(text) = read_line(&mut self.transport, deadline)? {
            if text.starts_with('$') {
                return Ok(StreamState::Streaming);
            }
            saw_partial = true;
        }
        Ok(if saw_partial {
            StreamState::Unknown
        } else {
            StreamState::Silent
        })
    }

    /// Ask the unit who it is: `$PMTK605` → `$PMTK705`.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] when no `$PMTK705` arrives, [`Error::Nmea`] when it is
    /// malformed or fails its checksum.
    pub fn request_identity(&mut self) -> Result<DeviceInfo> {
        let reply = self.await_reply(
            "PMTK605",
            "$PMTK705",
            self.reply_timeout,
            "$PMTK705 identity reply",
        )?;
        parse_device_info(&parse_sentence(&reply)?)
    }

    /// Hand the unit over to the BootROM with `$PMTK180*3B`.
    ///
    /// The sentence is honoured only while NMEA is still streaming (measured), so
    /// the stream is probed first and the refusal names that condition instead of
    /// leaving a bare sync timeout. This never touches the reset lines: pulsing
    /// them at a running firmware leaves the unit unresponsive until the battery
    /// is removed.
    ///
    /// On success the transport is left at [`DEFAULT_BOOTROM_BAUD`], where the
    /// four-byte BootROM sync will be answered.
    ///
    /// # Errors
    ///
    /// [`Error::State`] when no live stream is present, when the stream cannot
    /// be classified, or when the unit keeps streaming after the sentence.
    pub fn enter_bootrom(&mut self) -> Result<()> {
        match self.probe_stream()? {
            StreamState::Streaming => {}
            StreamState::Silent => {
                return Err(Error::State(String::from(
                    "no NMEA stream before $PMTK180: the firmware is not running, so the \
                     sentence would be ignored — power-cycle the logger and retry",
                )));
            }
            StreamState::Unknown => {
                return Err(Error::State(String::from(
                    "cannot tell whether the firmware is streaming NMEA; refusing to send \
                     $PMTK180 blind — power-cycle the logger and retry",
                )));
            }
        }

        self.send("PMTK180")?;
        // Anything already queued belongs to the old mode; keeping it would make
        // the "did the stream stop?" test read a sentence that predates the
        // switch.
        self.transport.clear_input()?;
        let deadline = Instant::now() + self.stream_window;
        if read_line(&mut self.transport, deadline)?.is_some() {
            return Err(Error::State(String::from(
                "the logger kept streaming NMEA after $PMTK180: the sentence was not honoured",
            )));
        }
        self.transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
        Ok(())
    }

    /// Bytes the device reports as used by the log (`$PMTK182,2,8`).
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] when there is no reply, [`Error::Nmea`] when it cannot
    /// be decoded.
    pub fn log_used_bytes(&mut self) -> Result<usize> {
        let reply = self.await_reply(
            "PMTK182,2,8",
            "$PMTK182,3,8",
            self.reply_timeout,
            "$PMTK182,3,8 log usage reply",
        )?;
        parse_log_usage(&parse_sentence(&reply)?)
    }

    /// Whether the device is currently recording (`$PMTK182,2,1`).
    ///
    /// # Errors
    ///
    /// As [`Self::log_used_bytes`].
    pub fn log_enabled(&mut self) -> Result<bool> {
        let reply = self.await_reply(
            "PMTK182,2,1",
            "$PMTK182,3,1",
            self.reply_timeout,
            "$PMTK182,3,1 logging-state reply",
        )?;
        let sentence = parse_sentence(&reply)?;
        let Some(value) = sentence.field(2) else {
            return Err(Error::Nmea {
                message: String::from("$PMTK182,3,1 carries no logging state"),
            });
        };
        match value.trim() {
            "0" => Ok(false),
            "" => Err(Error::Nmea {
                message: String::from("$PMTK182,3,1 carries an empty logging state"),
            }),
            _ => Ok(true),
        }
    }

    /// Stop recording before a transfer (`$PMTK182,5`).
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when the device answers but refuses, [`Error::Timeout`]
    /// when it does not answer at all.
    pub fn log_stop(&mut self) -> Result<()> {
        self.log_control("PMTK182,5", 5)
    }

    /// Resume recording after a transfer (`$PMTK182,4`).
    ///
    /// # Errors
    ///
    /// As [`Self::log_stop`].
    pub fn log_resume(&mut self) -> Result<()> {
        self.log_control("PMTK182,4", 4)
    }

    fn log_control(&mut self, body: &str, subcommand: u8) -> Result<()> {
        let reply = self.await_reply(
            body,
            "$PMTK001,182,",
            self.reply_timeout,
            &format!("$PMTK001,182,{subcommand} acknowledgement"),
        )?;
        let ack = parse_ack(&parse_sentence(&reply)?)?;
        if ack.command != 182 {
            return Err(Error::Framing {
                what: format!("acknowledgement was for command {}, not 182", ack.command),
                offset: None,
            });
        }
        if !ack.is_ok() {
            return Err(Error::Rejected {
                what: format!("$PMTK182,{subcommand} (status {})", ack.status),
            });
        }
        Ok(())
    }

    /// Read one chunk of the log (`$PMTK182,7`).
    ///
    /// `address` and `length` are sent as hexadecimal without a `0x` prefix — the
    /// device's own convention, and the single most expensive detail to get
    /// wrong, because a decimal interpretation reads the wrong region silently.
    ///
    /// # Errors
    ///
    /// [`Error::State`] when `length` is larger than [`LOG_CHUNK_BYTES`],
    /// [`Error::Timeout`] when there is no reply, [`Error::Framing`] when the
    /// reply is for another address.
    pub fn log_read(&mut self, address: usize, length: usize) -> Result<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        if length > LOG_CHUNK_BYTES {
            return Err(Error::State(format!(
                "asked for a {length}-byte log chunk; the device accepts at most \
                 {LOG_CHUNK_BYTES} bytes per $PMTK182,7"
            )));
        }
        let body = format!("PMTK182,7,{address:X},{length:X}");
        let reply = self.await_reply(
            &body,
            "$PMTK182,8",
            self.reply_timeout,
            &format!("$PMTK182,8 reply for 0x{address:X}"),
        )?;
        parse_log_chunk(&parse_sentence(&reply)?, address)
    }

    /// Download the whole log, pausing and resuming recording around it.
    ///
    /// `progress` is called with `(received, total)` after every chunk, so a
    /// caller can show a bar without knowing the chunking.
    ///
    /// Logging is always resumed, even when the transfer fails: a unit left with
    /// recording off loses the next trip, which is worse than a failed download.
    ///
    /// # Errors
    ///
    /// Whatever the transfer failed with. A failure to resume is reported only
    /// when the transfer itself succeeded, so the original cause is never
    /// masked.
    pub fn download_log(&mut self, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<u8>> {
        let total = self.log_used_bytes()?;
        if total == 0 {
            progress(0, 0);
            return Ok(Vec::new());
        }
        self.log_stop()?;
        let outcome = self.transfer(total, progress);
        let resumed = self.log_resume();
        match outcome {
            Err(err) => Err(err),
            Ok(data) => {
                resumed?;
                Ok(data)
            }
        }
    }

    fn transfer(
        &mut self,
        total: usize,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Vec<u8>> {
        let mut collected: Vec<u8> = Vec::with_capacity(total);
        while collected.len() < total {
            let want = LOG_CHUNK_BYTES.min(total - collected.len());
            let address = collected.len();
            let chunk = self.log_read(address, want)?;
            if chunk.is_empty() {
                return Err(Error::Framing {
                    what: format!(
                        "the device returned no data at 0x{address:X} ({} of {total} bytes)",
                        collected.len()
                    ),
                    offset: Some(address),
                });
            }
            if chunk.len() > want {
                return Err(Error::Framing {
                    what: format!(
                        "the device returned {} bytes for a {want}-byte request at 0x{address:X}",
                        chunk.len()
                    ),
                    offset: Some(address),
                });
            }
            collected.extend_from_slice(&chunk);
            progress(collected.len(), total);
        }
        Ok(collected)
    }

    /// Erase the log on the device (`$PMTK182,6`). **Destroys the log.**
    ///
    /// The caller is responsible for having a copy: this module cannot know
    /// whether the bytes on the device are the only ones.
    ///
    /// # Errors
    ///
    /// As [`Self::log_stop`].
    pub fn erase_log(&mut self) -> Result<()> {
        let reply = self.await_reply(
            "PMTK182,6",
            "$PMTK001,182,6",
            self.reply_timeout,
            "$PMTK001,182,6 erase acknowledgement",
        )?;
        let ack = parse_ack(&parse_sentence(&reply)?)?;
        if !ack.is_ok() {
            return Err(Error::Rejected {
                what: format!("$PMTK182,6 (status {})", ack.status),
            });
        }
        Ok(())
    }

    /// Send one sentence and wait for the first reply that starts with `prefix`.
    ///
    /// Named `await_reply` rather than `expect` so that a reader scanning for the
    /// crate's forbidden `Option::expect` does not have to stop and check.
    fn await_reply(
        &mut self,
        body: &str,
        prefix: &str,
        timeout: Duration,
        what: &str,
    ) -> Result<String> {
        // Stale bytes from the previous command must not be mistaken for this
        // one's reply; the trait's `clear_input` exists for exactly this.
        self.transport.clear_input()?;
        self.send(body)?;
        let deadline = Instant::now() + timeout;
        match read_line_until(&mut self.transport, deadline, prefix)? {
            Some(reply) => Ok(reply),
            None => Err(Error::Timeout {
                what: format!("{what} (sent ${body})"),
                timeout: timeout.into(),
            }),
        }
    }

    fn send(&mut self, body: &str) -> Result<()> {
        let bytes = line(body);
        self.transport.write_all(bytes.as_bytes())?;
        self.transport.flush()
    }
}

/// Read bytes until one line is complete and non-blank, or the deadline passes.
fn read_line<T: Transport>(transport: &mut T, deadline: Instant) -> Result<Option<String>> {
    let mut pending: Vec<u8> = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        let mut byte = [0u8; 1];
        if transport.read(&mut byte)? == 0 {
            continue;
        }
        let Some(value) = byte.first().copied() else {
            continue;
        };
        if value == b'\n' {
            let text = String::from_utf8_lossy(&pending).trim().to_owned();
            pending.clear();
            if text.is_empty() {
                continue;
            }
            return Ok(Some(text));
        }
        if value == b'\r' {
            continue;
        }
        if pending.len() >= MAX_LINE_BYTES {
            // A line that never terminates cannot be a reply we understand;
            // dropping the fragment resynchronises on the next `$`.
            pending.clear();
            continue;
        }
        pending.push(value);
    }
}

/// Read lines until one starts with `prefix`, or the deadline passes.
fn read_line_until<T: Transport>(
    transport: &mut T,
    deadline: Instant,
    prefix: &str,
) -> Result<Option<String>> {
    while let Some(text) = read_line(transport, deadline)? {
        if text.starts_with(prefix) {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// Decode `text` as an even number of hexadecimal digits.
fn decode_hex(text: &str) -> Result<Vec<u8>> {
    let bytes = text.trim().as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::Nmea {
            message: format!("hex payload has an odd number of digits: {text:?}"),
        });
    }
    let (pairs, _tail) = bytes.as_chunks::<2>();
    pairs
        .iter()
        .map(|pair| {
            let Some(high) = pair.first().copied().and_then(hex_nibble) else {
                return Err(Error::Nmea {
                    message: format!("invalid hex payload: {text:?}"),
                });
            };
            let Some(low) = pair.get(1).copied().and_then(hex_nibble) else {
                return Err(Error::Nmea {
                    message: format!("invalid hex payload: {text:?}"),
                });
            };
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_u8(text: &str) -> Option<u8> {
    let bytes = text.as_bytes();
    if bytes.len() != 2 {
        return None;
    }
    let high = bytes.first().copied().and_then(hex_nibble)?;
    let low = bytes.get(1).copied().and_then(hex_nibble)?;
    Some((high << 4) | low)
}

fn parse_hex_usize(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    usize::from_str_radix(trimmed, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity our fake device reports; the strings are the ones measured
    /// on a retail unit.
    fn measured_device() -> DeviceInfo {
        DeviceInfo {
            release: String::from("B-core_1.20"),
            build: String::from("0043"),
            model: String::from("01017-00F"),
            sdk: Some(String::from("1.0")),
        }
    }

    #[test]
    fn the_measured_commands_carry_the_checksums_the_hardware_notes_record() {
        assert_eq!(sentence("PMTK605"), "$PMTK605*31");
        assert_eq!(sentence("PMTK180"), "$PMTK180*3B");
        assert_eq!(sentence("PMTK182,2,8"), "$PMTK182,2,8*33");
    }

    #[test]
    fn a_sentence_ends_with_crlf_because_the_firmware_needs_the_terminator() {
        assert_eq!(line("PMTK605"), "$PMTK605*31\r\n");
        assert_eq!(
            line("PMTK180").as_bytes(),
            crate::protocol::brom::PMTK180_COMMAND
        );
    }

    #[test]
    fn the_measured_705_reply_parses_into_device_info() {
        let reply = format!(
            "${}*{:02X}",
            "PMTK705,B-core_1.20,0043,01017-00F,1.0",
            checksum("PMTK705,B-core_1.20,0043,01017-00F,1.0")
        );
        let sentence = parse_sentence(&reply).expect("valid sentence");
        let info = parse_device_info(&sentence).expect("four fields");
        assert_eq!(info, measured_device());
        assert!(info.is_known_release());
    }

    #[test]
    fn a_705_reply_without_a_checksum_is_refused() {
        let err =
            parse_sentence("$PMTK705,B-core_1.20,0043,01017-00F,1.0").expect_err("no checksum");
        assert!(matches!(err, Error::Nmea { .. }));
    }

    #[test]
    fn a_705_reply_with_a_wrong_checksum_is_refused() {
        let err = parse_sentence("$PMTK705,B-core_1.20,0043,01017-00F,1.0*00")
            .expect_err("wrong checksum");
        let text = format!("{err}");
        assert!(text.contains("checksum mismatch"), "got {text}");
    }

    #[test]
    fn a_705_reply_missing_the_model_is_refused() {
        let body = "PMTK705,B-core_1.20,0043";
        let reply = sentence(body);
        let sentence = parse_sentence(&reply).expect("valid sentence");
        let err = parse_device_info(&sentence).expect_err("no model field");
        assert!(format!("{err}").contains("model"), "got {err}");
    }

    #[test]
    fn the_sdk_field_is_optional() {
        let body = "PMTK705,B-core_1.20,0043,01017-00F";
        let sentence = parse_sentence(&sentence(body)).expect("valid sentence");
        let info = parse_device_info(&sentence).expect("three fields are enough");
        assert_eq!(info.sdk, None);
    }

    #[test]
    fn the_usage_reply_is_hexadecimal() {
        let sentence = parse_sentence(&sentence("PMTK182,3,8,00033F0C")).expect("valid sentence");
        assert_eq!(parse_log_usage(&sentence).expect("hex count"), 0x33F0C);
    }

    #[test]
    fn a_usage_reply_that_is_not_hexadecimal_is_refused() {
        let sentence = parse_sentence(&sentence("PMTK182,3,8,ZZ")).expect("valid sentence");
        assert!(parse_log_usage(&sentence).is_err());
    }

    #[test]
    fn a_log_chunk_reply_is_decoded_against_the_requested_address() {
        let sentence = parse_sentence(&sentence("PMTK182,8,800,DEADBEEF")).expect("valid sentence");
        let data = parse_log_chunk(&sentence, 0x800).expect("matching address");
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn a_log_chunk_from_another_address_is_a_framing_error() {
        let sentence = parse_sentence(&sentence("PMTK182,8,800,DEADBEEF")).expect("valid sentence");
        let err = parse_log_chunk(&sentence, 0x1000).expect_err("wrong address");
        assert!(matches!(err, Error::Framing { .. }));
    }

    #[test]
    fn an_odd_number_of_hex_digits_is_refused() {
        let sentence = parse_sentence(&sentence("PMTK182,8,0,ABC")).expect("valid sentence");
        assert!(parse_log_chunk(&sentence, 0).is_err());
    }

    #[test]
    fn an_acknowledgement_reads_its_command_and_status() {
        let sentence = parse_sentence(&sentence("PMTK001,182,5,3")).expect("valid sentence");
        let ack = parse_ack(&sentence).expect("acknowledgement");
        assert_eq!(ack.command, 182);
        assert!(ack.is_ok());
    }

    #[test]
    fn a_refused_acknowledgement_is_not_ok() {
        let sentence = parse_sentence(&sentence("PMTK001,182,5,0")).expect("valid sentence");
        let ack = parse_ack(&sentence).expect("acknowledgement");
        assert!(!ack.is_ok());
    }

    #[test]
    fn a_read_sentence_carries_hex_address_and_length_without_a_prefix() {
        // The exact bytes are what a network analyser saw on hardware; the test
        // exists because a decimal interpretation is silently wrong instead of
        // failing.
        assert_eq!(
            line("PMTK182,7,800,800"),
            "$PMTK182,7,800,800*22\r\n",
            "address and length are hexadecimal, no 0x"
        );
    }
}
