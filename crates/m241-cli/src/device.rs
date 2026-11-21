//! `m241 device ports`, `m241 device info`, and `m241 device doctor`.
//!
//! `ports` is an enumeration with a judgement attached: a bare device name
//! answers "is anything there", and the chip name answers "is that the logger".
//! Getting the second answer wrong is the difference between a five-second
//! timeout and reading the identity off the unit on the first try, so the
//! judgement is part of the output rather than left to the reader.
//!
//! `info` is one request/reply round trip, and the only command whose whole job
//! is to prove the wire works. It is also where the release string comes from:
//! the patch path does not trust that string — it decides on the image's bytes —
//! so an unknown release is a warning here, never a refusal.
//!
//! `doctor` combines environment checks, serial-port discovery, an optional
//! read-only `$PMTK605` probe, and an optional read-only inventory of a Holux
//! drop directory (identifying stock firmware profiles and `DA_v3.8.4.bin`).

use std::path::{Path, PathBuf};

use m241_core::firmware::image::FirmwareImage;
use m241_core::firmware::profiles::{Evidence, PROFILES};
use m241_core::firmware::sha256_hex;
use m241_core::{DeviceInfo, Error};

use crate::Result;
use crate::Session;
use crate::cli::{DoctorOptions, InfoOptions, PortsOptions};
use crate::fileio;
use crate::link::{self, Cable, PortEntry};

/// Expected filename of the proprietary MediaTek Download Agent.
const DA_FILENAME: &str = "DA_v3.8.4.bin";

/// Expected byte length of `DA_v3.8.4.bin`.
///
/// `docs/reference/01-da-protocol.md` §9: "The image this page describes is
/// `DA_v3.8.4.bin`, 14,896 bytes, covering `0x00100800`–`0x0010422F`" — the
/// same page's §2 uses the figure again ("outside the 14,896-byte image").
/// `0x0010422F - 0x00100800 + 1` is 14,896, so the size and the address range
/// agree. The size is the check this command makes; the contents are
/// deliberately not hashed, because the drop is a user's copy and not a pinned
/// upstream artefact.
const DA_SIZE_BYTES: usize = 14_896;

/// Maximum directory recursion depth when scanning `--from-drop`.
///
/// `docs/reference/02-cli.md` §`m241 device doctor` promises a "read-only
/// bounded-depth scan". The bound counts directory levels *below* the directory
/// given to `--from-drop`: a file `DROP/a/b/c/d/name` is four levels deep and is
/// read; `DROP/a/b/c/d/e/name` is five and the scan stops at `e`, reporting on
/// stderr that it did. `the_drop_scan_stops_one_level_past_max_drop_depth` pins
/// both sides of that line. Four is enough for Holux's own drop (`firmware/`,
/// `tool/`) plus an unpacked archive, and small enough that pointing
/// `--from-drop` at a home directory cannot walk the whole tree.
const MAX_DROP_DEPTH: usize = 4;

/// List the serial ports, flagging the ones that look like an M-241 cable.
///
/// # Errors
///
/// The enumeration's failure when the OS cannot list ports at all. An empty list
/// is not an error: `docs/reference/02-cli.md` says so, and a script that runs
/// `device ports` before plugging anything in should not have to special-case 0.
pub fn ports(options: &PortsOptions, session: Session<'_>) -> Result<()> {
    let entries = session.links.ports()?;
    if options.json {
        return session.console.json(&ports_document(&entries));
    }
    if entries.is_empty() {
        session.console.result(
            "no serial ports found. Plug the M-241 in with its USB cable, switch it on, and \
             run `m241 device ports` again.",
        );
        return Ok(());
    }
    session.console.result(&format!(
        "{:<22} {:<8} {:<7} {}",
        "DEVICE", "CABLE", "LIKELY", "DESCRIPTION"
    ));
    for entry in &entries {
        session.console.result(&format!(
            "{:<22} {:<8} {:<7} {}",
            entry.name,
            entry.cable.map_or("-", Cable::label),
            if entry.likely { "*" } else { "" },
            entry.description.as_deref().unwrap_or("-"),
        ));
    }
    session.console.result("");
    let candidates = candidates(&entries);
    if candidates.is_empty() {
        session.console.result(
            "No obvious M-241 cable found. The logger needs its USB cable attached and the \
             unit switched on.",
        );
    } else {
        session
            .console
            .result(&format!("candidate port(s): {}", candidates.join(", ")));
    }
    Ok(())
}

/// The ports a bare `--port auto` would consider.
#[must_use]
pub(crate) fn candidates(entries: &[PortEntry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|entry| entry.likely)
        .map(|entry| entry.name.as_str())
        .collect()
}

fn port_entry_documents(entries: &[PortEntry]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "name": entry.name,
                "cable": entry.cable.map(Cable::label),
                "likely": entry.likely,
                "description": entry.description,
            })
        })
        .collect()
}

/// The ports as the one JSON object `--json` promises.
#[must_use]
pub(crate) fn ports_document(entries: &[PortEntry]) -> serde_json::Value {
    serde_json::json!({
        "ports": port_entry_documents(entries),
        "candidates": candidates(entries),
    })
}

