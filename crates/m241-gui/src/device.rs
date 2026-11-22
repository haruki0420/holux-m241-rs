//! Background device jobs for the screens.
//!
//! Started by the Firmware screen (dump, write, recovery, restart, BootROM
//! re-entry) and by the Connect screen (probe-da, `0xD9` restart, doctor).
//!
//! Every job opens the port inside its own thread and reports through a channel,
//! so a 35-second read-back cannot freeze the window. The jobs call
//! [`m241_flash::flow`] and nothing else: the acceptance rules, the fixed range
//! and the typed confirmation live in the write crate, and this module is not a
//! second implementation of them.
//!
//! A job carries file paths, not bytes it read itself, because the files are
//! what a user can check and what the CLI takes. A failed read is reported as a
//! job failure, never as "no bytes to write".

use std::sync::mpsc::{self, Receiver, Sender};

use m241_core::DeviceInfo;
use m241_core::protocol::Port;
use m241_core::protocol::brom::DEFAULT_BOOTROM_BAUD;
use m241_core::protocol::pmtk::NMEA_BAUD;
use m241_core::protocol::pmtk::PmtkClient;
use m241_core::{Error, Result};
use m241_flash::da::{Confirmation, FLASH_WRITE_LENGTH, WriteReport};
use m241_flash::flow::{self, Event, FlashRequest, ProbeDaReport};
use m241_flash::{
    DaDigest, DownloadAgent, accept_vendor_image, check_image, write_plan, write_plan_vendor,
};

/// One write's inputs, already chosen on the screen.
#[derive(Debug, Clone)]
pub struct FlashJob {
    /// Serial port name.
    pub port: String,
    /// The user's own dump.
    pub backup: String,
    /// The image to write (the backup itself for a recovery).
    pub image: String,
    /// The pinned vendor `DA_v3.8.4.bin`.
    pub da: String,
    /// The typed confirmation; only [`Confirmation::granted`] is constructible.
    pub confirmation: Confirmation,
    /// Write the backup back, markers only (`--recover`).
    pub recover: bool,
    /// The image is the pinned vendor release (`--allow-vendor`).
    pub vendor: bool,
    /// Take the `--assume-brom` entry route.
    pub assume_brom: bool,
}

/// A job the screen can start.
#[derive(Debug, Clone)]
pub enum Job {
    /// Read the firmware range and save it to a file.
    Dump {
        /// Serial port name.
        port: String,
        /// Where to write the image.
        output: String,
        /// Take the `--assume-brom` entry route.
        assume_brom: bool,
    },
    /// Write a range and read it back.
    Flash(FlashJob),
    /// Send `0xD9` without waiting for a reply.
    Restart {
        /// Serial port name.
        port: String,
    },
    /// Enter the BootROM and sync, to see whether the unit answers.
    EnterBootRom {
        /// Serial port name.
        port: String,
        /// Take the `--assume-brom` route instead of the NMEA hand-over.
        assume_brom: bool,
    },
    /// Load the DA into RAM and read both candidate addresses (`fw probe-da`).
    ProbeDa {
        /// Serial port name.
        port: String,
        /// The pinned DA file.
        da: String,
        /// The typed acknowledgement; a DA that runs can program flash.
        confirmation: Confirmation,
        /// Take the `--assume-brom` entry route.
        assume_brom: bool,
    },
    /// `device doctor`: list ports, probe one, and inspect a vendor drop.
    Doctor {
        /// The port to probe, when one was selected.
        port: Option<String>,
        /// The directory to scan, when one was given.
        drop_dir: String,
        /// Where a found DA should be copied, when a path was given.
        extract_to: String,
    },
}

/// One serial port and whether it looks like an M-241 cable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortFinding {
    /// The OS name.
    pub name: String,
    /// Whether the name is one of the USB-serial patterns.
    pub likely: bool,
}

/// One file in a vendor drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropFile {
    /// Path as scanned.
    pub path: String,
    /// Size in bytes.
    pub size: usize,
    /// What it was identified as.
    pub kind: String,
}

/// What `device doctor` found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorReport {
    /// Serial ports on this machine.
    pub ports: Vec<PortFinding>,
    /// The probed unit's identity, when it answered.
    pub identity: Option<DeviceInfo>,
    /// Why the probe failed, when it did.
    pub probe_error: Option<String>,
    /// The scanned directory, when one was given.
    pub drop_dir: String,
    /// Files found in it.
    pub files: Vec<DropFile>,
    /// The DA file found, when one was.
    pub da_source: Option<String>,
    /// Where the DA was copied, when it was.
    pub extracted_to: Option<String>,
    /// Why the drop scan failed, when it did.
    pub drop_error: Option<String>,
}

