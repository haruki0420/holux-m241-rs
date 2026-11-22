//! The argument grammar: `m241 <subject> <verb>`.
//!
//! Subjects group commands by what they act on, so the help reads as a map of
//! the device rather than a list of verbs:
//!
//! ```text
//!   m241 device  ports | info | doctor
//!   m241 fw      dump  | patch | flash | probe-da | da-restart
//!   m241 log     info  | download | read | edit | erase | geotag | kmz
//!   m241 nmea    repair | proxy
//! ```
//!
//! Nothing here does any work. The grammar is separate from the handlers so a
//! test can assert what a command line parses to without opening a port, and so
//! the handler for one verb is not buried in the middle of the enum that
//! describes all seventeen.
//!
//! Two rules from `docs/reference/02-cli.md` are enforced here rather than in a
//! handler, because clap can produce the documented exit code 2 for them:
//!
//! * `fw patch --dry-run` needs no `-o`, but a real run does;
//! * the value of a flag is validated while parsing, so a typo is a usage error
//!   (exit 2) rather than a runtime failure halfway through a file. Concretely:
//!   `--format` must be one of the five `ExportFormat` names; `--utc-offset-days`
//!   and `--offset-days` must be `auto` or a signed number of days that fits an
//!   `i32` (negative values need `allow_hyphen_values` to be seen as values at
//!   all); and `fw dump --length` must be a positive, even count that does not
//!   run off the end of the 32-bit BootROM address space from `BASE_ADDR_FIRMWARE`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use m241_core::ExportFormat;
use m241_core::protocol::brom::BASE_ADDR_FIRMWARE;

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
    Doctor(DoctorOptions),
}

#[derive(Debug, Args)]
pub struct DoctorOptions {
    /// Probe the logger at this serial port (or `auto` for the only likely cable).
    #[arg(long, value_name = "PORT|auto")]
    pub port: Option<String>,

    /// Inspect a folder of Holux files (read-only, bounded depth) by SHA-256 and size.
    #[arg(long, value_name = "DIR")]
    pub from_drop: Option<PathBuf>,

    /// Copy only an identified `DA_v3.8.4.bin` from `--from-drop` into this directory.
    #[arg(long, requires = "from_drop", value_name = "DIR")]
    pub extract_to: Option<PathBuf>,

    /// Allow overwriting an existing `DA_v3.8.4.bin` under `--extract-to`.
    ///
    /// No `requires`: `--force` on its own is an accepted no-op, because it is
    /// also the documented flag for the other writing commands and a script that
    /// passes it to every command must not fail on the one that happens to have
    /// nothing to overwrite.
    #[arg(long)]
    pub force: bool,

    /// Print one JSON object instead of labelled diagnostic lines.
    #[arg(long)]
    pub json: bool,
}

impl Default for DoctorOptions {
    /// The defaults clap uses, so a test builds the same command the parser does.
    fn default() -> Self {
        Self {
            port: None,
            from_drop: None,
            extract_to: None,
            force: false,
            json: false,
        }
    }
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
    /// Write a firmware image back (destructive; `--dry-run` writes nothing).
    Flash(FlashOptions),
    /// Load the DA and read the two candidate firmware bases through it.
    ///
    /// **Writes no flash.** The DA is loaded into the chip's RAM and reads the
    /// two addresses the DA-side firmware base could be, so the start address
    /// `fw flash` refuses on can be settled by measurement.
    ProbeDa(ProbeDaOptions),
    /// Restart a unit left running the DA (`0xD9`); writes no flash.
    ///
    /// Sends the one-byte `0xD9` and does not wait for a reply. `0xD9` boots
    /// the firmware; it neither erases nor programs flash, so no `--yes` is
    /// needed, and DTR/RTS are never touched. Use it when a failed DA phase
    /// left the unit unresponsive instead of removing the battery.
    DaRestart(DaRestartOptions),
}

/// `m241 fw flash`: check an image against a backup, or write it.
///
/// The position argument is the image, and `--backup` is the dump that was taken
/// first — `AGENTS.md` §2.3 makes the second mandatory, so it is not optional
/// here. `--dry-run` is the only thing that does not need a port: it reads the
/// two files, accepts or refuses the image, and prints the plan.
#[derive(Debug, Args)]
pub struct FlashOptions {
    /// The image to write: your own backup with the WNRO patch applied.
    #[arg(value_name = "IMAGE")]
    pub image: PathBuf,

