//! The documented exit-code table, and the functions that produce it.
//!
//! `docs/reference/02-cli.md` promises codes 0–6. Success and every runtime
//! failure land here; code 2 is clap's judgement about argument parsing and is
//! asked for through [`exit_code_for_parse_error`]. The mapping is deliberately
//! exhaustive — no `_` arm — so adding a variant to [`m241_core::Error`] fails
//! to compile here instead of silently becoming a runtime error and costing a
//! documented code its producer.
//!
//! [`CliError`] separates a flash-write failure (exit code 6, where the device
//! state is undefined and the user must re-flash their backup) from any other
//! command refusal such as a rejected `$PMTK182` request (exit code 1).

use std::fmt;

/// The error type returned by [`crate::run`].
///
/// Wraps [`m241_core::Error`] so that exit code 6 stays reserved exclusively for
/// flash-write failures (`docs/reference/02-cli.md`), while a device rejecting
/// a read-only or log command still exits with code 1.
#[derive(Debug)]
pub enum CliError {
    /// Any error from the core library or CLI checks outside a flash write.
    Core(m241_core::Error),
    /// A flash write failed once erasing or programming had started; the device
    /// state is undefined and the user must re-flash their backup.
    FlashWriteFailed(m241_core::Error),
}

impl CliError {
    /// A suggested next action for the user, if one applies.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::FlashWriteFailed(_) => {
                Some("本体を再起動し、書き込みに失敗したなら自分のバックアップを書き戻してください")
            }
            Self::Core(m241_core::Error::Rejected { .. }) => {
                Some("本体の状態を確認し、電源を入れ直してから再試行してください")
            }
            Self::Core(err) => err.hint(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(err) => fmt::Display::fmt(err, f),
            Self::FlashWriteFailed(err) => write!(f, "flash write failed: {err}"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(err) | Self::FlashWriteFailed(err) => Some(err),
        }
    }
}

impl From<m241_core::Error> for CliError {
    fn from(err: m241_core::Error) -> Self {
        Self::Core(err)
    }
}

/// The documented exit code for a whole run.
///
/// Success lives in the same table as the failures so that the tests can pin
/// every code from 0 to 6 in one place. Code 2 is not here: it belongs to
/// argument parsing, which happens before `run` is ever called.
#[must_use]
pub fn exit_code_of(result: &Result<(), CliError>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(err) => exit_code_for_cli(err),
    }
}

/// Map a [`CliError`] onto the documented exit codes.
///
/// 0 ok / 1 runtime / 2 usage / 3 hardware / 4 verify / 5 safety / 6 flash.
#[must_use]
pub fn exit_code_for_cli(err: &CliError) -> u8 {
    match err {
        CliError::FlashWriteFailed(_) => 6,
        CliError::Core(inner) => exit_code_for(inner),
    }
}

/// 0 for `--help` / `--version`, 2 for a usage error.
///
/// Delegates the judgement to clap, which already knows which of its errors it
/// would print to stdout, but owns the number here so the documented table has
/// a producer in this crate rather than in a dependency's default.
#[must_use]
pub fn exit_code_for_parse_error(err: &clap::Error) -> u8 {
    if err.use_stderr() { 2 } else { 0 }
}

