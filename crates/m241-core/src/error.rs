//! The error hierarchy, and the reason it is one flat enum.
//!
//! Callers fall into two groups: code that retries or refuses ([`Error::is_fatal`])
//! and code that reports to a person (`Display` plus [`Error::hint`]). Both work
//! on one enum so a `?` never has to choose a wrapper, and so the CLI can map an
//! error onto a documented exit code without matching on strings.
//!
//! Variants that can point at a byte offset carry one. That is the whole reason
//! this type exists instead of `Box<dyn Error>`: a firmware patch failure is only
//! actionable if it says `0x402F4`.
//!
//! `Display` is written by hand rather than derived so that the offset renders
//! the same way everywhere — `0x402F4`, uppercase, no padding — and so that an
//! error without an offset simply has no parenthetical rather than a placeholder
//! nobody removed.

use std::fmt;

/// Byte position inside a firmware image or a log dump.
pub type Offset = usize;

/// Everything this crate can fail with.
#[derive(Debug)]
pub enum Error {
    /// The serial port could not be opened, or was not found.
    Port { message: String },
    /// The chip did not answer a command in time.
    Timeout {
        what: String,
        timeout: TimeoutDuration,
    },
    /// The bytes coming back are not what the protocol says they must be.
    ///
    /// `offset` is `None` when the failure has no position in an image: an echo
    /// that does not match or a sync probe answered with the wrong byte is a
    /// desynchronised line, not a bad byte at a known place. Printing a made-up
    /// `0x0` there would send the reader looking at the start of a file.
    Framing {
        what: String,
        offset: Option<Offset>,
    },
    /// The chip refused a command with its own negative acknowledgement.
    Rejected { what: String },
    /// A destructive action was refused because a safety condition was not met.
    Safety { reason: String },
    /// The image cannot be parsed.
    Firmware {
        message: String,
        offset: Option<Offset>,
    },
    /// The image is structurally fine but is not a version this build knows.
    Profile {
        message: String,
        offset: Option<Offset>,
    },
    /// A track-log dump could not be parsed.
    Log {
        message: String,
        offset: Option<Offset>,
    },
    /// An NMEA sentence is malformed.
    Nmea { message: String },
    /// The caller asked for something the current state does not allow.
    State(String),
    /// A file could not be read or written.
    ///
    /// `path` is the file the caller was working on. It is `None` when the error
    /// came from a bare [`std::io::Error`], which carries no path: the serial
    /// port, for instance, is mapped from `serialport::Error` and its own errors
    /// go through [`Error::Port`]. Display then reads `i/o: …` rather than
    /// inventing a placeholder path.
    Io {
        path: Option<String>,
        source: std::io::Error,
    },
}

