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

/// Expected byte length of `DA_v3.8.4.bin` (`docs/reference/01-da-protocol.md`).
const DA_SIZE_BYTES: usize = 14_896;

/// Maximum directory recursion depth when scanning `--from-drop`.
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
        return session.console.json(&document(&entries));
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
pub fn candidates(entries: &[PortEntry]) -> Vec<&str> {
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
pub fn document(entries: &[PortEntry]) -> serde_json::Value {
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

    if !is_known_release(&identity.release) {
        session.console.warn(&format!(
            "the release {:?} is not one this build knows; the patch path decides on the \
             image's bytes rather than on this string",
            identity.release
        ));
    }

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
pub fn is_known_release(release: &str) -> bool {
    release.starts_with("B-core_1.")
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
            if !is_known_release(&identity.release) {
                session.console.warn(&format!(
                    "the release {:?} is not one this build knows; the patch path decides on the \
                     image's bytes rather than on this string",
                    identity.release
                ));
            }
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
                console.result(&format!(
                    "{:<18}: none found (check USB cable, power switch, and OS serial permissions)",
                    "candidates"
                ));
            } else {
                console.result(&format!(
                    "{:<18}: {}",
                    "candidates",
                    candidate_list.join(", ")
                ));
            }
        }
        Err(err) => {
            console.result(&format!("{:<18}: error: {err}", "ports"));
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
                console.result(&format!("{:<18}: error ({port_label}: {err})", "probe"));
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
    use crate::testing::{AutoConfirm, FAKE_PORT, FakePorts, RecordingConsole};
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
        assert_eq!(document(&entries)["candidates"][0], "b");
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("m241-cli-doctor-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
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
    fn doctor_probes_device_and_records_probe_failure_without_crashing() {
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
        assert!(text.contains("ok (SIM0: B-core_1.20"), "got {text}");

        // A unit in BootROM does not answer $PMTK605: doctor reports error cleanly.
        let silent_links = FakePorts::single(|| FakeDevice::new().in_bootrom());
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
        assert!(files.iter().any(|item| {
            item["kind"] == "firmware"
                && item["profile"] == "V1.13"
                && item["evidence"] == "structure"
        }));
        assert!(
            files
                .iter()
                .any(|item| { item["kind"] == "download_agent" && item["size"] == DA_SIZE_BYTES })
        );
        assert!(files.iter().any(|item| item["kind"] == "unknown"));

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
}