    /// Your own dump of the unit, taken before anything was written. Required:
    /// it is how a failed write is undone.
    #[arg(long, value_name = "FILE")]
    pub backup: PathBuf,

    /// The vendor download agent (`DA_v3.8.4.bin`, 14,896 bytes).
    ///
    /// Required for a real write because the DA has to be loaded into the
    /// chip's RAM before it can program flash. Not used by `--dry-run`.
    #[arg(long, value_name = "FILE", required_unless_present = "dry_run")]
    pub da: Option<PathBuf>,

    /// Check the image and print the write plan; open no port and send nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Recovery mode: write `--backup` itself back over a unit a failed write
    /// left different. Requires the image to be byte-identical to the backup
    /// and skips the whole-unit comparison, which a partial unit cannot pass.
    /// `AGENTS.md` §2.5 covers this write-capable run.
    #[arg(long, conflicts_with = "dry_run")]
    pub recover: bool,

    /// Allow the pinned official V1.13 vendor image, identified by its full
    /// SHA-256. This is not the normal backup-diff path: it overwrites the
    /// unit's per-unit data. A real write requires `--yes`; `--dry-run` does
    /// not. `AGENTS.md` §2.3 / §2.5.
    #[arg(long, conflicts_with = "recover")]
    pub allow_vendor: bool,

    /// Confirm the write. There is no typed prompt in this build, so this is
    /// the only confirmation input; `AGENTS.md` §2.5 governs who may pass it.
    #[arg(long)]
    pub yes: bool,

    /// The chip is already in the BootROM; send the sync without `$PMTK180`.
    #[arg(long)]
    pub assume_brom: bool,

    /// Serial port for a real write, or `auto` for the only likely cable.
    ///
    /// Not used by `--dry-run`, which opens no port at all.
    #[arg(long, default_value = "auto", value_name = "PORT|auto")]
    pub port: String,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
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
    #[arg(long, default_value_t = DEFAULT_DUMP_LENGTH, value_parser = parse_dump_length)]
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

/// `m241 fw probe-da`: load the DA and read the two candidate bases.
#[derive(Debug, Args)]
pub struct ProbeDaOptions {
    /// The vendor download agent (`DA_v3.8.4.bin`, 14,896 bytes).
    ///
    /// Never committed here: it is a copy of MediaTek's binary that the user
    /// obtains from the vendor package.
    #[arg(long, value_name = "FILE")]
    pub da: PathBuf,

    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Confirm loading the DA; without it the command stops before opening a port.
    ///
    /// The load only writes the chip's RAM, but the DA it starts can drive the
    /// flash, so the hand-over is still an action that was asked for.
    #[arg(long)]
    pub yes: bool,

    /// The chip is already in the BootROM; send the sync without `$PMTK180`.
    #[arg(long)]
    pub assume_brom: bool,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

/// `m241 fw da-restart`: send the DA's one-byte `0xD9` restart.
#[derive(Debug, Args)]
pub struct DaRestartOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,
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
    /// Split, merge or trim the points of a dumped track log.
    Edit(LogEditOptions),
    /// Erase the track log on the device (destructive).
    Erase(EraseOptions),
    /// Write capture coordinates into photos' EXIF data.
    Geotag(LogGeotagOptions),
    /// Bundle a dumped track log and its photos into a KMZ archive.
    Kmz(LogKmzOptions),
}

#[derive(Debug, Args)]
pub struct LogGeotagOptions {
    /// The raw dump `m241 log download` wrote.
    #[arg(value_name = "LOG")]
    pub log: PathBuf,

    /// One or more photo files or directories to geotag.
    #[arg(required = true, num_args = 1.., value_name = "PHOTOS")]
    pub photos: Vec<PathBuf>,