/// What a [`Job`] reports.
#[derive(Debug)]
pub enum JobEvent {
    /// A stage or progress report from [`m241_flash::flow`].
    Flow(Event),
    /// A dump finished; the image was written to the job's output path.
    Dumped(Vec<u8>),
    /// The write's read-back matched.
    WriteDone(WriteReport),
    /// The recovery write's read-back matched.
    RecoverDone(WriteReport),
    /// `0xD9` was written (it has no reply).
    Restarted,
    /// The BootROM answered the sync.
    BootRomAnswered,
    /// `fw probe-da` finished.
    ProbeDone(ProbeDaReport),
    /// `device doctor` finished.
    DoctorDone(DoctorReport),
    /// The job failed; the message is the device's own words where it has them.
    Failed(String),
}

/// Open the port at the baud the chosen entry route expects.
fn open(port: &str, assume_brom: bool) -> Result<Port> {
    let baud = if assume_brom {
        DEFAULT_BOOTROM_BAUD
    } else {
        NMEA_BAUD
    };
    Port::open(port.trim(), baud)
}

/// Read a file, keeping the path in the error.
fn read_file(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path.trim()).map_err(|source| Error::Io {
        path: Some(path.trim().to_owned()),
        source,
    })
}

/// The pre-flight checks that need no port: the same acceptance calls the CLI
/// makes before it opens one.
fn preflight(job: &FlashJob) -> Result<Vec<u8>> {
    let backup = read_file(&job.backup)?;
    let image = read_file(&job.image)?;
    if backup.len() != FLASH_WRITE_LENGTH {
        return Err(Error::Safety {
            reason: format!(
                "the backup is {} bytes; the shipped write range is {FLASH_WRITE_LENGTH}",
                backup.len()
            ),
        });
    }
    if job.recover && backup != image {
        return Err(Error::Safety {
            reason: String::from(
                "recovery writes the backup itself back over the unit, so its image must be \
                 byte-identical to the backup",
            ),
        });
    }
    let da = read_file(&job.da)?;
    flow::check_da(&da)?;
    if job.vendor {
        let token = accept_vendor_image(&m241_core::firmware::PROFILE_V1_13, &image)?;
        write_plan_vendor(&m241_core::firmware::PROFILE_V1_13, &backup, token)?;
    } else if job.recover {
        check_image(&m241_core::firmware::PROFILE_V1_13, &backup, &backup)?;
    } else {
        write_plan(&m241_core::firmware::PROFILE_V1_13, &backup, &image)?;
    }
    Ok(da)
}

/// Run one job to completion, reporting through `tx`.
fn run(job: Job, tx: &Sender<JobEvent>) -> Result<()> {
    match job {
        Job::Dump {
            port,
            output,
            assume_brom,
        } => {
            let path = output.trim();
            if path.is_empty() {
                return Err(Error::State(String::from(
                    "no dump path: type where the image should be saved",
                )));
            }
            if std::path::Path::new(path).exists() {
                return Err(Error::Safety {
                    reason: format!(
                        "{path} already exists; the backup is the only way back, so this job will \
                         not overwrite it. Choose a new file name"
                    ),
                });
            }
            let transport = open(&port, assume_brom)?;
            let image = flow::dump(transport, assume_brom, &mut |event| {
                let _ = tx.send(JobEvent::Flow(event));
            })?;
            std::fs::write(path, &image).map_err(|source| Error::Io {
                path: Some(path.to_owned()),
                source,
            })?;
            let _ = tx.send(JobEvent::Dumped(image));
            Ok(())
        }
        Job::Flash(job) => {
            let da = preflight(&job)?;
            let backup = read_file(&job.backup)?;
            let image = read_file(&job.image)?;
            let transport = open(&job.port, job.assume_brom)?;
            let vendor = if job.vendor {
                Some(accept_vendor_image(
                    &m241_core::firmware::PROFILE_V1_13,
                    &image,
                )?)
            } else {
                None
            };
            let request = FlashRequest {
                backup: &backup,
                image: if job.recover { &backup } else { &image },
                da: &da,
                da_digest: DaDigest::vendor(),
                confirmation: job.confirmation,
                recover: job.recover,
                assume_brom: job.assume_brom,
                vendor,
            };
            let report = flow::flash(transport, &request, &mut |event| {
                let _ = tx.send(JobEvent::Flow(event));
            })?;
            let _ = tx.send(if job.recover {
                JobEvent::RecoverDone(report)
            } else {
                JobEvent::WriteDone(report)
            });
            Ok(())
        }
        Job::Restart { port } => {
            let transport = Port::open(port.trim(), DEFAULT_BOOTROM_BAUD)?;
            DownloadAgent::new(transport).restart()?;
            let _ = tx.send(JobEvent::Restarted);
            Ok(())
        }
        Job::EnterBootRom { port, assume_brom } => {
            let transport = open(&port, assume_brom)?;
            flow::probe_bootrom(transport, assume_brom)?;
            let _ = tx.send(JobEvent::BootRomAnswered);
            Ok(())
        }
        Job::ProbeDa {
            port,
            da,
            confirmation,
            assume_brom,
        } => {
            let da = read_file(&da)?;
            flow::check_da(&da)?;
            let transport = open(&port, assume_brom)?;
            let report = flow::probe_da(transport, &da, confirmation, assume_brom, &mut |event| {
                let _ = tx.send(JobEvent::Flow(event));
            })?;
            let _ = tx.send(JobEvent::ProbeDone(report));
            Ok(())
        }
        Job::Doctor {
            port,
            drop_dir,
            extract_to,
        } => {
            let report = doctor(port.as_deref(), &drop_dir, &extract_to);
            let _ = tx.send(JobEvent::DoctorDone(report));
            Ok(())
        }
    }
}

