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
    /// Suppress progress output.
    #[arg(long, global = true)]
    quiet: bool,

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
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("m241: error: {err}");
            if let Some(hint) = err.hint() {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(exit_code_for(&err))
        }
    }
}

fn run(cli: Cli) -> m241_core::Result<()> {
    let _ = (cli.quiet, cli.verbose);
    match cli.command {
        Command::Device { command } => match command {
            DeviceCommand::Ports => {
                for port in m241_core::protocol::Port::list()? {
                    println!("{port}");
                }
                Ok(())
            }
            DeviceCommand::Info | DeviceCommand::Doctor => Err(m241_core::Error::State(
                String::from("not implemented yet; the protocol layer lands with the core work"),
            )),
        },
        Command::Fw { command } => match command {
            FwCommand::Patch => Err(m241_core::Error::State(String::from(
                "not implemented yet; the patch plan lands with the core work",
            ))),
            FwCommand::Dump | FwCommand::Flash => Err(m241_core::Error::State(String::from(
                "the download-agent protocol is being re-established from the vendor binary",
            ))),
        },
        Command::Log { .. } | Command::Nmea { .. } => {
            Err(m241_core::Error::State(String::from("not implemented yet")))
        }
    }
}

/// Map an error onto the documented exit codes.
///
/// 0 ok / 1 runtime / 2 usage / 3 hardware / 4 verify / 5 safety / 6 flash.
fn exit_code_for(err: &m241_core::Error) -> u8 {
    use m241_core::Error;
    match err {
        Error::Port { .. } | Error::Timeout { .. } => 3,
        Error::Framing { .. } => 4,
        Error::Rejected { .. } => 6,
        Error::Firmware { .. } | Error::Profile { .. } => 4,
        _ => 1,
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

    #[test]
    fn port_failures_are_hardware_exit_code() {
        let err = m241_core::Error::Port {
            message: String::from("no such file"),
        };
        assert_eq!(exit_code_for(&err), 3);
    }

    #[test]
    fn a_rejected_write_is_the_flash_exit_code() {
        let err = m241_core::Error::Rejected {
            what: String::from("packet 12"),
        };
        assert_eq!(exit_code_for(&err), 6);
    }
}