/// `message (at 0x402F4)`, or just `message` when there is no offset.
fn with_offset(message: &str, offset: Option<Offset>) -> String {
    match offset {
        Some(offset) => format!("{message} (at 0x{offset:X})"),
        None => message.to_owned(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Port { message } => write!(f, "serial port: {message}"),
            Self::Timeout { what, timeout } => {
                write!(f, "no reply from the device within {timeout:?}: {what}")
            }
            Self::Framing { what, offset } => {
                write!(f, "framing error: {}", with_offset(what, *offset))
            }
            Self::Rejected { what } => write!(f, "the device rejected {what}"),
            Self::Safety { reason } => write!(f, "refused: {reason}"),
            Self::Firmware { message, offset } => {
                write!(f, "firmware image: {}", with_offset(message, *offset))
            }
            Self::Profile { message, offset } => {
                write!(
                    f,
                    "unknown firmware profile: {}",
                    with_offset(message, *offset)
                )
            }
            Self::Log { message, offset } => {
                write!(f, "track log: {}", with_offset(message, *offset))
            }
            Self::Nmea { message } => write!(f, "NMEA sentence: {message}"),
            Self::State(message) => f.write_str(message),
            Self::Io { path, source } => match path {
                Some(path) => write!(f, "i/o on {path}: {source}"),
                None => write!(f, "i/o: {source}"),
            },
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Error {
    /// Attach a byte offset to an error that can carry one.
    ///
    /// Errors without an offset field ignore this, which keeps call sites
    /// simple: `.at(offset)` is always safe to write.
    #[must_use]
    pub fn at(self, offset: Offset) -> Self {
        match self {
            Self::Firmware { message, .. } => Self::Firmware {
                message,
                offset: Some(offset),
            },
            Self::Profile { message, .. } => Self::Profile {
                message,
                offset: Some(offset),
            },
            Self::Log { message, .. } => Self::Log {
                message,
                offset: Some(offset),
            },
            Self::Framing { what, .. } => Self::Framing {
                what,
                offset: Some(offset),
            },
            other => other,
        }
    }

    /// Append a sentence about a second failure, keeping this error's variant.
    ///
    /// Used where one failure must not hide another: a lost `$PMTK182,5`
    /// acknowledgement and a failed `$PMTK182,4` resume both matter, and the
    /// first one still decides the exit code and whether a retry is safe.
    ///
    /// # Examples
    ///
    /// ```
    /// use m241_core::Error;
    ///
    /// let stop = Error::Rejected {
    ///     what: String::from("$PMTK182,5"),
    /// };
    /// let both = stop.with_note("logging could not be resumed: no reply");
    /// assert!(matches!(both, Error::Rejected { .. }));
    /// assert!(both.to_string().contains("$PMTK182,5"));
    /// assert!(both.to_string().contains("could not be resumed"));
    /// ```
    #[must_use]
    pub fn with_note(self, note: impl fmt::Display) -> Self {
        let note = note.to_string();
        match self {
            Self::Port { message } => Self::Port {
                message: format!("{message}; {note}"),
            },
            Self::Timeout { what, timeout } => Self::Timeout {
                what: format!("{what}; {note}"),
                timeout,
            },
            Self::Framing { what, offset } => Self::Framing {
                what: format!("{what}; {note}"),
                offset,
            },
            Self::Rejected { what } => Self::Rejected {
                what: format!("{what}; {note}"),
            },
            Self::Safety { reason } => Self::Safety {
                reason: format!("{reason}; {note}"),
            },
            Self::Firmware { message, offset } => Self::Firmware {
                message: format!("{message}; {note}"),
                offset,
            },
            Self::Profile { message, offset } => Self::Profile {
                message: format!("{message}; {note}"),
                offset,
            },
            Self::Log { message, offset } => Self::Log {
                message: format!("{message}; {note}"),
                offset,
            },
            Self::Nmea { message } => Self::Nmea {
                message: format!("{message}; {note}"),
            },
            Self::State(message) => Self::State(format!("{message}; {note}")),
            Self::Io { path, source } => Self::Io {
                path,
                // `kind()` survives: appending to the message must not turn
                // "not found" into a generic `Other`.
                source: std::io::Error::new(source.kind(), format!("{source}; {note}")),
            },
        }
    }

    /// A suggested next action, in the same voice as the interface's hint line.
    ///
    /// This is not a `Display` concern: the message names the problem, the hint
    /// names the recovery, and the interface puts them in different places.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Port { .. } => Some("ケーブルを挿し直し、`m241 device ports` で確認してください"),
            Self::Timeout { .. } => {
                Some("本体を電源から外して 10 秒待ち、入れ直してから再試行してください")
            }
            Self::Framing { .. } => {
                Some("USB ケーブルを短いものに替えるか、電源の安定した USB ポートを使ってください")
            }
            Self::Rejected { .. } => {
                Some("本体を再起動し、書き込みに失敗したなら自分のバックアップを書き戻してください")
            }
            Self::Safety { .. } => None,
            Self::Firmware { .. } => Some("自分の個体から吸い出したイメージか確認してください"),
            Self::Profile { .. } => Some("`m241 fw patch --dry-run` で検出結果を確認してください"),
            Self::Log { .. } => Some("`m241 log info` でブロックヘッダを確認してください"),
            Self::Nmea { .. } => Some("入力の行末が CRLF か確認してください"),
            Self::State(_) => None,
            Self::Io { .. } => Some("書き込み先の権限と空き容量を確認してください"),
        }
    }

    /// Should the caller stop rather than retry?
    ///
    /// A timeout is retryable. A framing error usually is not: the line is
    /// desynchronised and retrying makes it worse. A rejected write is fatal
    /// because the device state is now undefined. A safety refusal is fatal
    /// because retrying it unchanged would ask for the same dangerous action
    /// again.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Framing { .. }
                | Self::Rejected { .. }
                | Self::Safety { .. }
                | Self::Firmware { .. }
        )
    }
}