/// Ask the unit who it is: `$PMTK605` → `$PMTK705`.
///
/// # Errors
///
/// [`m241_core::Error::Port`] when the port cannot be opened or no plausible
/// port exists, [`m241_core::Error::Timeout`] when the unit does not answer,
/// [`m241_core::Error::Nmea`] when the reply is malformed.
pub fn info(options: &InfoOptions, session: Session<'_>) -> Result<()> {
    let port = link::resolve(session.links, &options.port)?;
    let mut client = link::open_pmtk(session.links, &port, session.timings)?;
    let identity = client.request_identity()?;
    warn_about_unknown_release(session.console, &identity.release);

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "release": identity.release,
            "build": identity.build,
            "model": identity.model,
            "sdk": identity.sdk,
            "known_release": is_known_release(&identity.release),
        }));
    }
    session.console.result(&format!("{:<10}: {port}", "port"));
    session
        .console
        .result(&format!("{:<10}: {}", "release", identity.release));
    session
        .console
        .result(&format!("{:<10}: {}", "build", identity.build));
    session
        .console
        .result(&format!("{:<10}: {}", "model", identity.model));
    session.console.result(&format!(
        "{:<10}: {}",
        "sdk",
        identity.sdk.as_deref().unwrap_or("-")
    ));
    Ok(())
}

/// Whether a release string names a firmware family this build knows.
///
/// A warning, never a refusal: a unit that reports an unknown release may still
/// be repairable, and the patch path decides on the image rather than on the
/// string.
#[must_use]
pub(crate) fn is_known_release(release: &str) -> bool {
    release.starts_with("B-core_1.")
}

/// Warn about a release string this build does not recognise.
///
/// One helper rather than the same sentence in `info` and `doctor`: the two
/// places must not drift into saying different things about the same unit, and a
/// wording change is meant to happen once.
fn warn_about_unknown_release(console: &dyn crate::Console, release: &str) {
    if is_known_release(release) {
        return;
    }
    console.warn(&format!(
        "the release {release:?} is not one this build knows; the patch path decides on the \
         image's bytes rather than on this string"
    ));
}

/// Outcome of the optional `$PMTK605` device probe in `device doctor`.
#[derive(Debug, Clone)]
struct ProbeReport {
    requested_port: String,
    resolved_port: Option<String>,
    identity: Option<DeviceInfo>,
    error: Option<String>,
}

/// Classification of one file found under `--from-drop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropFileKind {
    DownloadAgent,
    Firmware {
        profile: &'static str,
        evidence: Evidence,
    },
    Unknown,
}

impl DropFileKind {
    fn label(self) -> &'static str {
        match self {
            Self::DownloadAgent => "download_agent",
            Self::Firmware { .. } => "firmware",
            Self::Unknown => "unknown",
        }
    }
}

/// Inventory entry for one file scanned in `--from-drop`.
#[derive(Debug, Clone)]
struct DropFileEntry {
    path: PathBuf,
    size: usize,
    sha256: String,
    kind: DropFileKind,
    note: Option<String>,
}

/// Summary of the `--from-drop` directory scan and optional DA extraction.
#[derive(Debug, Clone)]
struct DropReport {
    directory: PathBuf,
    files: Vec<DropFileEntry>,
    download_agent: Option<PathBuf>,
    extracted_to: Option<PathBuf>,
}

/// Diagnose the environment, serial ports, optional vendor drop, and optional device.
///
/// Missing ports or failed `$PMTK605` probes are recorded as diagnostic findings
/// rather than crashing.
///
/// # Errors
///
/// * [`Error::Io`] when `--from-drop` cannot be read or `--extract-to` cannot be written;
/// * [`Error::Safety`] when `--extract-to` would overwrite an existing `DA_v3.8.4.bin`
///   without `--force` (exit code 5);
/// * [`Error::State`] when `--extract-to` is requested but no valid `DA_v3.8.4.bin`
///   was found in `--from-drop`.
pub fn doctor(options: &DoctorOptions, session: Session<'_>) -> Result<()> {
    let ports_result = session.links.ports();
    let probe = options
        .port
        .as_deref()
        .map(|port_spec| probe_device(port_spec, session));
    let drop = match &options.from_drop {
        Some(dir) => Some(inspect_drop(
            dir,
            options.extract_to.as_deref(),
            options.force,
            session.console,
        )?),
        None => None,
    };

    if options.json {
        return session.console.json(&doctor_document(
            &ports_result,
            probe.as_ref(),
            drop.as_ref(),
        ));
    }
    print_doctor_text(
        session.console,
        &ports_result,
        probe.as_ref(),
        drop.as_ref(),
    );
    Ok(())
}

/// Probe the unit via `$PMTK605` without failing the whole doctor report.
fn probe_device(port_spec: &str, session: Session<'_>) -> ProbeReport {
    let resolved_port = match link::resolve(session.links, port_spec) {
        Ok(name) => name,
        Err(err) => {
            return ProbeReport {
                requested_port: port_spec.to_owned(),
                resolved_port: None,
                identity: None,
                error: Some(err.to_string()),
            };
        }
    };
    let mut client = match link::open_pmtk(session.links, &resolved_port, session.timings) {
        Ok(client) => client,
        Err(err) => {
            return ProbeReport {
                requested_port: port_spec.to_owned(),
                resolved_port: Some(resolved_port),
                identity: None,
                error: Some(err.to_string()),
            };
        }
    };
    match client.request_identity() {
        Ok(identity) => {
            warn_about_unknown_release(session.console, &identity.release);
            ProbeReport {
                requested_port: port_spec.to_owned(),
                resolved_port: Some(resolved_port),
                identity: Some(identity),
                error: None,
            }
        }
        Err(err) => ProbeReport {
            requested_port: port_spec.to_owned(),
            resolved_port: Some(resolved_port),
            identity: None,
            error: Some(err.to_string()),
        },
    }
}