    /// Largest accepted difference between photo capture time and track point, in seconds.
    #[arg(
        long,
        default_value_t = m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS,
        value_name = "SECONDS"
    )]
    pub max_gap: u32,

    /// Days to add to every track timestamp, or `auto` to detect a WNRO-shifted log.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_parser = cal::parse_offset,
        default_value = "auto"
    )]
    pub utc_offset_days: OffsetSpec,

    /// Camera clock offset from UTC in minutes (for example `540` for UTC+9) when EXIF `DateTimeOriginal` is in local time.
    #[arg(
        long,
        allow_hyphen_values = true,
        default_value_t = 0,
        value_name = "MINUTES"
    )]
    pub tz_offset_minutes: i32,

    /// Print the plan and modify no photos.
    #[arg(long)]
    pub dry_run: bool,

    /// Print one JSON object instead of labelled lines.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LogKmzOptions {
    /// The raw dump `m241 log download` wrote.
    pub log: PathBuf,

    /// Photo files or directories to embed, matched to track points by capture
    /// time. A directory contributes its immediate regular files.
    #[arg(value_name = "PHOTOS")]
    pub photos: Vec<PathBuf>,

    /// Where to write the archive.
    #[arg(short, long, value_name = "FILE")]
    pub output: PathBuf,

    /// Largest long edge an embedded photo may keep, in pixels.
    ///
    /// A photo over the limit is scaled down and re-encoded, which drops its
    /// EXIF; one at or under it keeps its exact bytes.
    #[arg(long, value_parser = parse_photo_edge, value_name = "PIXELS")]
    pub max_photo_edge: Option<u32>,

    /// Largest accepted difference between photo capture time and track point,
    /// in seconds. A photo farther away is not embedded.
    #[arg(
        long,
        default_value_t = m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS,
        value_name = "SECONDS"
    )]
    pub max_gap: u32,

    /// Days to add to every track timestamp, or `auto` to detect a WNRO-shifted log.
    ///
    /// Matching photos against a stale log would otherwise never find a point:
    /// `auto` is what makes a photo from an unpatched unit line up.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_parser = cal::parse_offset,
        default_value = "auto"
    )]
    pub utc_offset_days: OffsetSpec,

    /// Camera clock offset from UTC in minutes (for example `540` for UTC+9)
    /// when EXIF `DateTimeOriginal` is in local time.
    ///
    /// Matching moves a copy of the track into the camera's clock; the archive
    /// itself stays UTC, including the time recorded for each photo.
    #[arg(
        long,
        allow_hyphen_values = true,
        default_value_t = 0,
        value_name = "MINUTES"
    )]
    pub tz_offset_minutes: i32,
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
pub struct EraseOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Where to save the copy that is taken before the log is erased.
    ///
    /// Required, and written before `$PMTK182,6` is considered: an erase
    /// without a copy is the one mistake this command must not make.
    #[arg(short, long)]
    pub output: PathBuf,

    /// Overwrite an existing output file.
    #[arg(long)]
    pub force: bool,

    /// Erase the log after saving it; without this the download still runs.
    #[arg(long)]
    pub yes: bool,

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

/// `m241 log edit`: one pure edit of a dumped track log, then export it.
///
/// Exactly one of the three operations must be given. They are exclusive
/// because each is a single [`m241_core::log::edit::TrackEditor`] operation: a
/// command line that asked for two would produce a result no single invocation
/// could describe, and the user could not tell which half failed.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("operation")
        .required(true)
        .multiple(false)
        .args(["split_at", "merge", "delete"])
))]
pub struct LogEditOptions {
    /// The raw dump `m241 log download` wrote.
    pub log: PathBuf,

    /// Split the log into its first N points and the rest, writing the second
    /// half to a sibling file with `-2` before its extension (`track-2.gpx`).
    #[arg(long, value_name = "N")]
    pub split_at: Option<usize>,

    /// Another log dump to join. Repeat the flag for several dumps; they are
    /// ordered by their earliest point, not by the order they are listed.
    #[arg(long, value_name = "FILE")]
    pub merge: Vec<PathBuf>,

    /// Comma-separated point indices to delete, counted from zero (`3,5,9`).
    #[arg(long, value_delimiter = ',', value_name = "INDICES")]
    pub delete: Vec<usize>,

    /// Output format.
    #[arg(long, value_parser = parse_format, default_value = "gpx")]
    pub format: ExportFormat,

