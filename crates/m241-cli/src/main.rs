//! `m241` — the command line interface.
//!
//! Subcommands are grouped by subject, then verb:
//!
//! ```text
//!   m241 device  ports | info | doctor
//!   m241 fw      dump  | patch | flash
//!   m241 log     info  | download | read | erase | geotag
//!   m241 nmea    repair | proxy
//! ```
//!
//! Machine output goes to stdout; progress and warnings go to stderr. Exit codes
//! are documented in `docs/reference/02-cli.md` and map one-to-one onto
//! [`ExitCode`].

// A CLI's whole job is to print. The lint that forbids `println!` exists to stop
// library code from doing it; here it is the interface.
#![allow(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

// `m241-flash` is a dependency of this binary because two of its subcommands
// write to the device: `fw flash` (the Download Agent) and `log erase`
// (`$PMTK182,6`). Neither is implemented yet, so nothing from the crate is
// called; the import keeps the dependency honest instead of declared-and-unused,
// and marks where those two subcommands will enter the write path.
#[allow(unused_imports)]
use m241_flash as _;

#[derive(Debug, Parser)]
#[command(
    name = "m241",
    version,
    about = "Repair and read a Holux M-241 GPS logger",
    long_about = "Repairs the GPS week-number-rollover date bug on a Holux M-241, \
                  and reads the track log off the device.\n\n\
                  Everything except the hardware commands runs offline.",
    propagate_version = true
)]
struct Cli {
    /// Print extra detail on stderr.
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// The device itself: ports, identity, environment.
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// The firmware image: dump, patch, flash.
    Fw {
        #[command(subcommand)]
        command: FwCommand,
    },
    /// The track log: download, inspect, convert.
    Log {
        #[command(subcommand)]
        command: LogCommand,
    },
    /// NMEA streams: repair dates, relay to another program.
    Nmea {
        #[command(subcommand)]
        command: NmeaCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// List serial ports and flag likely M-241 cables.
    Ports,
    /// Ask the logger for its firmware version and model.
    Info,
    /// Diagnose the environment and the device.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum FwCommand {
    /// Read the firmware off the device (read-only, safe).
    Dump,
    /// Apply the WNRO and version-marker patch to a dumped image.
    Patch,
    /// Write a firmware image back (destructive).
    Flash,
}

#[derive(Debug, Subcommand)]
enum LogCommand {
    /// Summarise a dumped track log.
    Info,
    /// Read the track log off the device.
    Download,
    /// Convert a dumped track log to GPX, KML, CSV, NMEA or JSON.
    Read,
    /// Erase the track log on the device (destructive).
    Erase,
    /// Write capture coordinates into photos' EXIF data.
    Geotag,
}

#[derive(Debug, Subcommand)]
enum NmeaCommand {
    /// Repair the dates in a captured NMEA file or stream.
    Repair,
    /// Relay a live NMEA stream with corrected dates.
    Proxy,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // `--help` and `--version` belong on stdout with exit 0; a bad
            // argument belongs on stderr with exit 2. clap already decides
            // which is which, so this only maps its decision onto the table.
            let _ = err.print();
            return ExitCode::from(exit_code_for_parse_error(&err));
        }
    };
    init_tracing(cli.verbose);
    let result = run(cli.command);
    if let Err(err) = &result {
        eprintln!("m241: error: {err}");
        if let Some(hint) = err.hint() {
            eprintln!("hint: {hint}");
        }
    }
    ExitCode::from(exit_code_of(&result))
}

fn run(command: Command) -> m241_core::Result<()> {
    match command {
        Command::Device { command } => match command {
            DeviceCommand::Ports => {
                for port in m241_core::protocol::Port::list()? {
                    println!("{port}");
                }
                Ok(())
            }
            DeviceCommand::Info => Err(not_implemented("device info")),
            DeviceCommand::Doctor => Err(not_implemented("device doctor")),
        },
        Command::Fw { command } => match command {
            // `dump` is a read: the BootROM client in `m241-core` performs it.
            FwCommand::Dump => Err(not_implemented("fw dump")),
            FwCommand::Patch => Err(not_implemented("fw patch")),
            FwCommand::Flash => Err(not_implemented("fw flash")),
        },
        Command::Log { command } => match command {
            LogCommand::Info => Err(not_implemented("log info")),
            LogCommand::Download => Err(not_implemented("log download")),
            LogCommand::Read => Err(not_implemented("log read")),
            LogCommand::Erase => Err(not_implemented("log erase")),
            LogCommand::Geotag => Err(not_implemented("log geotag")),
        },
        Command::Nmea { command } => match command {
            NmeaCommand::Repair => Err(not_implemented("nmea repair")),
            NmeaCommand::Proxy => Err(not_implemented("nmea proxy")),
        },
    }
}

/// The error for a command this build parses but cannot carry out yet.
///
/// The message names the command the user actually typed and says where to find
/// one that works. "not implemented yet" on its own leaves them with no next
/// step, and naming internal work items (`the vendor binary`) tells them nothing
/// they can act on.
fn not_implemented(command: &str) -> m241_core::Error {
    m241_core::Error::State(format!(
        "このビルドでは `m241 {command}` は未実装です。\
         `m241 --help` で使えるコマンドを確認してください。"
    ))
}

/// The documented exit code for a whole run.
///
/// Success lives in the same table as the failures so that the tests can pin
/// every code from 0 to 6 in one place. Code 2 is not here: it belongs to
/// argument parsing, which happens before `run` is ever called.
fn exit_code_of(result: &m241_core::Result<()>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(err) => exit_code_for(err),
    }
}