/// Walk `drop_dir` up to [`MAX_DROP_DEPTH`], identify known files, and optionally extract `DA_v3.8.4.bin`.
fn inspect_drop(
    drop_dir: &Path,
    extract_to: Option<&Path>,
    force: bool,
    console: &dyn crate::Console,
) -> Result<DropReport> {
    let mut file_paths = Vec::new();
    let mut depth_truncated = false;
    collect_drop_files(drop_dir, 0, &mut file_paths, &mut depth_truncated)?;
    if depth_truncated {
        console.warn(&format!(
            "stopped scanning subdirectories deeper than {MAX_DROP_DEPTH} levels under {}",
            drop_dir.display()
        ));
    }

    let mut files = Vec::with_capacity(file_paths.len());
    let mut download_agent: Option<PathBuf> = None;
    for path in file_paths {
        let bytes = fileio::read(&path)?;
        let entry = classify_drop_file(&path, &bytes);
        if entry.kind == DropFileKind::DownloadAgent && download_agent.is_none() {
            download_agent = Some(path);
        }
        files.push(entry);
    }

    let extracted_to = match extract_to {
        Some(dest_dir) => {
            let Some(source_path) = &download_agent else {
                return Err(Error::State(format!(
                    "no valid {DA_FILENAME} ({DA_SIZE_BYTES} bytes) found under {}",
                    drop_dir.display()
                )));
            };
            let dest_path = dest_dir.join(DA_FILENAME);
            fileio::ensure_free(&dest_path, force)?;
            let da_bytes = fileio::read(source_path)?;
            fileio::write(&dest_path, &da_bytes)?;
            Some(dest_path)
        }
        None => None,
    };

    Ok(DropReport {
        directory: drop_dir.to_path_buf(),
        files,
        download_agent,
        extracted_to,
    })
}

/// Collect regular files under `dir` up to [`MAX_DROP_DEPTH`], sorted deterministically by path.
fn collect_drop_files(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
    depth_truncated: &mut bool,
) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).map_err(|source| Error::io(dir.display().to_string(), source))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::io(dir.display().to_string(), source))?;
        let file_type = entry
            .file_type()
            .map_err(|source| Error::io(entry.path().display().to_string(), source))?;
        paths.push((entry.path(), file_type));
    }
    paths.sort_by(|(left, _), (right, _)| left.cmp(right));

    for (path, file_type) in paths {
        if file_type.is_file() {
            out.push(path);
        } else if file_type.is_dir() {
            if depth < MAX_DROP_DEPTH {
                collect_drop_files(&path, depth + 1, out, depth_truncated)?;
            } else {
                *depth_truncated = true;
            }
        }
    }
    Ok(())
}

/// Identify a single file from `--from-drop` by SHA-256, firmware structure, or DA filename+size.
fn classify_drop_file(path: &Path, bytes: &[u8]) -> DropFileEntry {
    let size = bytes.len();
    let sha256 = sha256_hex(bytes);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");

    if file_name.eq_ignore_ascii_case(DA_FILENAME) {
        if size == DA_SIZE_BYTES {
            return DropFileEntry {
                path: path.to_path_buf(),
                size,
                sha256,
                kind: DropFileKind::DownloadAgent,
                note: None,
            };
        }
        return DropFileEntry {
            path: path.to_path_buf(),
            size,
            sha256,
            kind: DropFileKind::Unknown,
            note: Some(format!(
                "filename matches {DA_FILENAME}, but expected {DA_SIZE_BYTES} bytes (got {size})"
            )),
        };
    }

    if let Some(profile) = PROFILES
        .iter()
        .find(|profile| sha256.eq_ignore_ascii_case(profile.sha256))
    {
        return DropFileEntry {
            path: path.to_path_buf(),
            size,
            sha256,
            kind: DropFileKind::Firmware {
                profile: profile.name,
                evidence: Evidence::Sha256,
            },
            note: None,
        };
    }

    if let Ok(image) = FirmwareImage::new(bytes)
        && let Ok(found) = m241_core::firmware::identify(&image)
    {
        return DropFileEntry {
            path: path.to_path_buf(),
            size,
            sha256,
            kind: DropFileKind::Firmware {
                profile: found.profile.name,
                evidence: found.evidence,
            },
            note: None,
        };
    }

    DropFileEntry {
        path: path.to_path_buf(),
        size,
        sha256,
        kind: DropFileKind::Unknown,
        note: None,
    }
}

fn evidence_label(evidence: Evidence) -> &'static str {
    match evidence {
        Evidence::Sha256 => "sha256",
        Evidence::Structure => "structure",
    }
}

fn doctor_document(
    ports_result: &Result<Vec<PortEntry>>,
    probe: Option<&ProbeReport>,
    drop: Option<&DropReport>,
) -> serde_json::Value {
    let ports_doc = match ports_result {
        Ok(entries) => {
            let candidate_list = candidates(entries);
            serde_json::json!({
                "status": "ok",
                "error": serde_json::Value::Null,
                "count": entries.len(),
                "candidate_count": candidate_list.len(),
                "candidates": candidate_list,
                "entries": port_entry_documents(entries),
            })
        }
        Err(err) => serde_json::json!({
            "status": "error",
            "error": err.to_string(),
            "count": 0,
            "candidate_count": 0,
            "candidates": [],
            "entries": [],
        }),
    };

    let probe_doc = probe.map(|report| {
        serde_json::json!({
            "requested_port": report.requested_port,
            "resolved_port": report.resolved_port,
            "status": if report.identity.is_some() { "ok" } else { "error" },
            "error": report.error,
            "release": report.identity.as_ref().map(|id| id.release.as_str()),
            "build": report.identity.as_ref().map(|id| id.build.as_str()),
            "model": report.identity.as_ref().map(|id| id.model.as_str()),
            "sdk": report.identity.as_ref().and_then(|id| id.sdk.as_deref()),
            "known_release": report.identity.as_ref().map(|id| is_known_release(&id.release)),
        })
    });

    let drop_doc = drop.map(|report| {
        let files: Vec<serde_json::Value> = report
            .files
            .iter()
            .map(|entry| {
                let (profile, evidence) = match entry.kind {
                    DropFileKind::Firmware { profile, evidence } => {
                        (Some(profile), Some(evidence_label(evidence)))
                    }
                    DropFileKind::DownloadAgent | DropFileKind::Unknown => (None, None),
                };
                serde_json::json!({
                    "path": entry.path.display().to_string(),
                    "size": entry.size,
                    "sha256": entry.sha256,
                    "kind": entry.kind.label(),
                    "profile": profile,
                    "evidence": evidence,
                    "note": entry.note,
                })
            })
            .collect();
        serde_json::json!({
            "directory": report.directory.display().to_string(),
            "file_count": report.files.len(),
            "files": files,
            "download_agent": report.download_agent.as_ref().map(|p| p.display().to_string()),
            "extracted_to": report.extracted_to.as_ref().map(|p| p.display().to_string()),
        })
    });

    serde_json::json!({
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "ports": ports_doc,
        "probe": probe_doc,
        "drop": drop_doc,
    })
}

