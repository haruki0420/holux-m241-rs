//! The documented exit-code table, and the one function that produces it.
//!
//! `docs/reference/02-cli.md` promises codes 0–6. Success and every runtime
//! failure land here; code 2 is clap's judgement about argument parsing and is
//! asked for through [`exit_code_for_parse_error`]. The mapping is deliberately
//! exhaustive — no `_` arm — so adding a variant to [`m241_core::Error`] fails
//! to compile here instead of silently becoming a runtime error and costing a
//! documented code its producer.

/// The documented exit code for a whole run.
///
/// Success lives in the same table as the failures so that the tests can pin
/// every code from 0 to 6 in one place. Code 2 is not here: it belongs to
/// argument parsing, which happens before `run` is ever called.
pub fn exit_code_of(result: &m241_core::Result<()>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(err) => exit_code_for(err),
    }
}

/// 0 for `--help` / `--version`, 2 for a usage error.
///
/// Delegates the judgement to clap, which already knows which of its errors it
/// would print to stdout, but owns the number here so the documented table has
/// a producer in this crate rather than in a dependency's default.
pub fn exit_code_for_parse_error(err: &clap::Error) -> u8 {
    if err.use_stderr() { 2 } else { 0 }
}

/// Map an error onto the documented exit codes.
///
/// 0 ok / 1 runtime / 2 usage / 3 hardware / 4 verify / 5 safety / 6 flash.
///
/// Deliberately exhaustive: no `_` arm, so adding a variant to [`m241_core::Error`]
/// fails to compile here instead of silently becoming a runtime error and
/// costing a documented code its producer.
pub fn exit_code_for(err: &m241_core::Error) -> u8 {
    use m241_core::Error;
    match err {
        Error::Port { .. } | Error::Timeout { .. } => 3,
        Error::Framing { .. } | Error::Firmware { .. } | Error::Profile { .. } => 4,
        Error::Safety { .. } => 5,
        Error::Rejected { .. } => 6,
        Error::Log { .. } | Error::Nmea { .. } | Error::State(_) | Error::Io { .. } => 1,
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
    fn errors_and_their_codes() -> Vec<(m241_core::Error, u8)> {
        vec![
            (
                m241_core::Error::Port {
                    message: String::from("no such file"),
                },
                3,
            ),
            (
                m241_core::Error::Timeout {
                    what: String::from("sync"),
                    timeout: std::time::Duration::from_millis(500).into(),
                },
                3,
            ),
            (
                m241_core::Error::Framing {
                    what: String::from("echo"),
                    offset: Some(3),
                },
                4,
            ),
            (
                m241_core::Error::Firmware {
                    message: String::from("too short"),
                    offset: None,
                },
                4,
            ),
            (
                m241_core::Error::Profile {
                    message: String::from("unknown size"),
                    offset: None,
                },
                4,
            ),
            (
                m241_core::Error::Safety {
                    reason: String::from("no backup"),
                },
                5,
            ),
            (
                m241_core::Error::Rejected {
                    what: String::from("packet 12"),
                },
                6,
            ),
            (
                m241_core::Error::Log {
                    message: String::from("bad block"),
                    offset: None,
                },
                1,
            ),
            (
                m241_core::Error::Nmea {
                    message: String::from("bad checksum"),
                },
                1,
            ),
            (m241_core::Error::State(String::from("not connected")), 1),
            (
                // Built through `From<io::Error>` on purpose: the `Io` variant's
                // `path` field is being reworked in the core work stream, and
                // the conversion is the part of its shape that is stable.
                m241_core::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound)),
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
                exit_code_for(&err),
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
}
