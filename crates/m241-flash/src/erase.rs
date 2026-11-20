//! Erasing the track log: the one PMTK command that destroys data.
//!
//! `$PMTK182,6` wipes the log on the device. It is a one-sentence command with
//! an acknowledgement, so it looks harmless next to `$PMTK182,5` and
//! `$PMTK182,4` — but those stop and restart recording, and this ends the log.
//! It lives in this crate rather than beside the read path for exactly that
//! reason: a caller that only meant to download a log must not be able to reach
//! it.
//!
//! # What the caller owes
//!
//! This module cannot know whether the bytes on the device are the only copy.
//! [`LogEraser::erase`] therefore takes a [`BackupProof`], which can only be
//! produced by [`BackupProof::of_bytes`] — that is, by handing over the dump
//! that was taken first. The check is deliberately weak (it compares lengths,
//! not contents: the dump is the caller's data and this crate has no business
//! re-parsing it), but its purpose is not to be strong. It is to make "erase
//! without a copy" a line of code a reviewer sees rather than a function
//! argument nobody reads.
//!
//! # Wire facts this module depends on
//!
//! The framing is `m241_core::protocol::pmtk`'s: `$<body>*<XOR>*\r\n`, and a
//! sentence the firmware cannot parse is dropped **without a reply**, so a
//! timeout here usually means the sentence never arrived intact. That is why the
//! refusal names the sentence, not the device.

use std::time::{Duration, Instant};

use m241_core::Result;
use m241_core::error::Error;
use m241_core::protocol::pmtk::{self, MAX_LINE_BYTES};
use m241_core::protocol::port::Transport;

/// The subcommand that erases the log.
pub const ERASE_SUBCOMMAND: u8 = 6;

/// How long to wait for the acknowledgement of `$PMTK182,6`.
///
/// The same five seconds the read path uses for a direct reply: the erase is
/// acknowledged as soon as the command is accepted, not when the flash has
/// finished, so a longer wait would only make a dead cable slower to report.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The command number PMTK uses for the log family.
const PMTK_LOG_COMMAND: u16 = 182;

/// Evidence that the log was copied off the device before this module is asked
/// to destroy it.
///
/// Construct one with [`BackupProof::of_bytes`]. There is no other constructor,
/// and there is deliberately no `Default`: a proof has to carry the bytes that
/// were saved.
#[derive(Debug, Clone, Copy)]
pub struct BackupProof {
    /// How many bytes the saved copy holds.
    saved_bytes: usize,
}

impl BackupProof {
    /// Record that `dump` — the log as it was read off the device — is on disk.
    ///
    /// This constructor is infallible and records `dump.len()` verbatim,
    /// including `0` for an empty slice. [`LogEraser::erase`] refuses a proof
    /// whose [`Self::saved_bytes`] is `0` with [`Error::Safety`] before
    /// sending anything on the wire, so an empty or un-taken backup cannot
    /// trigger `$PMTK182,6`.
    #[must_use]
    pub fn of_bytes(dump: &[u8]) -> Self {
        Self {
            saved_bytes: dump.len(),
        }
    }

    /// How many bytes the proof says were saved.
    #[must_use]
    pub fn saved_bytes(self) -> usize {
        self.saved_bytes
    }
}

/// Erases the track log on a device whose firmware is running.
///
/// Owns the transport for the same reason [`m241_core::protocol::PmtkClient`]
/// does: the two clients are used one after the other on one open port, and
/// handing the transport back with [`LogEraser::into_transport`] keeps the
/// ownership explicit instead of shared.
#[derive(Debug)]
pub struct LogEraser<T: Transport> {
    transport: T,
    reply_timeout: Duration,
}

impl<T: Transport> LogEraser<T> {
    /// Wrap an open transport that is already at the NMEA baud rate.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
        }
    }

    /// Override how long the acknowledgement may take.
    #[must_use]
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Take the transport back, for example to hand it to the read path.
    #[must_use]
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Erase the log (`$PMTK182,6`). **Destroys the log on the device.**
    ///
    /// `proof` must describe a copy that was taken first; see [`BackupProof`].
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when the proof carries no bytes at all, [`Error::Nmea`]
    /// when the acknowledgement is malformed or fails its checksum,
    /// [`Error::Rejected`] when the device answers but refuses, and
    /// [`Error::Timeout`] when it does not answer at all.
    pub fn erase(&mut self, proof: BackupProof) -> Result<()> {
        if proof.saved_bytes == 0 {
            return Err(Error::Safety {
                reason: String::from(
                    "refusing to erase a log that was not copied off the device first",
                ),
            });
        }
        let body = format!("PMTK182,{ERASE_SUBCOMMAND}");
        // Wait for the first `$PMTK001,` reply and verify both the command
        // number (182) and the subcommand (6). Checking only `command == 182`
        // would accept a stale `$PMTK001,182,5,3` (log-stop ACK) as if the
        // erase had succeeded.
        let reply = self.await_reply(&body, "$PMTK001,")?;
        let sentence = pmtk::parse_sentence(&reply)?;
        let ack = pmtk::parse_ack(&sentence)?;
        if ack.command != PMTK_LOG_COMMAND {
            return Err(Error::Framing {
                what: format!(
                    "acknowledgement was for command {}, not {PMTK_LOG_COMMAND}",
                    ack.command
                ),
                offset: None,
            });
        }
        let subcommand = (sentence.fields.len() == 3)
            .then(|| sentence.field(1).and_then(|field| field.parse::<u8>().ok()))
            .flatten();
        if subcommand != Some(ERASE_SUBCOMMAND) {
            return Err(Error::Framing {
                what: format!(
                    "acknowledgement was for $PMTK182 subcommand {:?}, not {ERASE_SUBCOMMAND} ({reply})",
                    sentence.field(1)
                ),
                offset: None,
            });
        }
        if !ack.is_ok() {
            return Err(Error::Rejected {
                what: format!("$PMTK182,{ERASE_SUBCOMMAND} (status {})", ack.status),
            });
        }
        Ok(())
    }

    /// Send one sentence and wait for the first reply that starts with `prefix`.
    fn await_reply(&mut self, body: &str, prefix: &str) -> Result<String> {
        // Stale bytes from the previous command must not be mistaken for this
        // one's reply; the trait's `clear_input` exists for exactly this.
        self.transport.clear_input()?;
        let bytes = pmtk::line(body);
        self.transport.write_all(bytes.as_bytes())?;
        self.transport.flush()?;
        let deadline = Instant::now() + self.reply_timeout;
        match read_line_until(&mut self.transport, deadline, prefix)? {
            Some(line) => Ok(line),
            None => Err(Error::Timeout {
                what: format!("$PMTK001,182,{ERASE_SUBCOMMAND} acknowledgement (sent ${body})"),
                timeout: self.reply_timeout.into(),
            }),
        }
    }
}