/// Map a core error onto the documented exit codes.
///
/// 0 ok / 1 runtime / 2 usage / 3 hardware / 4 verify / 5 safety (6 belongs to
/// [`CliError::FlashWriteFailed`]).
///
/// Deliberately exhaustive: no `_` arm, so adding a variant to [`m241_core::Error`]
/// fails to compile here instead of silently becoming a runtime error and
/// costing a documented code its producer.
#[must_use]
pub fn exit_code_for(err: &m241_core::Error) -> u8 {
    use m241_core::Error;
    match err {
        Error::Port { .. } | Error::Timeout { .. } => 3,
        Error::Framing { .. } | Error::Firmware { .. } | Error::Profile { .. } => 4,
        Error::Safety { .. } => 5,
        Error::Rejected { .. }
        | Error::Log { .. }
        | Error::Nmea { .. }
        | Error::State(_)
        | Error::Io { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One error per variant: the table the docs promise, produced for real.
    ///
    /// The `Error` enum is matched without a `_` arm in [`exit_code_for`], so a
    /// new variant breaks this test's sibling at compile time. This test is the
    /// runtime half: it checks each variant lands on the documented code.
    fn errors_and_their_codes() -> Vec<(CliError, u8)> {
        vec![
            (
                CliError::Core(m241_core::Error::Port {
                    message: String::from("no such file"),
                }),
                3,
            ),
            (
                CliError::Core(m241_core::Error::Timeout {
                    what: String::from("sync"),
                    timeout: std::time::Duration::from_millis(500).into(),
                }),
                3,
            ),
            (
                CliError::Core(m241_core::Error::Framing {
                    what: String::from("echo"),
                    offset: Some(3),
                }),
                4,
            ),
            (
                CliError::Core(m241_core::Error::Firmware {
                    message: String::from("too short"),
                    offset: None,
                }),
                4,
            ),
            (
                CliError::Core(m241_core::Error::Profile {
                    message: String::from("unknown size"),
                    offset: None,
                }),
                4,
            ),
            (
                CliError::Core(m241_core::Error::Safety {
                    reason: String::from("no backup"),
                }),
                5,
            ),
            (
                CliError::Core(m241_core::Error::Rejected {
                    what: String::from("$PMTK182,5 (status 2)"),
                }),
                1,
            ),
            (
                CliError::FlashWriteFailed(m241_core::Error::Rejected {
                    what: String::from("packet 12"),
                }),
                6,
            ),
            (
                CliError::Core(m241_core::Error::Log {
                    message: String::from("bad block"),
                    offset: None,
                }),
                1,
            ),
            (
                CliError::Core(m241_core::Error::Nmea {
                    message: String::from("bad checksum"),
                }),
                1,
            ),
            (
                CliError::Core(m241_core::Error::State(String::from("not connected"))),
                1,
            ),
            (
                // Built through `From<io::Error>` on purpose: the `Io` variant's
                // `path` field is being reworked in the core work stream, and
                // the conversion is the part of its shape that is stable.
                CliError::Core(m241_core::Error::from(std::io::Error::from(
                    std::io::ErrorKind::NotFound,
                ))),
                1,
            ),
        ]
    }

    #[test]
    fn every_documented_exit_code_has_a_producer() {
        // 0: a command that succeeds.
        assert_eq!(exit_code_of(&Ok(())), 0);

        // 1..=6: one variant on each documented row, through both entry points.
        let mut produced = vec![0u8];
        for (err, expected) in errors_and_their_codes() {
            assert_eq!(
                exit_code_for_cli(&err),
                expected,
                "wrong code for {err:?}: {err}"
            );
            assert_eq!(
                exit_code_of(&Err(err)),
                expected,
                "exit_code_of lost the code"
            );
            produced.push(expected);
        }
        produced.sort_unstable();
        produced.dedup();
        assert_eq!(
            produced,
            vec![0, 1, 3, 4, 5, 6],
            "2 is clap's, tested in `cli`"
        );
    }

    #[test]
    fn a_safety_refusal_is_exit_code_five() {
        let err = m241_core::Error::Safety {
            reason: String::from("destructive action without a backup"),
        };
        assert_eq!(exit_code_for(&err), 5);
        assert!(err.is_fatal());
        assert!(err.hint().is_none());
        assert!(format!("{err}").starts_with("refused: "));
    }

    #[test]
    fn a_non_flash_device_rejection_is_exit_code_one_not_six() {
        let core = m241_core::Error::Rejected {
            what: String::from("$PMTK182,5 (status 2)"),
        };
        assert_eq!(exit_code_for(&core), 1);
        let cli = CliError::from(core);
        assert_eq!(exit_code_for_cli(&cli), 1);
        let hint = cli.hint().unwrap_or("");
        assert!(
            !hint.contains("バックアップ"),
            "a non-flash rejection must not tell the user to re-flash: {hint}"
        );

        let flash = CliError::FlashWriteFailed(m241_core::Error::Rejected {
            what: String::from("packet 12"),
        });
        assert_eq!(exit_code_for_cli(&flash), 6);
        assert!(
            flash.hint().unwrap_or("").contains("バックアップ"),
            "a flash-write failure must point the user to their backup"
        );
    }
}