    /// Where to write the edited track. Required: a split writes two files.
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum NmeaCommand {
    /// Repair the dates in a captured NMEA file or stream.
    Repair(RepairOptions),
    /// Relay a live NMEA stream with corrected dates.
    Proxy(ProxyOptions),
}

#[derive(Debug, Args)]
pub struct ProxyOptions {
    /// Serial port, or `auto` to pick the only likely cable.
    #[arg(long, default_value = "auto")]
    pub port: String,

    /// Serial baud rate of the live NMEA stream.
    #[arg(long, default_value_t = m241_core::protocol::pmtk::NMEA_BAUD)]
    pub baud: u32,

    /// Days to add to every date, or `auto` to fix only stale dates.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_parser = cal::parse_offset,
        default_value = "auto"
    )]
    pub offset_days: OffsetSpec,

    /// Relay repaired NMEA lines to standard output (default when `--bind` is omitted).
    #[arg(long)]
    pub stdout: bool,

    /// Bind a TCP server relaying repaired NMEA lines to connected clients.
    #[arg(long, value_name = "HOST:PORT")]
    pub bind: Option<String>,

    /// Allow `--bind` to listen on a non-loopback address.
    #[arg(long)]
    pub allow_non_loopback: bool,

    /// Stop after relaying this many lines.
    #[arg(long, value_name = "N")]
    pub max_lines: Option<usize>,
}