impl Error {
    /// Wrap an I/O error with the file it happened on.
    ///
    /// Prefer this over [`Error::from`] whenever the path is known: `i/o on
    /// backup/stock.bin: permission denied` names the file to fix, and a bare
    /// `i/o: permission denied` does not.
    #[must_use]
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: Some(path.into()),
            source,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        // A bare `io::Error` carries no path. Guessing one (`<unbound>`) put a
        // placeholder in front of the user; `None` lets Display print the OS's
        // wording alone.
        Self::Io { path: None, source }
    }
}

impl From<serialport::Error> for Error {
    fn from(source: serialport::Error) -> Self {
        // The OS's own wording is kept: "permission denied" and "no such file"
        // need different fixes, and paraphrasing loses that distinction.
        Self::Port {
            message: source.to_string(),
        }
    }
}

/// A timeout whose `Debug` prints `500ms`, not `Duration { secs: 0, nanos: … }`.
///
/// Named `TimeoutDuration` rather than `Duration` so it cannot be mistaken for
/// [`std::time::Duration`] at a use site that has both in scope.
///
/// [`Error::Timeout`]'s message embeds this, and a raw `Duration` debug in a
/// user-facing message is worse than useless.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeoutDuration(std::time::Duration);

impl TimeoutDuration {
    /// Wrap a standard duration.
    #[must_use]
    pub const fn from_std(value: std::time::Duration) -> Self {
        Self(value)
    }

    /// Unwrap to the standard duration.
    #[must_use]
    pub const fn as_std(self) -> std::time::Duration {
        self.0
    }
}

impl fmt::Debug for TimeoutDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.subsec_millis() == 0 {
            write!(f, "{}s", self.0.as_secs())
        } else {
            write!(f, "{}ms", self.0.as_millis())
        }
    }
}

impl From<std::time::Duration> for TimeoutDuration {
    fn from(value: std::time::Duration) -> Self {
        Self(value)
    }
}