/// 0 for `--help` / `--version`, 2 for a usage error.
///
/// Delegates the judgement to clap, which already knows which of its errors it
/// would print to stdout, but owns the number here so the documented table has
/// a producer in this file rather than in a dependency's default.
fn exit_code_for_parse_error(err: &clap::Error) -> u8 {
    if err.use_stderr() { 2 } else { 0 }
}

/// Map an error onto the documented exit codes.
///
/// 0 ok / 1 runtime / 2 usage / 3 hardware / 4 verify / 5 safety / 6 flash.
///
/// Deliberately exhaustive: no `_` arm, so adding a variant to [`Error`] fails
/// to compile here instead of silently becoming a runtime error and costing a
/// documented code its producer.
fn exit_code_for(err: &m241_core::Error) -> u8 {
    use m241_core::Error;
    match err {
        Error::Port { .. } | Error::Timeout { .. } => 3,
        Error::Framing { .. } | Error::Firmware { .. } | Error::Profile { .. } => 4,
        Error::Safety { .. } => 5,
        Error::Rejected { .. } => 6,
        Error::Log { .. } | Error::Nmea { .. } | Error::State(_) | Error::Io { .. } => 1,
    }
}

fn init_tracing(verbose: bool) {
    let level = if verbose { "debug" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subcommand_parses() {
        for args in [
            vec!["m241", "device", "ports"],
            vec!["m241", "device", "info"],
            vec!["m241", "device", "doctor"],
            vec!["m241", "fw", "dump"],
            vec!["m241", "fw", "patch"],
            vec!["m241", "fw", "flash"],
            vec!["m241", "log", "info"],
            vec!["m241", "log", "download"],
            vec!["m241", "log", "read"],
            vec!["m241", "log", "erase"],
            vec!["m241", "log", "geotag"],
            vec!["m241", "nmea", "repair"],
            vec!["m241", "nmea", "proxy"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?} failed: {e}"));
        }
    }

    /// Every stub command, with the name its message has to carry.
    fn unimplemented_commands() -> Vec<(Command, &'static str)> {
        vec![
            (
                Command::Device {
                    command: DeviceCommand::Info,
                },
                "device info",
            ),
            (
                Command::Device {
                    command: DeviceCommand::Doctor,
                },
                "device doctor",
            ),
            (
                Command::Fw {
                    command: FwCommand::Dump,
                },
                "fw dump",
            ),
            (
                Command::Fw {
                    command: FwCommand::Patch,
                },
                "fw patch",
            ),
            (
                Command::Fw {
                    command: FwCommand::Flash,
                },
                "fw flash",
            ),
            (
                Command::Log {
                    command: LogCommand::Info,
                },
                "log info",
            ),
            (
                Command::Log {
                    command: LogCommand::Download,
                },
                "log download",
            ),
            (
                Command::Log {
                    command: LogCommand::Read,
                },
                "log read",
            ),
            (
                Command::Log {
                    command: LogCommand::Erase,
                },
                "log erase",
            ),
            (
                Command::Log {
                    command: LogCommand::Geotag,
                },
                "log geotag",
            ),
            (
                Command::Nmea {
                    command: NmeaCommand::Repair,
                },
                "nmea repair",
            ),
            (
                Command::Nmea {
                    command: NmeaCommand::Proxy,
                },
                "nmea proxy",
            ),
        ]
    }

    #[test]
    fn an_unimplemented_command_names_itself_and_a_next_step() {
        for (command, name) in unimplemented_commands() {
            let err = run(command).expect_err("a stub command must fail");
            let text = err.to_string();
            assert!(
                text.contains(&format!("`m241 {name}`")),
                "{name}: message does not name the command: {text}"
            );
            assert!(text.contains("未実装"), "{name}: not in Japanese: {text}");
            assert!(
                text.contains("`m241 --help`"),
                "{name}: no next step for the user: {text}"
            );
            assert!(
                !text.contains("not implemented") && !text.contains("vendor"),
                "{name}: still an internal work item: {text}"
            );
        }
    }

    #[test]
    fn two_operations_of_the_same_subject_get_different_messages() {
        let info = run(Command::Log {
            command: LogCommand::Info,
        })
        .expect_err("stub");
        let geotag = run(Command::Log {
            command: LogCommand::Geotag,
        })
        .expect_err("stub");
        assert_ne!(info.to_string(), geotag.to_string());
    }

    /// One error per variant: the table the docs promise, produced for real.
    ///
    /// The `Error` enum is matched without a `_` arm in `exit_code_for`, so a
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
            "2 is clap's, tested below"
        );
    }

    #[test]
    fn a_usage_error_is_exit_code_two() {
        let err = Cli::try_parse_from(["m241", "device", "blorp"])
            .expect_err("an unknown verb must be a usage error");
        assert!(err.use_stderr(), "usage errors go to stderr");
        assert_eq!(exit_code_for_parse_error(&err), 2);
    }

    #[test]
    fn help_and_version_stay_exit_code_zero() {
        for args in [vec!["m241", "--help"], vec!["m241", "--version"]] {
            let err = Cli::try_parse_from(&args).expect_err("clap short-circuits these");
            assert!(!err.use_stderr(), "{args:?} must print to stdout");
            assert_eq!(exit_code_for_parse_error(&err), 0);
        }
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
