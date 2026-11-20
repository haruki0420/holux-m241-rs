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
    Timeout { what: String, timeout: Duration },
    /// The bytes coming back are not what the protocol says they must be.
    Framing { what: String, offset: Offset },
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
    Io {
        path: String,
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
                write!(f, "framing error: {what} (at 0x{offset:X})")
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
            Self::Io { path, source } => write!(f, "i/o on {path}: {source}"),
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
            Self::Framing { what, .. } => Self::Framing { what, offset },
            other => other,
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

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: String::from("<unbound>"),
            source,
        }
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

/// A duration whose `Debug` prints `500ms`, not `Duration { secs: 0, nanos: … }`.
///
/// [`Error::Timeout`]'s message embeds this, and a raw `Duration` debug in a
/// user-facing message is worse than useless.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(std::time::Duration);

impl Duration {
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

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.subsec_millis() == 0 {
            write!(f, "{}s", self.0.as_secs())
        } else {
            write!(f, "{}ms", self.0.as_millis())
        }
    }
}

impl From<std::time::Duration> for Duration {
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
                offset: 3
            }
            .hint()
            .is_some()
        );
    }

    #[test]
    fn an_io_error_keeps_its_source_chain() {
        let err = Error::Io {
            path: String::from("backup/stock.bin"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert!(std::error::Error::source(&err).is_some());
        assert!(format!("{err}").contains("backup/stock.bin"));
    }
}