/// Print the text form of `device doctor`.
///
/// The split follows `docs/reference/02-cli.md` §Output literally: stdout
/// carries the machine-readable findings, stderr carries the problems.
///
/// * A complete report always writes every line to stdout, `=` lines included,
///   so a script can parse `probe : error (...)`. A `--port` that does not answer
///   is a change in the unit's state, not a failed report, and the exit code
///   stays 0 — that is the difference between `doctor` and `device info`, which
///   fails and exits 3.
/// * Anything that went wrong is repeated as a `m241: warning:` line on stderr,
///   so a human reading stderr sees it without reading stdout, and a run **whose
///   report could not even be started** (the OS would not list ports) puts that
///   failure only on stderr.
fn print_doctor_text(
    console: &dyn crate::Console,
    ports_result: &Result<Vec<PortEntry>>,
    probe: Option<&ProbeReport>,
    drop: Option<&DropReport>,
) {
    console.result(&format!(
        "{:<18}: {} ({})",
        "environment",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    console.result(&format!("{:<18}: {}", "version", env!("CARGO_PKG_VERSION")));

    match ports_result {
        Ok(entries) => {
            let candidate_list = candidates(entries);
            console.result(&format!(
                "{:<18}: {} total, {} candidate(s)",
                "ports",
                entries.len(),
                candidate_list.len()
            ));
            if candidate_list.is_empty() {
                let line = format!(
                    "{:<18}: none found (check USB cable, power switch, and OS serial permissions)",
                    "candidates"
                );
                console.result(&line);
                console.warn(&line);
            } else {
                console.result(&format!(
                    "{:<18}: {}",
                    "candidates",
                    candidate_list.join(", ")
                ));
            }
        }
        Err(err) => {
            // There is nothing to count, so this run is a failed check rather
            // than a report with a finding: the reason goes to stderr alone and
            // stdout stops here instead of claiming a port count it does not have.
            console.warn(&format!("could not list serial ports: {err}"));
            return;
        }
    }

    if let Some(report) = probe {
        let port_label = report
            .resolved_port
            .as_deref()
            .unwrap_or(&report.requested_port);
        match &report.identity {
            Some(identity) => {
                console.result(&format!(
                    "{:<18}: ok ({port_label}: {} build {} model {})",
                    "probe", identity.release, identity.build, identity.model
                ));
            }
            None => {
                let err = report.error.as_deref().unwrap_or("probe failed");
                let detail = format!("{port_label}: {err}");
                console.result(&format!("{:<18}: error ({detail})", "probe"));
                console.warn(&format!("the $PMTK605 probe failed ({detail})"));
            }
        }
    }

    if let Some(report) = drop {
        console.result(&format!(
            "{:<18}: {} ({} file(s))",
            "drop",
            report.directory.display(),
            report.files.len()
        ));
        for entry in &report.files {
            let detail = match entry.kind {
                DropFileKind::DownloadAgent => format!("{DA_FILENAME} ({DA_SIZE_BYTES} bytes)"),
                DropFileKind::Firmware { profile, evidence } => {
                    format!("firmware {profile} ({})", evidence_label(evidence))
                }
                DropFileKind::Unknown => entry
                    .note
                    .clone()
                    .unwrap_or_else(|| String::from("unclassified")),
            };
            console.result(&format!(
                "  {:<16}: {} ({} bytes, sha256 {})",
                detail,
                entry.path.display(),
                entry.size,
                entry.sha256
            ));
        }
        if let Some(dest) = &report.extracted_to {
            console.result(&format!("{:<18}: {}", "extracted", dest.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::testing::{AutoConfirm, FAKE_PORT, FakePorts, RecordingConsole, Scratch};
    use m241_core::protocol::simulator::FakeDevice;

    fn run_ports(
        options: &PortsOptions,
        links: &FakePorts,
        console: &RecordingConsole,
    ) -> Result<()> {
        ports(
            options,
            Session::new(links, console, &AutoConfirm).with_timings(Timings::fast()),
        )
    }

    fn run_info(
        options: &InfoOptions,
        links: &FakePorts,
        console: &RecordingConsole,
    ) -> Result<()> {
        info(
            options,
            Session::new(links, console, &AutoConfirm).with_timings(Timings::fast()),
        )
    }

    #[test]
    fn an_empty_port_list_is_success() {
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("empty is not an error");
        assert!(
            console.stdout().contains("no serial ports found"),
            "got {}",
            console.stdout()
        );
        assert_eq!(console.stderr(), "", "nothing to warn about");
    }

    #[test]
    fn a_flagged_port_names_its_chip_and_marks_the_candidate() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("/dev/ttyUSB0"),
            cable: Some(Cable::Cp210x),
            likely: true,
            description: Some(String::from("CP2102 USB to UART Bridge")),
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("ports");
        let text = console.stdout();
        assert!(text.contains("DEVICE"), "got {text}");
        assert!(text.contains("/dev/ttyUSB0"), "got {text}");
        assert!(text.contains("CP210x"), "got {text}");
        assert!(
            text.contains("candidate port(s): /dev/ttyUSB0"),
            "got {text}"
        );
    }

    #[test]
    fn ports_json_is_one_object_with_the_candidate_list() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("COM3"),
            cable: Some(Cable::Ch340),
            likely: true,
            description: None,
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: true }, &links, &console).expect("ports");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("the output is one JSON object");
        assert_eq!(document["ports"][0]["name"], "COM3");
        assert_eq!(document["ports"][0]["cable"], "CH340");
        assert_eq!(document["ports"][0]["description"], serde_json::Value::Null);
        assert_eq!(document["candidates"][0], "COM3");
    }

    #[test]
    fn an_unrelated_port_is_listed_but_not_flagged() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("/dev/ttyS0"),
            cable: None,
            likely: false,
            description: Some(String::from("pci")),
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("ports");
        let text = console.stdout();
        assert!(text.contains("/dev/ttyS0"), "got {text}");
        assert!(text.contains("No obvious M-241 cable found"), "got {text}");
    }

    #[test]
    fn info_reads_the_identity_the_simulated_unit_reports() {
        let links = FakePorts::single(FakeDevice::new);
        let console = RecordingConsole::default();
        run_info(&InfoOptions::default(), &links, &console).expect("identity");
        let text = console.stdout();
        assert!(text.contains("B-core_1.20"), "got {text}");
        assert!(text.contains("01017-00F"), "got {text}");
        assert!(text.contains("0043"), "got {text}");
    }
    #[test]
    fn info_json_is_one_object() {
        let links = FakePorts::single(|| {
            FakeDevice::with_identity("B-core_1.20", "0043", "01017-00F", "1.0")
        });
        let console = RecordingConsole::default();
        run_info(
            &InfoOptions {
                json: true,
                ..InfoOptions::default()
            },
            &links,
            &console,
        )
        .expect("identity");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["release"], "B-core_1.20");
        assert_eq!(document["sdk"], "1.0");
        assert_eq!(document["known_release"], true);
        assert_eq!(document["port"], FAKE_PORT);
    }
    #[test]
    fn an_unknown_release_is_reported_but_not_refused() {
        let links =
            FakePorts::single(|| FakeDevice::with_identity("X-core_9.99", "0001", "99999-00Z", ""));
        let console = RecordingConsole::default();
        run_info(&InfoOptions::default(), &links, &console).expect("still answerable");
        assert!(
            console.stderr().contains("not one this build knows"),
            "got {}",
            console.stderr()
        );
        assert!(console.stdout().contains("X-core_9.99"));
    }
    #[test]
    fn opening_a_device_that_never_answers_is_a_hardware_error() {
        // A unit already in the BootROM answers no PMTK sentence, so the
        // identity request has nothing to read and times out.
        let links = FakePorts::single(|| FakeDevice::new().in_bootrom());
        let console = RecordingConsole::default();
        let err = run_info(&InfoOptions::default(), &links, &console).expect_err("no reply");
        assert!(matches!(err, m241_core::Error::Timeout { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 3);
    }
    #[test]
    fn an_ambiguous_auto_ports_needs_an_explicit_choice() {
        let links = FakePorts::with_ports(vec![
            PortEntry {
                name: String::from("/dev/ttyUSB0"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
            PortEntry {
                name: String::from("/dev/ttyUSB1"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
        ]);
        let console = RecordingConsole::default();
        let err = run_info(&InfoOptions::default(), &links, &console).expect_err("ambiguous");
        assert!(matches!(err, m241_core::Error::Port { .. }), "got {err}");
        assert!(format!("{err}").contains("--port"), "got {err}");
        assert_eq!(console.stdout(), "", "nothing is printed on a failure");
    }
    #[test]
    fn the_known_release_check_accepts_the_family_and_nothing_else() {
        assert!(is_known_release("B-core_1.20"));
        assert!(!is_known_release("X-core_9.99"));
        assert!(!is_known_release(""));
    }

    #[test]
    fn info_accepts_an_explicit_port_name() {
        let links = FakePorts::single(FakeDevice::new);
        let console = RecordingConsole::default();
        run_info(
            &InfoOptions {
                port: String::from(FAKE_PORT),
                json: false,
            },
            &links,
            &console,
        )
        .expect("explicit port");
        let text = console.stdout();
        assert!(text.contains(FAKE_PORT), "got {text}");
        assert!(text.contains("B-core_1.20"), "got {text}");
    }

    #[test]
    fn the_candidate_helper_returns_only_flagged_ports() {
        let entries = vec![
            PortEntry {
                name: String::from("a"),
                cable: None,
                likely: false,
                description: None,
            },
            PortEntry {
                name: String::from("b"),
                cable: Some(Cable::Ftdi),
                likely: true,
                description: None,
            },
        ];
        assert_eq!(candidates(&entries), vec!["b"]);
        assert_eq!(ports_document(&entries)["candidates"][0], "b");
    }

    fn run_doctor(
        options: &DoctorOptions,
        links: &FakePorts,
        console: &RecordingConsole,
    ) -> Result<()> {
        doctor(
            options,
            Session::new(links, console, &AutoConfirm).with_timings(Timings::fast()),
        )
    }

    #[test]
    fn doctor_reports_environment_and_ports_cleanly_when_empty_or_present() {
        let empty_links = FakePorts::empty();
        let console = RecordingConsole::default();
        run_doctor(&DoctorOptions::default(), &empty_links, &console).expect("empty doctor");
        let text = console.stdout();
        assert!(text.contains("environment"), "got {text}");
        assert!(text.contains("0 total, 0 candidate(s)"), "got {text}");

        let with_ports = FakePorts::with_ports(vec![PortEntry {
            name: String::from("/dev/ttyUSB0"),
            cable: Some(Cable::Cp210x),
            likely: true,
            description: Some(String::from("CP2102")),
        }]);
        let console = RecordingConsole::default();
        run_doctor(&DoctorOptions::default(), &with_ports, &console).expect("candidate doctor");
        let text = console.stdout();
        assert!(text.contains("1 total, 1 candidate(s)"), "got {text}");
        assert!(text.contains("/dev/ttyUSB0"), "got {text}");
    }

    #[test]
    fn doctor_reports_the_simulated_probe_identity_on_its_ok_line() {
        let links = FakePorts::single(FakeDevice::new);
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: Some(String::from("auto")),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("probe ok");
        let text = console.stdout();
        // The identity is the simulator's own, not a measurement: SIM0 is the
        // fake port in `crate::testing`, and the strings are what `FakeDevice`
        // answers with.
        assert!(
            text.contains(&format!("ok ({FAKE_PORT}: B-core_1.20")),
            "got {text}"
        );
        assert_eq!(console.stderr(), "", "a clean probe warns about nothing");
    }

    #[test]
    fn a_failed_probe_is_a_finding_on_stdout_and_a_diagnostic_on_stderr() {
        // A unit in the BootROM answers no $PMTK605. Unlike `device info`, which
        // exits 3, `doctor` reports the finding and still exits 0.
        let silent_links = FakePorts::single(|| FakeDevice::new().in_bootrom());

        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: Some(String::from("auto")),
                ..DoctorOptions::default()
            },
            &silent_links,
            &console,
        )
        .expect("probe failure must not crash doctor");
        let text = console.stdout();
        assert!(text.contains("probe             : error ("), "got {text}");
        let stderr = console.stderr();
        assert!(
            stderr.contains(&format!("$PMTK605 probe failed ({FAKE_PORT}: ")),
            "the finding is repeated on stderr: {stderr}"
        );

        // The same finding in `--json`, and still nothing but the object on stdout.
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: Some(String::from("auto")),
                json: true,
                ..DoctorOptions::default()
            },
            &silent_links,
            &console,
        )
        .expect("probe failure must not crash doctor");
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["probe"]["status"], "error");
        assert!(doc["probe"]["error"].is_string(), "got {doc}");
    }

    #[test]
    fn a_port_listing_failure_goes_to_stderr_and_stops_the_report() {
        // `Links` refuses to list at all: the report cannot count ports, so
        // stdout stops before the `ports` line rather than printing a count that
        // was never taken.
        struct FailingPorts;
        impl crate::Links for FailingPorts {
            fn ports(&self) -> Result<Vec<PortEntry>> {
                Err(Error::State(String::from("serial subsystem unavailable")))
            }
            fn open(&self, _name: &str, _baud: u32) -> Result<crate::link::Opened> {
                Err(Error::State(String::from("not reached")))
            }
        }
        let console = RecordingConsole::default();
        doctor(
            &DoctorOptions::default(),
            Session::new(&FailingPorts, &console, &AutoConfirm),
        )
        .expect("a failed check is still a doctor report");
        assert!(
            !console.stdout().contains("ports"),
            "stdout must not carry a port count: {}",
            console.stdout()
        );
        let stderr = console.stderr();
        assert!(
            stderr.contains("could not list serial ports"),
            "got {stderr}"
        );
        assert!(
            stderr.contains("serial subsystem unavailable"),
            "got {stderr}"
        );
    }

    #[test]
    fn doctor_warns_about_an_unknown_release_once_on_stderr() {
        let links =
            FakePorts::single(|| FakeDevice::with_identity("X-core_9.99", "0001", "99999-00Z", ""));
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: Some(String::from("auto")),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("doctor");
        let stderr = console.stderr();
        assert_eq!(
            stderr.matches("not one this build knows").count(),
            1,
            "the warning is emitted once, by the shared helper: {stderr}"
        );
        assert!(
            console.stdout().contains("X-core_9.99"),
            "got {}",
            console.stdout()
        );
    }

    #[test]
    fn doctor_from_drop_identifies_firmware_and_extracts_only_da_with_safety_check() {
        let scratch = Scratch::new("drop");
        let drop_dir = scratch.join("holux_drop");
        let nested_dir = drop_dir.join("nested");
        let extract_dir = scratch.join("vendor");
        std::fs::create_dir_all(&nested_dir).expect("create drop dirs");

        let fw_path = drop_dir.join("m241_v1_13.bin");
        fileio::write(&fw_path, &crate::testing::synthetic_image()).expect("write synthetic fw");
        let da_bytes = vec![0xA5u8; DA_SIZE_BYTES];
        let da_path = nested_dir.join("da_v3.8.4.bin");
        fileio::write(&da_path, &da_bytes).expect("write synthetic da");
        let notes_path = drop_dir.join("readme.txt");
        fileio::write_text(&notes_path, "unrelated text").expect("write notes");

        let links = FakePorts::single(FakeDevice::new);
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: Some(String::from("auto")),
                from_drop: Some(drop_dir.clone()),
                extract_to: Some(extract_dir.clone()),
                force: false,
                json: true,
            },
            &links,
            &console,
        )
        .expect("doctor --from-drop --extract-to --json");

        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["environment"]["os"], std::env::consts::OS);
        assert_eq!(doc["environment"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(doc["ports"]["count"], 1);
        assert_eq!(doc["ports"]["candidate_count"], 1);
        assert_eq!(doc["probe"]["status"], "ok");
        assert_eq!(doc["probe"]["release"], "B-core_1.20");
        assert_eq!(doc["drop"]["file_count"], 3);

        let files = doc["drop"]["files"].as_array().expect("files array");
        let firmware = files
            .iter()
            .find(|item| item["kind"] == "firmware")
            .unwrap_or_else(|| panic!("no firmware entry in {doc}"));
        assert_eq!(firmware["profile"], "V1.13");
        assert_eq!(firmware["evidence"], "structure");
        // Pinned digest, so a scan that stopped hashing (or hashed the wrong
        // bytes) cannot pass on the other fields alone.
        assert_eq!(
            firmware["sha256"],
            sha256_hex(&crate::testing::synthetic_image()),
            "the digest is the SHA-256 of the bytes on disk"
        );
        let da = files
            .iter()
            .find(|item| item["kind"] == "download_agent")
            .unwrap_or_else(|| panic!("no download-agent entry in {doc}"));
        assert_eq!(da["size"], DA_SIZE_BYTES);
        assert_eq!(da["sha256"], sha256_hex(&da_bytes));
        let unknown = files
            .iter()
            .find(|item| item["kind"] == "unknown")
            .unwrap_or_else(|| panic!("no unknown entry in {doc}"));
        assert_eq!(
            unknown["sha256"],
            sha256_hex(b"unrelated text"),
            "an unclassified file is still hashed"
        );

        // Only DA_v3.8.4.bin was copied to extract_dir.
        let extracted_da = extract_dir.join(DA_FILENAME);
        assert_eq!(fileio::read(&extracted_da).expect("extracted da"), da_bytes);
        assert!(!extract_dir.join("m241_v1_13.bin").exists());
        assert!(!extract_dir.join("readme.txt").exists());

        // Second run without --force refuses overwrite with Error::Safety (exit 5).
        let console = RecordingConsole::default();
        let err = run_doctor(
            &DoctorOptions {
                port: None,
                from_drop: Some(drop_dir.clone()),
                extract_to: Some(extract_dir.clone()),
                force: false,
                json: false,
            },
            &links,
            &console,
        )
        .expect_err("must refuse overwrite without --force");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5);

        // With --force, overwrite succeeds.
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                port: None,
                from_drop: Some(drop_dir),
                extract_to: Some(extract_dir),
                force: true,
                json: false,
            },
            &links,
            &console,
        )
        .expect("--force allows overwrite");
    }

    /// What a directory tree holds, for "nothing changed" checks.
    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push((path.clone(), fileio::read(&path).expect("read file")));
                }
            }
        }
        files.sort_by(|(left, _), (right, _)| left.cmp(right));
        files
    }

    #[test]
    fn a_wrong_sized_download_agent_is_unknown_and_extraction_is_refused() {
        // M1: the 14,896-byte identification is the only thing separating a DA
        // from a file that merely has its name. One byte over must not be
        // extracted; it must be reported as an unknown file with the reason.
        let scratch = Scratch::new("da-wrong-size");
        let drop_dir = scratch.join("holux_drop");
        let extract_dir = scratch.join("vendor");
        std::fs::create_dir_all(&drop_dir).expect("create drop dir");

        let wrong_bytes = vec![0xA5u8; DA_SIZE_BYTES + 1];
        fileio::write(&drop_dir.join(DA_FILENAME), &wrong_bytes).expect("write wrong-sized da");

        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let err = run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir.clone()),
                extract_to: Some(extract_dir.clone()),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect_err("a DA of the wrong size must not be extracted");
        assert!(matches!(err, Error::State(_)), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 1);
        assert!(
            format!("{err}").contains(&DA_SIZE_BYTES.to_string()),
            "the refusal names the size it expected: {err}"
        );
        assert!(!extract_dir.exists(), "nothing was written");

        // The inventory itself says why the file was not accepted.
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir),
                json: true,
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("the inventory still runs");
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert!(doc["drop"]["download_agent"].is_null(), "got {doc}");
        assert_eq!(doc["drop"]["files"][0]["kind"], "unknown", "got {doc}");
        let note = doc["drop"]["files"][0]["note"].as_str().unwrap_or("");
        assert!(note.contains(&DA_SIZE_BYTES.to_string()), "got {note}");
    }

    #[test]
    fn extract_to_without_a_download_agent_in_the_drop_is_refused() {
        // S1: the failure `docs/reference/02-cli.md` documents — `--extract-to`
        // with a drop that holds no identified DA is `Error::State`, not an
        // empty directory quietly created.
        let scratch = Scratch::new("no-da");
        let drop_dir = scratch.join("holux_drop");
        let extract_dir = scratch.join("vendor");
        std::fs::create_dir_all(&drop_dir).expect("create drop dir");
        fileio::write_text(&drop_dir.join("readme.txt"), "nothing to extract").expect("write");

        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let err = run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir),
                extract_to: Some(extract_dir.clone()),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect_err("no DA means nothing to copy");
        assert!(matches!(err, Error::State(_)), "got {err}");
        assert!(format!("{err}").contains(DA_FILENAME), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 1);
        assert!(!extract_dir.exists());
    }

    #[test]
    fn the_drop_scan_stops_one_level_past_max_drop_depth() {
        // S3 / R18: the scan reads files up to four directory levels below
        // `--from-drop` (`holux_drop/d1/d2/d3/d4/` is the deepest that is read)
        // and stops at the fifth. Keying a file to a specific depth is what makes
        // the test catch a bound that is off by one in either direction: a
        // marker proving the bound was met, and a file proving it was not passed.
        let scratch = Scratch::new("drop-depth");
        let drop_dir = scratch.join("holux_drop");
        let mut level = drop_dir.clone();
        for depth in 1..=6 {
            level = level.join(format!("d{depth}"));
            std::fs::create_dir_all(&level).expect("create level");
            if depth == 5 {
                fileio::write_text(&level.join("beyond.txt"), "x").expect("write file");
            }
        }
        fileio::write_text(&level.join("deepest_file.txt"), "x").expect("write");
        let at_the_bound = drop_dir.join("d1").join("d2").join("d3").join("d4");
        fileio::write_text(&at_the_bound.join("bound.txt"), "x").expect("write bound marker");

        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir),
                json: true,
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("depth-limited scan");
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        let listed: Vec<String> = doc["drop"]["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|item| item["path"].as_str().unwrap_or("").to_owned())
            .collect();
        assert!(
            listed
                .iter()
                .any(|path| Path::new(path).ends_with(Path::new("d4").join("bound.txt"))),
            "four levels below the root is read: {listed:?}"
        );
        assert!(
            !listed.iter().any(|path| path.ends_with("beyond.txt")),
            "five levels below the root is not read: {listed:?}"
        );
        assert!(
            !listed.iter().any(|path| path.ends_with("deepest_file.txt")),
            "six levels below the root is not read: {listed:?}"
        );
        assert_eq!(listed.len(), 1, "got {listed:?}");
        assert!(
            console.stderr().contains("deeper than 4 levels"),
            "the truncation is reported: {}",
            console.stderr()
        );

        // A tree exactly MAX_DROP_DEPTH deep is read in full and silent.
        let shallow = scratch.join("shallow");
        let mut level = shallow.clone();
        for depth in 1..=MAX_DROP_DEPTH {
            level = level.join(format!("d{depth}"));
        }
        std::fs::create_dir_all(&level).expect("create deep level");
        fileio::write_text(&level.join("deep.txt"), "x").expect("write file");
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                from_drop: Some(shallow),
                json: true,
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("scan at the limit");
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["drop"]["file_count"], 1, "got {doc}");
        assert_eq!(console.stderr(), "", "no truncation to report");
    }

    #[test]
    fn a_drop_scan_leaves_the_drop_directory_untouched() {
        // S4: `--from-drop` is documented as a read-only scan, so the input tree
        // must be byte-identical afterwards, with and without `--extract-to`.
        let scratch = Scratch::new("drop-readonly");
        let drop_dir = scratch.join("holux_drop");
        let nested = drop_dir.join("tool").join("da");
        std::fs::create_dir_all(&nested).expect("create drop dirs");
        fileio::write(
            &drop_dir.join("m241_v1_13.bin"),
            &crate::testing::synthetic_image(),
        )
        .expect("write firmware");
        let da_bytes = vec![0x5Au8; DA_SIZE_BYTES];
        fileio::write(&nested.join(DA_FILENAME), &da_bytes).expect("write da");
        fileio::write_text(&drop_dir.join("readme.txt"), "drop notes").expect("write notes");
        let before = tree_snapshot(&drop_dir);

        let links = FakePorts::empty();
        for extract_to in [None, Some(scratch.join("vendor"))] {
            let console = RecordingConsole::default();
            run_doctor(
                &DoctorOptions {
                    from_drop: Some(drop_dir.clone()),
                    extract_to,
                    ..DoctorOptions::default()
                },
                &links,
                &console,
            )
            .expect("doctor --from-drop");
        }

        assert_eq!(tree_snapshot(&drop_dir), before, "the drop was modified");
    }

    #[test]
    fn a_drop_scan_does_not_create_the_extract_directory_it_only_copies_one_file() {
        // S4 / S1, from the outside: the destination is created only when there
        // is something to copy into it, and then it holds the DA and nothing else.
        let scratch = Scratch::new("extract-creates");
        let drop_dir = scratch.join("holux_drop");
        std::fs::create_dir_all(&drop_dir).expect("create drop dir");
        fileio::write(&drop_dir.join(DA_FILENAME), &vec![0x11u8; DA_SIZE_BYTES]).expect("write da");

        // Read-only pass: no destination is created.
        let only_report = scratch.join("never_created");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir.clone()),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("read-only scan");
        assert!(!only_report.exists(), "no --extract-to, no directory");
        assert!(
            !scratch.join("vendor").exists(),
            "no --extract-to, no directory"
        );

        // Extraction pass: the destination appears, holding only the DA.
        let extract_dir = scratch.join("vendor");
        let console = RecordingConsole::default();
        run_doctor(
            &DoctorOptions {
                from_drop: Some(drop_dir),
                extract_to: Some(extract_dir.clone()),
                ..DoctorOptions::default()
            },
            &links,
            &console,
        )
        .expect("extract");
        let written: Vec<String> = std::fs::read_dir(&extract_dir)
            .expect("destination created")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(written, vec![String::from(DA_FILENAME)], "got {written:?}");
        // The bytes crossed, not just the name: `drop(fileio::write(..))`
        // leaves an empty file that a name-only assertion would accept.
        assert_eq!(
            fileio::read(&extract_dir.join(DA_FILENAME)).expect("read extracted"),
            vec![0x11u8; DA_SIZE_BYTES],
            "the extracted DA is the drop's bytes"
        );
    }
}
