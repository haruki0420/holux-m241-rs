//! The argument grammar: `m241 <subject> <verb>`.
//!
//! Subjects group commands by what they act on, so the help reads as a map of
//! the device rather than a list of verbs:
//!
//! ```text
//!   m241 device  ports | info | doctor
//!   m241 fw      dump  | patch | flash
//!   m241 log     info  | download | read | erase | geotag
//!   m241 nmea    repair | proxy
//! ```
//!
//! Nothing here does any work. The grammar is separate from the handlers so a
//! test can assert what a command line parses to without opening a port, and so
//! the handler for one verb is not buried in the middle of the enum that
//! describes all thirteen.
//!
//! Two rules from `docs/reference/02-cli.md` are enforced here rather than in a
//! handler, because clap can produce the documented exit code 2 for them:
//!
//! * `fw patch --dry-run` needs no `-o`, but a real run does;
//! * `--format` and `--utc-offset-days` are validated while parsing, so a typo
//!   is a usage error rather than a runtime failure halfway through a file.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use m241_core::ExportFormat;

use crate::cal::{self, OffsetSpec};

/// How many bytes `fw dump` reads from the firmware base when `--length` is not
/// given, and the default the handler falls back to.
///
/// The measured V1.13 image is exactly this long. A V1.12 unit's image is 308
/// bytes shorter (406,312), so `--length 406312` is the right command there;
/// reading the longer default on a V1.12 unit reads 308 bytes of whatever
/// follows the image, which the patch planner then rejects on size rather than
/// silently patching.
pub const DEFAULT_DUMP_LENGTH: usize = 406_620;

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
pub struct Cli {
    /// Print extra detail on stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
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
pub enum DeviceCommand {
    /// List serial ports and flag likely M-241 cables.
    Ports(PortsOptions),
    /// Ask the logger for its firmware version and model.
    Info(InfoOptions),
    /// Diagnose the environment and the device.
    Doctor,
}

#[derive(Debug, Default, Args)]
pub struct PortsOptions {
    /// Print one JSON object instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InfoOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

impl Default for InfoOptions {
    /// The defaults clap uses, so a test builds the same command the parser does.
    fn default() -> Self {
        Self {
            port: String::from("auto"),
            json: false,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum FwCommand {
    /// Read the firmware off the device (read-only, safe).
    Dump(DumpOptions),
    /// Apply the WNRO and version-marker patch to a dumped image.
    Patch(PatchOptions),
    /// Write a firmware image back (destructive).
    Flash,
}

#[derive(Debug, Args)]
pub struct DumpOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Where to write the image.
    #[arg(short, long)]
    pub output: PathBuf,

    /// How many bytes to read from the firmware base.
    ///
    /// The measured V1.13 image is 406,620 bytes; the V1.12 image is 406,312.
    #[arg(long, default_value_t = DEFAULT_DUMP_LENGTH)]
    pub length: usize,

    /// The chip is already in the BootROM; send the sync without `$PMTK180`.
    #[arg(long)]
    pub assume_brom: bool,

    /// Overwrite an existing output file.
    #[arg(long)]
    pub force: bool,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct PatchOptions {
    /// The dumped image to patch. Never modified.
    pub source: PathBuf,

    /// Where to write the patched image. Required unless `--dry-run`.
    #[arg(short, long, required_unless_present = "dry_run")]
    pub output: Option<PathBuf>,

    /// Print the plan and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum LogCommand {
    /// Summarise a dumped track log.
    Info(LogInfoOptions),
    /// Read the track log off the device.
    Download(DownloadOptions),
    /// Convert a dumped track log to GPX, KML, CSV, NMEA or JSON.
    Read(LogReadOptions),
    /// Erase the track log on the device (destructive).
    Erase,
    /// Write capture coordinates into photos' EXIF data.
    Geotag,
}

#[derive(Debug, Args)]
pub struct LogInfoOptions {
    /// The raw dump `m241 log download` wrote.
    pub file: PathBuf,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DownloadOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Where to write the raw dump.
    #[arg(short, long)]
    pub output: PathBuf,

    /// Overwrite an existing output file.
    #[arg(long)]
    pub force: bool,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LogReadOptions {
    /// The raw dump `m241 log download` wrote.
    pub file: PathBuf,

    /// Output format.
    #[arg(long, value_parser = parse_format, default_value = "gpx")]
    pub format: ExportFormat,

    /// Where to write the converted track; stdout when omitted.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Days to add to every timestamp, or `auto` to detect a WNRO-shifted log.
    ///
    /// `allow_hyphen_values` is what makes `--utc-offset-days -7168` a value
    /// rather than an unknown flag: a negative shift is the normal way to undo
    /// one applied by mistake.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_parser = cal::parse_offset,
        default_value = "auto"
    )]
    pub utc_offset_days: OffsetSpec,
}

#[derive(Debug, Subcommand)]
pub enum NmeaCommand {
    /// Repair the dates in a captured NMEA file or stream.
    Repair(RepairOptions),
    /// Relay a live NMEA stream with corrected dates.
    Proxy,
}

#[derive(Debug, Args)]
pub struct RepairOptions {
    /// The captured stream; `-` reads standard input.
    #[arg(default_value = "-")]
    pub input: String,

    /// Where to write the repaired stream; standard output when omitted.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Days to add to every date, or `auto` to fix only stale dates.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_parser = cal::parse_offset,
        default_value = "auto"
    )]
    pub offset_days: OffsetSpec,
}

/// `--format gpx` and friends, validated while parsing.
fn parse_format(text: &str) -> Result<ExportFormat, String> {
    ExportFormat::parse(text)
        .ok_or_else(|| format!("{text:?} is not a format; choose gpx, kml, csv, nmea or json"))
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
            vec!["m241", "fw", "dump", "-o", "stock.bin"],
            vec!["m241", "fw", "patch", "stock.bin", "--dry-run"],
            vec!["m241", "fw", "flash"],
            vec!["m241", "log", "info", "track.bin"],
            vec!["m241", "log", "download", "-o", "track.bin"],
            vec!["m241", "log", "read", "track.bin"],
            vec!["m241", "log", "erase"],
            vec!["m241", "log", "geotag"],
            vec!["m241", "nmea", "repair"],
            vec!["m241", "nmea", "proxy"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?} failed: {e}"));
        }
    }