/// Whether a port name looks like a USB-serial bridge a logger would use.
fn looks_like_cable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["ttyusb", "ttyacm", "usbserial", "usbmodem", "com"]
        .iter()
        .any(|hint| lower.contains(hint))
}

/// Read one file from a vendor drop and say what it looks like.
fn classify_drop(path: &std::path::Path, size: usize) -> String {
    if size == m241_flash::da::DA_IMAGE_SIZE {
        return String::from("DA_v3.8.4.bin（DA）");
    }
    // A drop can contain anything; do not read a multi-gigabyte file into
    // memory to decide it is not firmware. The largest known release is under
    // half a megabyte.
    if size > 1 << 20 {
        return String::from("大きすぎてファームとして読みません");
    }
    let Ok(bytes) = std::fs::read(path) else {
        return String::from("読めません");
    };
    if let Ok(image) = m241_core::firmware::FirmwareImage::new(&bytes)
        && let Ok(found) = m241_core::firmware::identify(&image)
    {
        return format!("ファーム（{}）", found.profile.name);
    }
    if bytes.starts_with(&[0x00, 0x00, 0xA0, 0xE1]) {
        return String::from("DA の可能性（先頭バイト一致・サイズ違い）");
    }
    String::from("不明")
}

/// Walk `dir` two levels deep and collect regular files.
fn collect_drop(
    dir: &std::path::Path,
    depth: usize,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    if depth > 2 {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: Some(dir.display().to_string()),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: Some(dir.display().to_string()),
            source,
        })?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_drop(&path, depth + 1, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// The `device doctor` report: ports, one probe, and the drop scan.
///
/// Every failure is recorded in the report rather than ending it: doctor is the
/// command that is supposed to say what is broken, and a report that stops at
/// the first problem is the one thing it must not do.
fn doctor(port: Option<&str>, drop_dir: &str, extract_to: &str) -> DoctorReport {
    let mut report = DoctorReport {
        drop_dir: String::from(drop_dir.trim()),
        ..DoctorReport::default()
    };
    match Port::list() {
        Ok(ports) => {
            report.ports = ports
                .into_iter()
                .map(|name| PortFinding {
                    likely: looks_like_cable(&name),
                    name,
                })
                .collect();
        }
        Err(err) => report.probe_error = Some(format!("ポートを列挙できません: {err}")),
    }
    if let Some(port) = port.map(str::trim).filter(|port| !port.is_empty()) {
        match Port::open(port, NMEA_BAUD) {
            Ok(port) => {
                let mut client = PmtkClient::new(port);
                match client.request_identity() {
                    Ok(identity) => report.identity = Some(identity),
                    Err(err) => report.probe_error = Some(err.to_string()),
                }
            }
            Err(err) => report.probe_error = Some(err.to_string()),
        }
    }
    if !drop_dir.trim().is_empty() {
        let dir = std::path::Path::new(drop_dir.trim());
        let mut paths = Vec::new();
        match collect_drop(dir, 0, &mut paths) {
            Ok(()) => {
                for path in paths {
                    let size = std::fs::metadata(&path)
                        .map(|meta| usize::try_from(meta.len()).unwrap_or(usize::MAX))
                        .unwrap_or(0);
                    let kind = classify_drop(&path, size);
                    if report.da_source.is_none()
                        && kind.contains("DA")
                        && size == m241_flash::da::DA_IMAGE_SIZE
                    {
                        report.da_source = Some(path.display().to_string());
                    }
                    report.files.push(DropFile {
                        path: path.display().to_string(),
                        size,
                        kind,
                    });
                }
            }
            Err(err) => report.drop_error = Some(err.to_string()),
        }
    }
    if !extract_to.trim().is_empty() {
        if let Some(source) = &report.da_source {
            let dest = std::path::Path::new(extract_to.trim()).join("DA_v3.8.4.bin");
            if dest.exists() {
                report.drop_error = Some(format!(
                    "{} は既にあります。上書きしません。別の抽出先を指定してください",
                    dest.display()
                ));
                return report;
            }
            match std::fs::copy(source, &dest) {
                Ok(_) => report.extracted_to = Some(dest.display().to_string()),
                Err(source_err) => {
                    report.drop_error = Some(format!(
                        "{} にコピーできません: {source_err}",
                        dest.display()
                    ));
                }
            }
        } else {
            report.drop_error = Some(String::from(
                "DA_v3.8.4.bin（14,896 バイト）が指定ディレクトリに見つかりません",
            ));
        }
    }
    report
}

/// Start `job` on its own thread and return the channel it reports on.
///
/// The receiver dropping is cancellation: the worker's sends fail and its work
/// is abandoned. Nothing here cancels a write mid-packet, which is the safe
/// direction.
pub fn start(job: Job) -> Receiver<JobEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(err) = run(job, &tx) {
            let _ = tx.send(JobEvent::Failed(err.to_string()));
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_backup_is_reported_before_any_port_is_opened() {
        let job = FlashJob {
            port: String::from("/dev/does-not-exist"),
            backup: String::from("/tmp/m241-no-such-backup.bin"),
            image: String::from("/tmp/m241-no-such-image.bin"),
            da: String::from("/tmp/m241-no-such-da.bin"),
            confirmation: Confirmation::granted(),
            recover: false,
            vendor: false,
            assume_brom: false,
        };
        let error = preflight(&job).expect_err("a missing backup must be refused");
        assert!(
            error.to_string().contains("m241-no-such-backup"),
            "the refusal does not name the missing file: {error}"
        );
    }

    #[test]
    fn recovery_refuses_an_image_that_is_not_the_backup() {
        let dir = std::env::temp_dir().join(format!("m241-gui-job-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let backup = dir.join("backup.bin");
        let image = dir.join("image.bin");
        let data = vec![0u8; FLASH_WRITE_LENGTH];
        std::fs::write(&backup, &data).expect("write backup");
        let mut other = data.clone();
        other[0] = 1;
        std::fs::write(&image, &other).expect("write image");
        let job = FlashJob {
            port: String::from("/dev/does-not-exist"),
            backup: backup.display().to_string(),
            image: image.display().to_string(),
            da: String::from("/dev/null"),
            confirmation: Confirmation::granted(),
            recover: true,
            vendor: false,
            assume_brom: true,
        };
        let error = preflight(&job).expect_err("recovery must require the backup itself");
        assert!(
            error.to_string().contains("byte-identical"),
            "the refusal does not say the image must be the backup: {error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_port_that_looks_like_a_usb_bridge_is_flagged() {
        assert!(looks_like_cable("/dev/ttyUSB0"));
        assert!(looks_like_cable("COM3"));
        assert!(!looks_like_cable("/dev/ttyS0"));
    }

    #[test]
    fn doctor_will_not_overwrite_an_existing_da_copy() {
        let root = std::env::temp_dir().join(format!("m241-doctor-{}", std::process::id()));
        let drop_dir = root.join("drop");
        let extract = root.join("out");
        std::fs::create_dir_all(&drop_dir).expect("drop dir");
        std::fs::create_dir_all(&extract).expect("extract dir");
        let source = drop_dir.join("DA_v3.8.4.bin");
        std::fs::write(&source, vec![0u8; m241_flash::da::DA_IMAGE_SIZE]).expect("write DA");
        let dest = extract.join("DA_v3.8.4.bin");
        std::fs::write(&dest, b"keep me").expect("write existing dest");

        let report = doctor(
            None,
            &drop_dir.display().to_string(),
            &extract.display().to_string(),
        );
        assert_eq!(report.files.len(), 1);
        assert!(report.da_source.is_some(), "the DA was not recognised");
        assert!(
            report
                .drop_error
                .as_deref()
                .is_some_and(|reason| reason.contains("既にあります")),
            "an existing destination was overwritten: {:?}",
            report.drop_error
        );
        assert_eq!(std::fs::read(&dest).expect("keep me"), b"keep me");
        let _ = std::fs::remove_dir_all(&root);
    }
}