/// Read bytes until one line is complete and non-blank, or the deadline passes.
///
/// A duplicate of the reader inside [`m241_core::protocol::pmtk`] would normally
/// be a defect, but that one is private to the read path on purpose: sharing it
/// would put this crate's command framing back inside the crate that must stay
/// free of it. The two parsers see the same wire and are pinned to the same
/// behaviour by their own tests.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport that answers with a queued line and records what was written.
    #[derive(Debug, Default)]
    struct Stub {
        reply: Vec<u8>,
        cursor: usize,
        written: Vec<u8>,
    }

    impl Stub {
        fn answering(text: &str) -> Self {
            let mut reply = Vec::new();
            reply.extend_from_slice(pmtk::line(text).as_bytes());
            Self {
                reply,
                cursor: 0,
                written: Vec::new(),
            }
        }
    }

    impl Transport for Stub {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let left = self.reply.len().saturating_sub(self.cursor);
            if left == 0 || buf.is_empty() {
                return Ok(0);
            }
            let take = left.min(buf.len());
            buf[..take].copy_from_slice(&self.reply[self.cursor..self.cursor + take]);
            self.cursor += take;
            Ok(take)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.written.extend_from_slice(buf);
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
        fn clear_input(&mut self) -> Result<()> {
            self.cursor = 0;
            Ok(())
        }
        fn set_baud(&mut self, _baud: u32) -> Result<()> {
            Ok(())
        }
        fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
            Ok(())
        }
    }

    fn dump() -> Vec<u8> {
        vec![0x42; 0x900]
    }

    #[test]
    fn the_erase_sentence_is_the_measured_one() {
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,6,3"));
        eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect("accepted");
        assert_eq!(
            eraser.into_transport().written,
            b"$PMTK182,6*23\r\n",
            "the erase command and its XOR checksum"
        );
    }

    #[test]
    fn an_accepted_erase_returns_unit() {
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,6,3"));
        assert!(
            eraser.erase(BackupProof::of_bytes(&dump())).is_ok(),
            "status 3 is acceptance"
        );
    }

    #[test]
    fn a_refused_erase_is_rejected_not_silently_ignored() {
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,6,0"));
        let err = eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect_err("status 0 is a refusal");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(
            err.is_fatal(),
            "a refused write leaves the device undefined"
        );
    }

    #[test]
    fn an_empty_backup_is_refused_before_anything_is_sent() {
        // The check must run before the sentence goes out: sending it and then
        // refusing would still be a destructive command on the wire.
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,6,3"));
        let err = eraser
            .erase(BackupProof::of_bytes(&[]))
            .expect_err("no copy was taken");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            eraser.into_transport().written.is_empty(),
            "the erase sentence must not be sent without a backup"
        );
    }

    #[test]
    fn an_acknowledgement_for_another_command_is_a_framing_error() {
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,5,3"))
            .with_reply_timeout(Duration::from_millis(30));
        let err = eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect_err("wrong command");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
    }

    #[test]
    fn a_log_stop_acknowledgement_is_not_accepted_as_erase_success() {
        // `$PMTK001,182,5,3` acknowledges `$PMTK182,5` (logging stop), not
        // `$PMTK182,6` (erase). Accepting any `command == 182` ACK would treat
        // a stale stop reply as erase success.
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,5,3"))
            .with_reply_timeout(Duration::from_millis(30));
        let err = eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect_err("subcommand 5 is log stop, not erase");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(err.is_fatal(), "a mismatched ACK must not be retried");
    }

    #[test]
    fn a_pmtk182_acknowledgement_without_a_subcommand_is_a_framing_error() {
        let mut eraser = LogEraser::new(Stub::answering("PMTK001,182,3"))
            .with_reply_timeout(Duration::from_millis(30));
        let err = eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect_err("missing subcommand");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
    }

    #[test]
    fn silence_is_a_timeout_that_names_the_sentence() {
        let mut eraser =
            LogEraser::new(Stub::default()).with_reply_timeout(Duration::from_millis(30));
        let err = eraser
            .erase(BackupProof::of_bytes(&dump()))
            .expect_err("no reply");
        let text = format!("{err}");
        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(text.contains("PMTK182,6"), "got {text}");
    }

    #[test]
    fn a_backup_proof_carries_the_byte_count_it_was_given() {
        let proof = BackupProof::of_bytes(&dump());
        assert_eq!(proof.saved_bytes(), dump().len());
        assert_eq!(BackupProof::of_bytes(&[]).saved_bytes(), 0);
    }
}