    #[test]
    fn a_usage_error_is_exit_code_two() {
        let err = Cli::try_parse_from(["m241", "device", "blorp"])
            .expect_err("an unknown verb must be a usage error");
        assert!(err.use_stderr(), "usage errors go to stderr");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
    }

    #[test]
    fn help_and_version_stay_exit_code_zero() {
        for args in [vec!["m241", "--help"], vec!["m241", "--version"]] {
            let err = Cli::try_parse_from(&args).expect_err("clap short-circuits these");
            assert!(!err.use_stderr(), "{args:?} must print to stdout");
            assert_eq!(crate::exit::exit_code_for_parse_error(&err), 0);
        }
    }

    #[test]
    fn verbose_is_global_and_defaults_to_off() {
        let cli = Cli::try_parse_from(["m241", "device", "ports"]).expect("parses");
        assert!(!cli.verbose);
        let cli = Cli::try_parse_from(["m241", "--verbose", "device", "ports"]).expect("parses");
        assert!(cli.verbose);
        // Clap's `global` also accepts the flag after the subcommand.
        let cli = Cli::try_parse_from(["m241", "device", "ports", "--verbose"]).expect("parses");
        assert!(cli.verbose);
    }

    #[test]
    fn a_patch_run_without_dry_run_needs_an_output() {
        let err = Cli::try_parse_from(["m241", "fw", "patch", "stock.bin"])
            .expect_err("a real run must say where to write");
        assert!(err.use_stderr());
        assert!(err.to_string().contains("--output"), "got {err}");
        assert!(
            Cli::try_parse_from(["m241", "fw", "patch", "stock.bin", "--dry-run"]).is_ok(),
            "a dry run writes nothing and needs no destination"
        );
    }

    #[test]
    fn an_unknown_format_is_a_usage_error() {
        let err = Cli::try_parse_from(["m241", "log", "read", "track.bin", "--format", "yaml"])
            .expect_err("yaml is not one of the five formats");
        assert!(err.use_stderr());
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
    }

    #[test]
    fn the_offset_flag_takes_a_number_or_auto() {
        let cli = Cli::try_parse_from(["m241", "log", "read", "track.bin"]).expect("parses");
        let Command::Log {
            command: LogCommand::Read(options),
        } = cli.command
        else {
            panic!("expected log read");
        };
        assert_eq!(options.utc_offset_days, OffsetSpec::Auto);
        assert_eq!(options.format, ExportFormat::Gpx);

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "read",
            "track.bin",
            "--utc-offset-days",
            "-7168",
            "--format",
            "csv",
        ])
        .expect("parses");
        let Command::Log {
            command: LogCommand::Read(options),
        } = cli.command
        else {
            panic!("expected log read");
        };
        assert_eq!(options.utc_offset_days, OffsetSpec::Days(-7_168));
        assert_eq!(options.format, ExportFormat::Csv);

        let err = Cli::try_parse_from([
            "m241",
            "log",
            "read",
            "track.bin",
            "--utc-offset-days",
            "yesterday",
        ])
        .expect_err("not a number and not auto");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
    }
}