/// The crate's result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeout_wrapper_does_not_shadow_the_standard_duration() {
        // Both names are in scope here, which is exactly the ambiguity the old
        // `Duration` name created: `std::time::Duration` had to be written out
        // in full at every use site.
        let std_timeout = std::time::Duration::from_millis(500);
        let wrapped = TimeoutDuration::from(std_timeout);
        assert_eq!(wrapped.as_std(), std_timeout);
        assert_eq!(TimeoutDuration::from_std(std_timeout), wrapped);
    }

    #[test]
    fn an_offset_reaches_the_display_string_in_hex() {
        let err = Error::Firmware {
            message: String::from("bad epoch constant"),
            offset: None,
        }
        .at(0x402F4);
        let text = format!("{err}");
        assert!(text.contains("0x402F4"), "got {text}");
        assert!(text.contains("bad epoch constant"), "got {text}");
    }

    #[test]
    fn an_error_without_an_offset_prints_no_parenthetical() {
        let err = Error::Firmware {
            message: String::from("too short"),
            offset: None,
        };
        assert!(!format!("{err}").contains('('), "got {err}");
    }

    #[test]
    fn at_is_a_no_op_where_the_variant_has_no_offset() {
        let err = Error::Timeout {
            what: String::from("sync"),
            timeout: std::time::Duration::from_millis(500).into(),
        }
        .at(12);
        assert!(matches!(err, Error::Timeout { .. }));
    }

    #[test]
    fn fatal_covers_the_writes_that_leave_the_device_undefined() {
        assert!(
            Error::Rejected {
                what: String::from("packet")
            }
            .is_fatal()
        );
        assert!(
            !Error::Timeout {
                what: String::from("sync"),
                timeout: std::time::Duration::from_secs(5).into()
            }
            .is_fatal()
        );
    }

    #[test]
    fn timeout_debug_does_not_print_nanoseconds() {
        let err = Error::Timeout {
            what: String::from("sync"),
            timeout: std::time::Duration::from_millis(500).into(),
        };
        let text = format!("{err}");
        assert!(text.contains("500ms"), "got {text}");
        assert!(!text.contains("nanos"), "got {text}");
    }

    #[test]
    fn whole_second_timeouts_print_as_seconds() {
        let err = Error::Timeout {
            what: String::from("sync"),
            timeout: std::time::Duration::from_secs(5).into(),
        };
        assert!(format!("{err}").contains("5s"), "got {err}");
    }

    #[test]
    fn every_error_carries_a_hint_except_state() {
        assert!(Error::State(String::from("not connected")).hint().is_none());
        assert!(
            Error::Framing {
                what: String::from("echo"),
                offset: Some(3)
            }
            .hint()
            .is_some()
        );
    }

    #[test]
    fn a_note_keeps_the_variant_and_names_both_failures() {
        let stop = Error::Timeout {
            what: String::from("$PMTK001,182,5 acknowledgement"),
            timeout: std::time::Duration::from_millis(500).into(),
        };
        let both = stop.with_note("logging could not be resumed: no reply");
        assert!(
            matches!(both, Error::Timeout { .. }),
            "the first failure's variant decides the exit code: got {both:?}"
        );
        let text = format!("{both}");
        assert!(text.contains("$PMTK001,182,5"), "got {text}");
        assert!(text.contains("could not be resumed"), "got {text}");

        // `kind()` must survive the rewrite: a not-found file is still not
        // found, even with a note.
        let io = Error::io(
            "backup/stock.bin",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        )
        .with_note("the destructive step was not attempted");
        match io {
            Error::Io { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
                assert!(
                    source
                        .to_string()
                        .contains("the destructive step was not attempted")
                );
            }
            other => panic!("expected Error::Io, got {other}"),
        }
    }

    #[test]
    fn a_framing_error_without_a_position_prints_no_coordinates() {
        // An echo mismatch has no offset into an image; it must not pretend to
        // be at 0x0, which would send the reader to the start of a file.
        let err = Error::Framing {
            what: String::from("echo was [0xA2], expected [0xA1]"),
            offset: None,
        };
        let text = format!("{err}");
        assert!(!text.contains("0x0"), "got {text}");
        assert!(!text.contains("at "), "got {text}");
        assert!(text.contains("echo was"), "got {text}");
    }

    #[test]
    fn a_framing_error_that_names_an_offset_still_prints_it() {
        let err = Error::Framing {
            what: String::from("data does not match the block header"),
            offset: Some(0x1F4),
        };
        assert!(format!("{err}").contains("at 0x1F4"), "got {err}");
    }

    #[test]
    fn an_io_error_keeps_its_source_chain() {
        let err = Error::io(
            "backup/stock.bin",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(std::error::Error::source(&err).is_some());
        assert!(format!("{err}").contains("backup/stock.bin"));
    }

    #[test]
    fn a_bare_io_error_prints_no_placeholder_path() {
        // `Error::from(io::Error)` has no path to name. It used to print
        // "i/o on <unbound>: …", which reads as a real file name.
        let err = Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let text = format!("{err}");
        assert!(!text.contains("<unbound>"), "got {text}");
        assert!(!text.contains("i/o on "), "got {text}");
        assert!(text.starts_with("i/o: "), "got {text}");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn an_io_error_with_a_path_names_the_file() {
        let err = Error::io(
            "backup/stock.bin",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(
            format!("{err}").starts_with("i/o on backup/stock.bin: "),
            "got {err}"
        );
    }
}