impl Default for ProxyOptions {
    /// The defaults clap uses, so a test builds the same command the parser does.
    fn default() -> Self {
        Self {
            port: String::from("auto"),
            baud: m241_core::protocol::pmtk::NMEA_BAUD,
            offset_days: OffsetSpec::Auto,
            stdout: false,
            bind: None,
            allow_non_loopback: false,
            max_lines: None,
        }
    }
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

/// `--max-photo-edge`: a positive long-edge limit in pixels.
///
/// Zero is a usage error rather than "no limit": a user who types it meant a
/// number, and silently embedding full-size photos would hide the typo.
fn parse_photo_edge(text: &str) -> Result<u32, String> {
    let pixels = text
        .parse::<u32>()
        .map_err(|_| format!("{text:?} is not a pixel count"))?;
    if pixels == 0 {
        return Err(String::from(
            "--max-photo-edge must be positive; it is a long-edge limit in pixels",
        ));
    }
    Ok(pixels)
}

/// `--length` for `fw dump`: positive, even (16-bit BootROM reads), and within
/// the 32-bit BootROM address space from [`BASE_ADDR_FIRMWARE`].
fn parse_dump_length(text: &str) -> Result<usize, String> {
    let length = text
        .parse::<usize>()
        .map_err(|_| format!("{text:?} is not a valid byte length"))?;
    if length == 0 {
        return Err(String::from(
            "--length must be positive; a firmware image is about 400 KiB",
        ));
    }
    if !length.is_multiple_of(2) {
        return Err(format!(
            "--length {length} is odd; the BootROM reads 16-bit units"
        ));
    }
    if u32::try_from(length)
        .ok()
        .and_then(|bytes| BASE_ADDR_FIRMWARE.checked_add(bytes))
        .is_none()
    {
        return Err(format!(
            "--length {length} overflows the 32-bit BootROM address space from 0x{BASE_ADDR_FIRMWARE:08X}"
        ));
    }
    Ok(length)
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
            vec![
                "m241",
                "fw",
                "flash",
                "image.bin",
                "--backup",
                "backup.bin",
                "--dry-run",
            ],
            vec!["m241", "log", "info", "track.bin"],
            vec!["m241", "log", "download", "-o", "track.bin"],
            vec!["m241", "log", "read", "track.bin"],
            vec![
                "m241",
                "log",
                "edit",
                "track.bin",
                "--split-at",
                "2",
                "-o",
                "a.gpx",
            ],
            vec!["m241", "log", "erase", "-o", "track.bin"],
            vec!["m241", "log", "geotag", "track.bin", "photo.jpg"],
            vec!["m241", "log", "kmz", "track.bin", "-o", "track.kmz"],
            vec!["m241", "nmea", "repair"],
            vec!["m241", "nmea", "proxy"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?} failed: {e}"));
        }
    }

    #[test]
    fn non_destructive_subcommand_flags_parse_with_documented_defaults() {
        let cli = Cli::try_parse_from(["m241", "device", "doctor"]).expect("doctor defaults");
        let Command::Device {
            command: DeviceCommand::Doctor(doctor),
        } = cli.command
        else {
            panic!("expected device doctor");
        };
        assert_eq!(doctor.port, None);
        assert_eq!(doctor.from_drop, None);
        assert_eq!(doctor.extract_to, None);
        assert!(!doctor.force);
        assert!(!doctor.json);

        let err = Cli::try_parse_from(["m241", "device", "doctor", "--extract-to", "vendor"])
            .expect_err("--extract-to requires --from-drop");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);

        // S8: the documented combinations, pinned both ways. `--force` might look
        // as if it belonged to `--extract-to` alone, but the reference does not
        // say so; giving it `requires = "extract_to"` would break a script that
        // passes `--force` to every command, and giving `--from-drop` a
        // `requires = "extract_to"` would break a plain read-only inventory.
        Cli::try_parse_from(["m241", "device", "doctor", "--force"])
            .expect("--force on its own is an accepted no-op");
        Cli::try_parse_from(["m241", "device", "doctor", "--from-drop", "drop"])
            .expect("--from-drop alone is the read-only inventory");
        Cli::try_parse_from(["m241", "device", "doctor", "--from-drop", "drop", "--force"])
            .expect("--force with --from-drop but no extraction is still valid");
        Cli::try_parse_from([
            "m241",
            "device",
            "doctor",
            "--from-drop",
            "drop",
            "--extract-to",
            "vendor",
            "--force",
            "--json",
        ])
        .expect("the documented extraction command line");

        let cli = Cli::try_parse_from(["m241", "device", "doctor", "--force"])
            .expect("--force alone parses");
        let Command::Device {
            command: DeviceCommand::Doctor(doctor),
        } = cli.command
        else {
            panic!("expected device doctor");
        };
        assert!(doctor.force, "the flag reaches the handler");

        let cli =
            Cli::try_parse_from(["m241", "log", "geotag", "track.bin", "a.jpg", "photos_dir"])
                .expect("log geotag defaults");
        let Command::Log {
            command: LogCommand::Geotag(geotag),
        } = cli.command
        else {
            panic!("expected log geotag");
        };
        assert_eq!(geotag.log, PathBuf::from("track.bin"));
        assert_eq!(
            geotag.photos,
            vec![PathBuf::from("a.jpg"), PathBuf::from("photos_dir")]
        );
        assert_eq!(
            geotag.max_gap,
            m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS
        );
        assert_eq!(geotag.utc_offset_days, OffsetSpec::Auto);
        assert_eq!(geotag.tz_offset_minutes, 0);
        assert!(!geotag.dry_run);
        assert!(!geotag.json);

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "geotag",
            "track.bin",
            "a.jpg",
            "--max-gap",
            "120",
            "--utc-offset-days",
            "7168",
            "--tz-offset-minutes",
            "-300",
            "--dry-run",
            "--json",
        ])
        .expect("log geotag custom flags");
        let Command::Log {
            command: LogCommand::Geotag(geotag),
        } = cli.command
        else {
            panic!("expected log geotag");
        };
        assert_eq!(geotag.max_gap, 120);
        assert_eq!(geotag.utc_offset_days, OffsetSpec::Days(7_168));
        assert_eq!(geotag.tz_offset_minutes, -300);
        assert!(geotag.dry_run);
        assert!(geotag.json);

        let cli = Cli::try_parse_from(["m241", "nmea", "proxy"]).expect("nmea proxy defaults");
        let Command::Nmea {
            command: NmeaCommand::Proxy(proxy),
        } = cli.command
        else {
            panic!("expected nmea proxy");
        };
        assert_eq!(proxy.port, "auto");
        assert_eq!(proxy.baud, m241_core::protocol::pmtk::NMEA_BAUD);
        assert_eq!(proxy.offset_days, OffsetSpec::Auto);
        assert!(!proxy.stdout);
        assert_eq!(proxy.bind, None);
        assert!(!proxy.allow_non_loopback);
        assert_eq!(proxy.max_lines, None);
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
    fn fw_flash_parses_the_documented_flags_and_defaults() {
        let cli = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--dry-run",
        ])
        .expect("the documented dry-run command line");
        let Command::Fw {
            command: FwCommand::Flash(flash),
        } = cli.command
        else {
            panic!("expected fw flash");
        };
        assert_eq!(flash.image, PathBuf::from("image.bin"));
        assert_eq!(flash.backup, PathBuf::from("backup.bin"));
        assert_eq!(flash.da, None, "a dry run needs no DA file");
        assert!(flash.dry_run);
        assert!(!flash.recover);
        assert!(!flash.allow_vendor);
        assert!(!flash.yes);
        assert!(!flash.assume_brom);
        assert_eq!(flash.port, "auto");
        assert!(!flash.json);

        let cli = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--da",
            "DA_v3.8.4.bin",
            "--recover",
            "--yes",
            "--assume-brom",
            "--port",
            "COM3",
            "--json",
        ])
        .expect("the documented real-write command line");
        let Command::Fw {
            command: FwCommand::Flash(flash),
        } = cli.command
        else {
            panic!("expected fw flash");
        };
        assert!(!flash.dry_run);
        assert!(flash.recover);
        assert!(!flash.allow_vendor);
        assert_eq!(
            flash.da,
            Some(PathBuf::from("DA_v3.8.4.bin")),
            "a real write needs the vendor DA"
        );
        assert!(flash.yes);
        assert!(flash.assume_brom);
        assert_eq!(flash.port, "COM3");
        assert!(flash.json);
    }

    #[test]
    fn fw_da_restart_parses_the_port_and_takes_no_yes() {
        let cli = Cli::try_parse_from(["m241", "fw", "da-restart"]).expect("parses");
        let Command::Fw {
            command: FwCommand::DaRestart(options),
        } = cli.command
        else {
            panic!("expected fw da-restart");
        };
        assert_eq!(options.port, "auto", "the documented default");

        let cli = Cli::try_parse_from(["m241", "fw", "da-restart", "--port", "COM3"])
            .expect("a resolved port parses");
        let Command::Fw {
            command: FwCommand::DaRestart(options),
        } = cli.command
        else {
            panic!("expected fw da-restart");
        };
        assert_eq!(options.port, "COM3");

        // `0xD9` restarts the chip and writes no flash, so it is not a
        // write-capable run and must not offer the confirmation flag.
        let err = Cli::try_parse_from(["m241", "fw", "da-restart", "--yes"])
            .expect_err("--yes is not part of da-restart");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn fw_flash_without_a_da_is_a_usage_error_unless_dry_run() {
        // The DA is loaded into the chip's RAM before a real write, so the
        // grammar asks for it up front rather than opening a port and then
        // failing with "no DA". A dry run touches no device and needs none.
        let err = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--yes",
        ])
        .expect_err("no --da");
        assert!(err.use_stderr(), "got {err}");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
        assert!(err.to_string().contains("--da"), "got {err}");

        Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--dry-run",
        ])
        .expect("a dry run needs no --da");
    }

    #[test]
    fn fw_flash_no_longer_accepts_allow_cross_model() {
        // The vendor/cross-model route was removed (`AGENTS.md` §2.3). If the
        // flag is put back without the rule that made it reachable, this fails
        // before a user can believe the bypass exists again.
        let err = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--allow-cross-model",
        ])
        .expect_err("--allow-cross-model must be an unknown argument");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
        assert!(
            err.to_string().contains("--allow-cross-model"),
            "the usage error must name the removed flag: {err}"
        );
    }

    #[test]
    fn fw_flash_allow_vendor_parses_and_conflicts_with_recover() {
        let cli = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "vendor.bin",
            "--backup",
            "backup.bin",
            "--allow-vendor",
            "--dry-run",
        ])
        .expect("--allow-vendor with a dry run");
        let Command::Fw {
            command: FwCommand::Flash(flash),
        } = cli.command
        else {
            panic!("expected fw flash");
        };
        assert!(flash.allow_vendor);

        let err = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "vendor.bin",
            "--backup",
            "backup.bin",
            "--allow-vendor",
            "--recover",
        ])
        .expect_err("--allow-vendor and --recover must conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn fw_flash_recover_conflicts_with_dry_run() {
        // `--recover` is a write-capable mode; a dry run cannot preview it
        // because the normal plan would not skip `verify_device`.
        let err = Cli::try_parse_from([
            "m241",
            "fw",
            "flash",
            "image.bin",
            "--backup",
            "backup.bin",
            "--dry-run",
            "--recover",
        ])
        .expect_err("--recover and --dry-run must conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn fw_flash_without_a_backup_is_a_usage_error() {
        // `AGENTS.md` §2.3: no write is planned without the user's own dump, so
        // a command line that names no backup is a usage error (exit 2), not a
        // runtime refusal after the files are read.
        let err =
            Cli::try_parse_from(["m241", "fw", "flash", "image.bin"]).expect_err("no --backup");
        assert!(err.use_stderr(), "got {err}");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
        assert!(err.to_string().contains("--backup"), "got {err}");
    }

    #[test]
    fn log_erase_needs_a_backup_path_and_parses_its_flags() {
        let err = Cli::try_parse_from(["m241", "log", "erase"])
            .expect_err("an erase must say where the backup goes");
        assert!(err.use_stderr());
        assert!(err.to_string().contains("--output"), "got {err}");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);

        let cli = Cli::try_parse_from(["m241", "log", "erase", "-o", "track.bin"])
            .expect("the documented invocation parses");
        let Command::Log {
            command: LogCommand::Erase(options),
        } = cli.command
        else {
            panic!("expected log erase");
        };
        assert_eq!(options.port, "auto");
        assert_eq!(options.output, PathBuf::from("track.bin"));
        assert!(!options.force);
        assert!(!options.yes);
        assert!(!options.json);

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "erase",
            "--port",
            "COM3",
            "-o",
            "backup.bin",
            "--force",
            "--yes",
            "--json",
        ])
        .expect("every erase flag parses");
        let Command::Log {
            command: LogCommand::Erase(options),
        } = cli.command
        else {
            panic!("expected log erase");
        };
        assert_eq!(options.port, "COM3");
        assert_eq!(options.output, PathBuf::from("backup.bin"));
        assert!(options.force);
        assert!(options.yes);
        assert!(options.json);
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

    #[test]
    fn log_kmz_parses_its_documented_flags_and_defaults() {
        let cli = Cli::try_parse_from(["m241", "log", "kmz", "track.bin", "-o", "track.kmz"])
            .expect("log kmz without photos");
        let Command::Log {
            command: LogCommand::Kmz(kmz),
        } = cli.command
        else {
            panic!("expected log kmz");
        };
        assert_eq!(kmz.log, PathBuf::from("track.bin"));
        assert!(kmz.photos.is_empty(), "the photos are optional");
        assert_eq!(kmz.output, PathBuf::from("track.kmz"));
        assert_eq!(
            kmz.max_photo_edge, None,
            "no limit embeds the bytes as they are"
        );
        assert_eq!(kmz.max_gap, m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS);
        assert_eq!(kmz.utc_offset_days, OffsetSpec::Auto);
        assert_eq!(
            kmz.tz_offset_minutes, 0,
            "the camera clock is UTC unless the user says otherwise"
        );

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "kmz",
            "track.bin",
            "photos",
            "a.jpg",
            "-o",
            "track.kmz",
            "--max-photo-edge",
            "1600",
            "--max-gap",
            "120",
            "--utc-offset-days",
            "7168",
            "--tz-offset-minutes",
            "540",
        ])
        .expect("log kmz with every flag");
        let Command::Log {
            command: LogCommand::Kmz(kmz),
        } = cli.command
        else {
            panic!("expected log kmz");
        };
        assert_eq!(
            kmz.photos,
            vec![PathBuf::from("photos"), PathBuf::from("a.jpg")]
        );
        assert_eq!(kmz.max_photo_edge, Some(1600));
        assert_eq!(kmz.max_gap, 120);
        assert_eq!(kmz.utc_offset_days, OffsetSpec::Days(7_168));
        assert_eq!(kmz.tz_offset_minutes, 540);

        let err = Cli::try_parse_from([
            "m241",
            "log",
            "kmz",
            "track.bin",
            "-o",
            "track.kmz",
            "--max-photo-edge",
            "0",
        ])
        .expect_err("zero is not a limit");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
    }

    #[test]
    fn log_edit_requires_exactly_one_operation() {
        // None: the group is required. Two: they conflict. One: it parses, and
        // each operation carries its own arguments.
        let err = Cli::try_parse_from(["m241", "log", "edit", "track.bin", "-o", "out.gpx"])
            .expect_err("no operation");
        assert!(err.use_stderr(), "got {err}");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);

        let err = Cli::try_parse_from([
            "m241",
            "log",
            "edit",
            "track.bin",
            "--split-at",
            "2",
            "--delete",
            "1",
            "-o",
            "out.gpx",
        ])
        .expect_err("two operations");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert_eq!(
            crate::exit::exit_code_for_parse_error(&err),
            2,
            "two operations are a usage error"
        );

        // `log edit` works on the timestamps as recorded, so the timer flags
        // `log read` has are not silently accepted here.
        let err = Cli::try_parse_from([
            "m241",
            "log",
            "edit",
            "track.bin",
            "--split-at",
            "2",
            "--utc-offset-days",
            "-7168",
            "-o",
            "out.gpx",
        ])
        .expect_err("edit has no --utc-offset-days");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);

        // `-o` is required: a split writes two files, so there is no default.
        let err = Cli::try_parse_from(["m241", "log", "edit", "track.bin", "--split-at", "2"])
            .expect_err("edit without -o");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);
        let err =
            Cli::try_parse_from(["m241", "log", "kmz", "track.bin"]).expect_err("kmz without -o");
        assert_eq!(crate::exit::exit_code_for_parse_error(&err), 2);

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "edit",
            "track.bin",
            "--split-at",
            "2",
            "-o",
            "out.gpx",
        ])
        .expect("split");
        let Command::Log {
            command: LogCommand::Edit(edit),
        } = cli.command
        else {
            panic!("expected log edit");
        };
        assert_eq!(edit.log, PathBuf::from("track.bin"));
        assert_eq!(edit.split_at, Some(2));
        assert!(edit.merge.is_empty());
        assert!(edit.delete.is_empty());
        assert_eq!(edit.format, ExportFormat::Gpx);
        assert_eq!(edit.output, PathBuf::from("out.gpx"));

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "edit",
            "track.bin",
            "--merge",
            "a.bin",
            "--merge",
            "b.bin",
            "--format",
            "csv",
            "-o",
            "out.csv",
        ])
        .expect("merge");
        let Command::Log {
            command: LogCommand::Edit(edit),
        } = cli.command
        else {
            panic!("expected log edit");
        };
        assert_eq!(
            edit.merge,
            vec![PathBuf::from("a.bin"), PathBuf::from("b.bin")]
        );
        assert_eq!(edit.format, ExportFormat::Csv);

        let cli = Cli::try_parse_from([
            "m241",
            "log",
            "edit",
            "track.bin",
            "--delete",
            "3,5,9",
            "-o",
            "out.csv",
        ])
        .expect("delete");
        let Command::Log {
            command: LogCommand::Edit(edit),
        } = cli.command
        else {
            panic!("expected log edit");
        };
        assert_eq!(edit.delete, vec![3, 5, 9]);
    }

    #[test]
    fn invalid_dump_lengths_are_usage_errors() {
        for bad in ["0", "4001", "4294967294"] {
            let err =
                Cli::try_parse_from(["m241", "fw", "dump", "-o", "stock.bin", "--length", bad])
                    .expect_err("invalid --length must fail during argument parsing");
            assert!(err.use_stderr(), "length {bad}: {err}");
            assert_eq!(
                crate::exit::exit_code_for_parse_error(&err),
                2,
                "length {bad}: {err}"
            );
        }

        let cli = Cli::try_parse_from([
            "m241",
            "fw",
            "dump",
            "-o",
            "stock.bin",
            "--length",
            "406312",
        ])
        .expect("V1.12 length parses");
        let Command::Fw {
            command: FwCommand::Dump(options),
        } = cli.command
        else {
            panic!("expected fw dump");
        };
        assert_eq!(options.length, 406_312);
    }
}
