//! Port enumeration, device probes, log downloads, and firmware operations.
//!
//! Everything that opens a serial port lives here, on the Rust side, and passes
//! through [`open_port`]. The frontend only passes a port name string (including
//! manual paths such as Bluetooth SPP `/dev/rfcomm0`); WebView code has no
//! serial access and the capability set does not grant one.
//!
//! Write and DA-probe operations delegate to [`m241_flash::flow`], reusing the
//! same layout, image, backup, and read-back checks as the CLI. Destructive
//! opcodes never appear in this crate (`tools/check_safety.py`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use m241_core::firmware::profiles::{PROFILE_V1_12, PROFILE_V1_13, identify};
use m241_core::firmware::{self, FirmwareImage, GpsEpoch, sha256_hex};
use m241_core::protocol::Port;
use m241_core::protocol::brom::DEFAULT_BOOTROM_BAUD;
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};
use m241_core::protocol::port::Transport;
use m241_core::{DeviceInfo as CoreDeviceInfo, Error, FirmwareProfile, Result};
use m241_flash::da::{
    Confirmation, DaDigest, ImageClassification, VECTOR, VENDOR_PER_UNIT_DATA_BYTES,
    accept_unverified_image, accept_vendor_image, check_image, check_layout, check_physical_image,
    classify_image, profile_patch_offsets, write_plan, write_plan_vendor,
};
use m241_flash::flow::{self, FlashRequest};

/// Typed confirmation word required to start [`fw_flash`].
pub const WRITE_CONFIRMATION: &str = "WRITE";

/// Typed confirmation word required to start [`fw_probe_da`].
pub const PROBE_CONFIRMATION: &str = "PROBE";

/// The USB-serial bridges a Holux cable is known to appear as.
///
/// The vendor ids are the ones `m241-cli`'s `link::identify` uses; keeping the
/// list here rather than making the GUI depend on the CLI keeps the two
/// binaries independent, and the values are USB ids rather than logic that can
/// drift.
const KNOWN_VIDS: [u16; 4] = [
    0x10C4, // Silicon Labs CP210x — the genuine Holux cable
    0x067B, // Prolific PL2303
    0x0403, // FTDI FT232
    0x1A86, // WCH CH340 / CH341
];

/// One serial port, as the frontend's port picker needs it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    /// The OS name: `/dev/ttyUSB0` on Linux, `COM3` on Windows.
    pub name: String,
    /// The OS description, when it has one.
    pub description: Option<String>,
    /// The port family the OS reported: `usb`, `pci`, `bluetooth` or `unknown`.
    pub kind: String,
    /// Whether the USB ids (or the description) look like an M-241 cable.
    pub likely: bool,
}

/// What the frontend receives after a successful `device_info` call.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// The port that was opened, echoed so a late reply cannot be attributed to
    /// a different selection.
    pub port: String,
    /// Release name, e.g. `B-core_1.20`.
    pub release: String,
    /// Build number as the firmware reports it.
    pub build: String,
    /// Model or product id, e.g. `01017-00F`.
    pub model: String,
    /// SDK version, when the firmware answers with one.
    pub sdk: Option<String>,
    /// Whether the release names a firmware family this build knows. A warning
    /// for the operator, never a refusal.
    pub known_release: bool,
    /// Bytes of track log the unit reports in use (`$PMTK182,3,8`).
    pub log_used_bytes: u64,
    /// Whether the unit is currently recording. `None` when the optional
    /// `$PMTK182,2,1` request failed; the rest of the card is still valid.
    pub log_enabled: Option<bool>,
}

/// List the serial ports, likely cables first.
///
/// An enumeration failure yields an empty list rather than a panic: the screen
/// then says no ports were found and offers a manual entry, which is the same
/// recovery a genuinely empty machine gets. The OS error itself is not useful
/// in the picker and there is no second channel for it in the P0 contract.
#[must_use]
pub fn list_ports() -> Vec<PortInfo> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    let mut entries: Vec<PortInfo> = ports.into_iter().map(describe_port).collect();
    // Stable sort: likely cables first, the OS's own order kept otherwise.
    entries.sort_by_key(|entry| !entry.likely);
    entries
}

fn describe_port(port: serialport::SerialPortInfo) -> PortInfo {
    let name = port.port_name;
    match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            let description = usb.product.clone().or_else(|| usb.manufacturer.clone());
            let likely = looks_like_cable(Some(usb.vid), Some(usb.pid), description.as_deref());
            PortInfo {
                name,
                description,
                kind: String::from("usb"),
                likely,
            }
        }
        other => {
            let kind = match other {
                serialport::SerialPortType::PciPort => "pci",
                serialport::SerialPortType::BluetoothPort => "bluetooth",
                _ => "unknown",
            };
            PortInfo {
                name,
                description: None,
                kind: String::from(kind),
                likely: false,
            }
        }
    }
}

/// Whether a port's USB ids or description identify it as a logger cable.
///
/// The ids decide when they exist; the description is the fallback for a port
/// the OS names but cannot attribute to a USB vendor.
#[must_use]
pub fn looks_like_cable(vid: Option<u16>, _pid: Option<u16>, description: Option<&str>) -> bool {
    if let Some(vid) = vid
        && KNOWN_VIDS.contains(&vid)
    {
        return true;
    }
    let Some(description) = description else {
        return false;
    };
    let lowered = description.to_ascii_lowercase();
    [
        "holux",
        "cp210",
        "silicon labs",
        "prolific",
        "ftdi",
        "ch340",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// Open `port_name` through the crate's single serial entry point.
fn open_port(port_name: &str, baud: u32) -> Result<Port> {
    let trimmed = port_name.trim();
    if trimmed.is_empty() {
        return Err(Error::Port {
            message: String::from("no port name was given"),
        });
    }
    Port::open(trimmed, baud)
}

fn entry_baud(assume_brom: bool) -> u32 {
    if assume_brom {
        DEFAULT_BOOTROM_BAUD
    } else {
        NMEA_BAUD
    }
}

/// Open `port_name` at the NMEA baud and read the identity and log usage.
///
/// # Errors
///
/// [`m241_core::Error`] from opening the port or from either read-only request.
/// The port is closed when the function returns: the client owns it.
pub fn probe(port_name: &str) -> Result<DeviceInfo> {
    let trimmed = port_name.trim();
    let port = open_port(trimmed, NMEA_BAUD)?;
    let mut client = PmtkClient::new(port);
    let info = client.request_identity()?;
    let log_used_bytes = client.log_used_bytes()?;
    // The recording state is a convenience figure, not part of the probe's
    // contract: a unit that answers the identity and the usage but not
    // `$PMTK182,2,1` still gets a useful card, with this one field unknown.
    let log_enabled = client.log_enabled().ok();
    Ok(DeviceInfo::from_core(
        trimmed,
        info,
        log_used_bytes,
        log_enabled,
    ))
}

/// Download the raw track log bytes from `port_name`.
///
/// # Errors
///
/// [`m241_core::Error`] from opening the port or reading the PMTK182 log.
pub fn download_log_bytes(port_name: &str) -> Result<Vec<u8>> {
    let port = open_port(port_name, NMEA_BAUD)?;
    let mut client = PmtkClient::new(port);
    client.download_log(&mut |_, _| {})
}

/// Firmware write mode accepted by [`fw_flash`] and recommended by [`fw_inspect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashMode {
    /// Strict backup-diff mode (backup itself or WNRO-patched backup).
    Strict,
    /// Write the backup itself back without whole-device pre-comparison.
    Recover,
    /// Official vendor V1.13 or V1.12 image verified by pinned full SHA-256.
    Vendor,
    /// Any physically valid image (non-empty, even length, `<= 512 KiB`).
    Unverified,
}

impl FlashMode {
    /// Parse a mode string from the IPC boundary.
    ///
    /// # Errors
    ///
    /// [`Error::Safety`] when `raw` is not one of the four supported modes.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "strict" => Ok(Self::Strict),
            "recover" => Ok(Self::Recover),
            "vendor" => Ok(Self::Vendor),
            "unverified" => Ok(Self::Unverified),
            other => Err(Error::Safety {
                reason: format!(
                    "unsupported flash mode `{other}`; expected one of strict, recover, vendor, unverified"
                ),
            }),
        }
    }
}

/// Outcome of `fw_dump`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FwDumpOutcome {
    /// Absolute path written.
    pub destination: String,
    /// Byte length written.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the dump.
    pub sha256: String,
    /// Identified profile name, if recognised.
    pub profile: Option<String>,
    /// Whether the effective GPS epoch constant is already WNRO-patched.
    pub wnro_patched: Option<bool>,
}

/// Outcome of `fw_patch`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FwPatchOutcome {
    /// Absolute path written.
    pub output_path: String,
    /// Byte length written.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the patched image.
    pub sha256: String,
    /// Byte offsets modified by the patch.
    pub changed_offsets: Vec<usize>,
}

/// Inspection summary returned by `fw_inspect`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FwInspectDto {
    /// Image size in bytes.
    pub image_bytes: u64,
    /// Image SHA-256 hex digest.
    pub image_sha256: String,
    /// Optional backup size in bytes.
    pub backup_bytes: Option<u64>,
    /// Optional backup SHA-256 hex digest.
    pub backup_sha256: Option<String>,
    /// Identified profile name, if known.
    pub profile: Option<String>,
    /// Whether the effective GPS epoch is already WNRO-patched (`None` if unreadable/unexpected).
    pub wnro_patched: Option<bool>,
    /// Differing byte count vs backup (when backup is provided and same length).
    pub diff_count: Option<usize>,
    /// Whether any difference vs backup falls outside the profile's patch offsets.
    pub outside_patch_offsets: Option<bool>,
    /// Whether the image starts with the ARM reset vector (`18 F0 9F E5`).
    pub arm_vector_ok: bool,
    /// Whether the image matches the pinned official V1.13 release.
    pub is_vendor_v113: bool,
    /// Whether the image matches the pinned official V1.12 release.
    pub is_vendor_v112: bool,
    /// Recommended flash mode for this image/backup pair.
    pub recommended_mode: FlashMode,
    /// Human-readable warnings in Japanese.
    pub warnings: Vec<String>,
}

/// Outcome of `fw_probe_da`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FwProbeDaOutcome {
    /// Serial port probed.
    pub port: String,
    /// Flash chip `MMMM:DDDD` identifier.
    pub flash_id: String,
    /// Flash manufacturer ID.
    pub flash_manufacturer: u16,
    /// Flash device ID.
    pub flash_device: u16,
    /// Flash size reported by the DA in bytes.
    pub flash_size_bytes: u32,
    /// DA version string (e.g. `v3.8`).
    pub da_version: String,
}

/// Outcome of `fw_flash`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FwFlashOutcome {
    /// Serial port written.
    pub port: String,
    /// Mode used (`strict`, `recover`, `vendor`, `unverified`).
    pub mode: FlashMode,
    /// Number of bytes programmed and verified.
    pub bytes_written: usize,
    /// SHA-256 of the written image.
    pub image_sha256: String,
    /// Whether BootROM read-back matched under the mode's policy.
    pub verified_read_back: bool,
    /// Number of per-unit bytes (`0x5C..0x60`) restored by firmware across reboot.
    pub restored_per_unit_bytes: usize,
    /// Elapsed wall-clock time in milliseconds.
    pub elapsed_ms: u64,
}

/// Validate an absolute file path and read its bytes.
fn read_absolute_file(raw: &str, label: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::State(format!("{label} is empty")));
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(Error::Safety {
            reason: format!("{label} must be an absolute path: {trimmed}"),
        });
    }
    std::fs::read(path).map_err(|source| Error::Io {
        path: Some(trimmed.to_owned()),
        source,
    })
}

/// Validate an absolute destination path (not a directory, parent directory exists).
pub(crate) fn prepare_output_path(raw: &str, label: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::State(format!("{label} is empty")));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(Error::Safety {
            reason: format!("{label} must be an absolute path: {trimmed}"),
        });
    }
    if path.is_dir() {
        return Err(Error::State(format!(
            "{} is a directory; include a file name",
            path.display()
        )));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(Error::Io {
            path: Some(parent.display().to_string()),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "destination parent directory does not exist",
            ),
        });
    }
    Ok(path)
}

fn canonical_destination(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).ok();
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    Some(std::fs::canonicalize(parent).ok()?.join(name))
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let (Ok(left_meta), Ok(right_meta)) = (std::fs::metadata(left), std::fs::metadata(right))
    else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        left_meta.dev() == right_meta.dev() && left_meta.ino() == right_meta.ino()
    }
    #[cfg(not(unix))]
    {
        let (Ok(left_canonical), Ok(right_canonical)) =
            (std::fs::canonicalize(left), std::fs::canonicalize(right))
        else {
            return false;
        };
        left_canonical == right_canonical
    }
}

fn epoch_patched_state(bytes: &[u8]) -> Option<bool> {
    let image = FirmwareImage::new(bytes).ok()?;
    let epoch = image.effective_epoch()?;
    match GpsEpoch::from_constant(epoch) {
        GpsEpoch::Wnro => Some(true),
        GpsEpoch::Stock => Some(false),
        GpsEpoch::Unexpected(_) => None,
    }
}

/// Dump firmware from `port_name` (read-only) and save it to `destination`.
///
/// # Errors
///
/// [`Error`] from path validation, port opening, BootROM entry/read, or file writing.
pub fn fw_dump(port_name: &str, destination: &str, assume_brom: bool) -> Result<FwDumpOutcome> {
    let dest_path = prepare_output_path(destination, "destination")?;
    let transport = open_port(port_name, entry_baud(assume_brom))?;
    fw_dump_on(transport, &dest_path, assume_brom)
}

fn fw_dump_on<T: Transport>(
    transport: T,
    dest_path: &Path,
    assume_brom: bool,
) -> Result<FwDumpOutcome> {
    let bytes = flow::dump(transport, assume_brom, &mut |_| {})?;
    std::fs::write(dest_path, &bytes).map_err(|source| Error::Io {
        path: Some(dest_path.display().to_string()),
        source,
    })?;
    let sha256 = sha256_hex(&bytes);
    let profile = FirmwareImage::new(&bytes)
        .ok()
        .and_then(|image| identify(&image).ok().map(|id| id.profile.name.to_owned()));
    let wnro_patched = epoch_patched_state(&bytes);
    Ok(FwDumpOutcome {
        destination: dest_path.display().to_string(),
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        sha256,
        profile,
        wnro_patched,
    })
}

/// Apply the WNRO patch to `input_path` and write the result to `output_path`.
///
/// Never opens a serial port and refuses to overwrite the input image.
///
/// # Errors
///
/// [`Error`] when paths are invalid/identical, the image cannot be identified or
/// planned, or writing fails.
pub fn fw_patch(input_path: &str, output_path: &str) -> Result<FwPatchOutcome> {
    let bytes = read_absolute_file(input_path, "inputPath")?;
    let out_path = prepare_output_path(output_path, "outputPath")?;
    let in_path = Path::new(input_path.trim());
    if let (Ok(in_canon), Some(out_canon)) = (
        std::fs::canonicalize(in_path),
        canonical_destination(&out_path),
    ) && same_file(&in_canon, &out_canon)
    {
        return Err(Error::Safety {
            reason: String::from(
                "outputPath points to the same file as inputPath; refusing to overwrite the backup",
            ),
        });
    }

    let image = FirmwareImage::new(&bytes)?;
    let _identification = identify(&image)?;
    let plan = firmware::patch::plan(&image)?;
    if !plan.changes_anything() {
        return Err(Error::Firmware {
            message: format!(
                "the image is already patched: {}",
                plan.no_op_reason.as_deref().unwrap_or("nothing to change")
            ),
            offset: Some(firmware::image::EFFECTIVE_EPOCH_OFFSET),
        });
    }
    let patched = plan.apply(&bytes)?;
    let patched_image = FirmwareImage::new(&patched)?;
    if !patched_image.is_patched() {
        return Err(Error::Firmware {
            message: String::from("patched image failed effective epoch verification"),
            offset: Some(firmware::image::EFFECTIVE_EPOCH_OFFSET),
        });
    }
    std::fs::write(&out_path, &patched).map_err(|source| Error::Io {
        path: Some(out_path.display().to_string()),
        source,
    })?;
    Ok(FwPatchOutcome {
        output_path: out_path.display().to_string(),
        bytes: u64::try_from(patched.len()).unwrap_or(u64::MAX),
        sha256: sha256_hex(&patched),
        changed_offsets: plan.edits.iter().map(|edit| edit.offset).collect(),
    })
}

fn diff_summary(
    backup: &[u8],
    image: &[u8],
    profile: Option<&'static FirmwareProfile>,
) -> (Option<usize>, Option<bool>) {
    if backup.len() != image.len() {
        return (None, Some(true));
    }
    let diff_count = backup
        .iter()
        .zip(image.iter())
        .filter(|(a, b)| a != b)
        .count();
    let outside = match profile.and_then(profile_patch_offsets) {
        Some(allowed) => backup
            .iter()
            .zip(image.iter())
            .enumerate()
            .any(|(offset, (a, b))| a != b && !allowed.contains(&offset)),
        None => diff_count > 0,
    };
    (Some(diff_count), Some(outside))
}

/// Inspect a firmware image (and optional backup) without opening a serial port.
///
/// Reuses [`classify_image`], [`accept_vendor_image`], and [`identify`].
///
/// # Errors
///
/// [`Error`] when `image_path` (or non-empty `backup_path`) cannot be read or is relative.
pub fn fw_inspect(image_path: &str, backup_path: Option<&str>) -> Result<FwInspectDto> {
    let image = read_absolute_file(image_path, "imagePath")?;
    let backup = match backup_path.map(str::trim).filter(|s| !s.is_empty()) {
        Some(path) => Some(read_absolute_file(path, "backupPath")?),
        None => None,
    };

    let image_sha256 = sha256_hex(&image);
    let backup_bytes = backup
        .as_ref()
        .map(|b| u64::try_from(b.len()).unwrap_or(u64::MAX));
    let backup_sha256 = backup.as_deref().map(sha256_hex);

    let is_vendor_v113 = accept_vendor_image(&PROFILE_V1_13, &image).is_ok();
    let is_vendor_v112 = accept_vendor_image(&PROFILE_V1_12, &image).is_ok();
    let arm_vector_ok = image.starts_with(&VECTOR);
    let identified_profile = FirmwareImage::new(&image)
        .ok()
        .and_then(|parsed| identify(&parsed).ok().map(|id| id.profile));
    let wnro_patched = epoch_patched_state(&image);

    let mut warnings = Vec::new();
    if let Some(backup_data) = backup.as_deref()
        && let Err(err) = check_physical_image(backup_data)
    {
        warnings.push(format!("バックアップの検証に失敗: {err}"));
    }

    let classification = classify_image(backup.as_deref(), &image);
    let (profile_name, diff_count, outside_patch_offsets, recommended_mode) = match classification {
        ImageClassification::VerifiedBackupOrPatch {
            profile,
            differing_bytes,
        } => {
            if differing_bytes == 0 {
                warnings.push(String::from(
                    "イメージはバックアップと同一です（変更なし）。障害後の復旧には recover モードも使用できます。",
                ));
            }
            (
                Some(profile.name.to_owned()),
                Some(differing_bytes),
                Some(false),
                FlashMode::Strict,
            )
        }
        ImageClassification::OfficialVendor { profile } => {
            let (diff_count, outside) = match backup.as_deref() {
                Some(backup_data) => diff_summary(backup_data, &image, Some(profile)),
                None => (None, None),
            };
            warnings.push(format!(
                "公式 {} ベンダーイメージです。書き込むと個体データ {VENDOR_PER_UNIT_DATA_BYTES} バイトが上書きされます。",
                profile.name
            ));
            warnings.push(String::from(
                "公式ベンダーイメージは未パッチ状態（GPS エポックが 7168 日遅れ）です。",
            ));
            if profile.name == PROFILE_V1_12.name {
                warnings.push(String::from(
                    "V1.12 ベンダーイメージの実機書き込みは未検証です（現在の固定書き込み長は V1.13 用の 406,620 バイトです）。",
                ));
            }
            (
                Some(profile.name.to_owned()),
                diff_count,
                outside,
                FlashMode::Vendor,
            )
        }
        ImageClassification::Unverified { has_arm_vector, .. } => {
            let (diff_count, outside) = match backup.as_deref() {
                Some(backup_data) => diff_summary(backup_data, &image, identified_profile),
                None => (None, None),
            };
            warnings.push(String::from(
                "バックアップ差分・公式ベンダーのいずれにも一致しない未検証イメージです（unverified モード）。",
            ));
            if !has_arm_vector {
                warnings.push(String::from(
                    "先頭 4 バイトが ARM リセットベクタ（18 F0 9F E5）と一致しません。",
                ));
            }
            if outside == Some(true) {
                warnings.push(String::from(
                    "バックアップとの差分がパッチ対象オフセット外に存在します。",
                ));
            }
            (
                identified_profile.map(|p| p.name.to_owned()),
                diff_count,
                outside,
                FlashMode::Unverified,
            )
        }
        ImageClassification::Blocked { reason } => {
            let (diff_count, outside) = match backup.as_deref() {
                Some(backup_data) => diff_summary(backup_data, &image, identified_profile),
                None => (None, None),
            };
            warnings.push(reason);
            (
                identified_profile.map(|p| p.name.to_owned()),
                diff_count,
                outside,
                FlashMode::Unverified,
            )
        }
    };

    Ok(FwInspectDto {
        image_bytes: u64::try_from(image.len()).unwrap_or(u64::MAX),
        image_sha256,
        backup_bytes,
        backup_sha256,
        profile: profile_name,
        wnro_patched,
        diff_count,
        outside_patch_offsets,
        arm_vector_ok,
        is_vendor_v113,
        is_vendor_v112,
        recommended_mode,
        warnings,
    })
}

/// Load the vendor DA into RAM and probe both candidate flash addresses (`m241_flash::flow::probe_da`).
///
/// Requires `confirm_word.trim() == "PROBE"` before reading files or opening the port.
///
/// # Errors
///
/// [`Error::Safety`] when `confirm_word` is not `PROBE` or the DA fails digest verification,
/// or transport/BootROM/DA errors.
pub fn fw_probe_da(
    port_name: &str,
    da_path: &str,
    assume_brom: bool,
    confirm_word: &str,
) -> Result<FwProbeDaOutcome> {
    if confirm_word.trim() != PROBE_CONFIRMATION {
        return Err(Error::Safety {
            reason: format!(
                "refusing to load the DA without typing `{PROBE_CONFIRMATION}` in the confirmation field"
            ),
        });
    }
    let trimmed_port = port_name.trim();
    if trimmed_port.is_empty() {
        return Err(Error::Port {
            message: String::from("no port name was given"),
        });
    }
    let da = read_absolute_file(da_path, "daPath")?;
    flow::check_da(&da)?;
    let transport = open_port(trimmed_port, entry_baud(assume_brom))?;
    fw_probe_da_on(
        transport,
        trimmed_port,
        &da,
        DaDigest::vendor(),
        assume_brom,
        confirm_word,
    )
}

fn fw_probe_da_on<T: Transport>(
    transport: T,
    port_name: &str,
    da: &[u8],
    da_digest: DaDigest,
    assume_brom: bool,
    confirm_word: &str,
) -> Result<FwProbeDaOutcome> {
    if confirm_word.trim() != PROBE_CONFIRMATION {
        return Err(Error::Safety {
            reason: format!(
                "refusing to load the DA without typing `{PROBE_CONFIRMATION}` in the confirmation field"
            ),
        });
    }
    let confirmation = Confirmation::granted();
    let report = flow::probe_da(
        transport,
        da,
        da_digest,
        confirmation,
        assume_brom,
        &mut |_| {},
    )?;
    Ok(FwProbeDaOutcome {
        port: port_name.to_owned(),
        flash_id: format!("{:04X}:{:04X}", report.id.manufacturer, report.id.device),
        flash_manufacturer: report.id.manufacturer,
        flash_device: report.id.device,
        flash_size_bytes: report.id.flash_bytes,
        da_version: format!("v{}.{}", report.id.version >> 8, report.id.version & 0xFF),
    })
}

/// Program firmware through [`m241_flash::flow::flash`] and verify by BootROM read-back.
///
/// Requires `confirm_word.trim() == "WRITE"` before reading files or opening the port.
///
/// # Errors
///
/// [`Error::Safety`] for an untyped confirmation, bad mode, layout failure, bad DA,
/// or failed image/backup acceptance; or transport/DA/read-back errors.
pub fn fw_flash(
    port_name: &str,
    image_path: &str,
    backup_path: &str,
    da_path: &str,
    mode: &str,
    assume_brom: bool,
    confirm_word: &str,
) -> Result<FwFlashOutcome> {
    if confirm_word.trim() != WRITE_CONFIRMATION {
        return Err(Error::Safety {
            reason: format!(
                "refusing to write flash without typing `{WRITE_CONFIRMATION}` in the confirmation field"
            ),
        });
    }
    let parsed_mode = FlashMode::parse(mode)?;
    let trimmed_port = port_name.trim();
    if trimmed_port.is_empty() {
        return Err(Error::Port {
            message: String::from("no port name was given"),
        });
    }
    let backup = read_absolute_file(backup_path, "backupPath")?;
    let image = read_absolute_file(image_path, "imagePath")?;
    let da = read_absolute_file(da_path, "daPath")?;
    flow::check_da(&da)?;
    preflight_flash(parsed_mode, &backup, &image)?;
    let transport = open_port(trimmed_port, entry_baud(assume_brom))?;
    fw_flash_on(
        transport,
        trimmed_port,
        &backup,
        &image,
        &da,
        DaDigest::vendor(),
        parsed_mode,
        assume_brom,
        confirm_word,
    )
}

fn preflight_flash(mode: FlashMode, backup: &[u8], image: &[u8]) -> Result<()> {
    check_physical_image(image)?;
    check_physical_image(backup)?;
    check_layout()?;
    match mode {
        FlashMode::Strict => {
            let profile = identify(&FirmwareImage::new(image)?)?.profile;
            write_plan(profile, backup, image)?;
        }
        FlashMode::Recover => {
            if image != backup {
                return Err(Error::Safety {
                    reason: String::from(
                        "recover mode requires the image to be byte-identical to the backup",
                    ),
                });
            }
            let profile = identify(&FirmwareImage::new(backup)?)?.profile;
            check_image(profile, backup, backup)?;
        }
        FlashMode::Vendor => {
            let profile = identify(&FirmwareImage::new(image)?)?.profile;
            let token = accept_vendor_image(profile, image)?;
            write_plan_vendor(profile, backup, token)?;
        }
        FlashMode::Unverified => {
            accept_unverified_image(image)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fw_flash_on<T: Transport>(
    transport: T,
    port_name: &str,
    backup: &[u8],
    image: &[u8],
    da: &[u8],
    da_digest: DaDigest,
    mode: FlashMode,
    assume_brom: bool,
    confirm_word: &str,
) -> Result<FwFlashOutcome> {
    if confirm_word.trim() != WRITE_CONFIRMATION {
        return Err(Error::Safety {
            reason: format!(
                "refusing to write flash without typing `{WRITE_CONFIRMATION}` in the confirmation field"
            ),
        });
    }
    let confirmation = Confirmation::granted();
    preflight_flash(mode, backup, image)?;

    let vendor = if mode == FlashMode::Vendor {
        let profile = identify(&FirmwareImage::new(image)?)?.profile;
        Some(accept_vendor_image(profile, image)?)
    } else {
        None
    };
    let unverified = if mode == FlashMode::Unverified {
        Some(accept_unverified_image(image)?)
    } else {
        None
    };
    let target_image = if mode == FlashMode::Recover {
        backup
    } else {
        image
    };
    let request = FlashRequest {
        backup,
        image: target_image,
        da,
        da_digest,
        confirmation,
        recover: mode == FlashMode::Recover,
        assume_brom,
        vendor,
        unverified,
    };
    let started = Instant::now();
    let report = m241_flash::flow::flash(transport, &request, &mut |_| {})?;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(FwFlashOutcome {
        port: port_name.to_owned(),
        mode,
        bytes_written: report.length,
        image_sha256: sha256_hex(target_image),
        verified_read_back: report.verified == report.length,
        restored_per_unit_bytes: report.restored_per_unit_bytes,
        elapsed_ms,
    })
}

impl DeviceInfo {
    fn from_core(
        port: &str,
        info: CoreDeviceInfo,
        log_used_bytes: usize,
        log_enabled: Option<bool>,
    ) -> Self {
        Self {
            port: port.to_owned(),
            known_release: info.is_known_release(),
            release: info.release,
            build: info.build,
            model: info.model,
            sdk: info.sdk,
            log_used_bytes: u64::try_from(log_used_bytes).unwrap_or(u64::MAX),
            log_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_usb_vendor_is_flagged_without_a_description() {
        assert!(looks_like_cable(Some(0x10C4), Some(0xEA60), None));
        assert!(looks_like_cable(Some(0x1A86), Some(0x7523), None));
    }

    #[test]
    fn an_unrelated_usb_port_is_not_flagged() {
        assert!(!looks_like_cable(
            Some(0x046D),
            Some(0xC52B),
            Some("Logitech")
        ));
    }

    #[test]
    fn the_description_is_the_fallback_when_the_ids_are_absent() {
        assert!(looks_like_cable(None, None, Some("HOLUX M-241")));
        assert!(looks_like_cable(None, None, Some("Silicon Labs CP210x")));
        assert!(!looks_like_cable(None, None, Some("USB Serial")));
    }

    #[test]
    fn a_device_info_keeps_the_release_warning_and_the_usage() {
        let info = DeviceInfo::from_core(
            "/dev/ttyUSB0",
            CoreDeviceInfo {
                release: String::from("B-core_1.20"),
                build: String::from("0043"),
                model: String::from("01017-00F"),
                sdk: Some(String::from("1.0")),
            },
            0x0003_3F7C,
            Some(true),
        );
        assert_eq!(info.port, "/dev/ttyUSB0");
        assert!(info.known_release);
        assert_eq!(info.log_used_bytes, 212_860);
        assert_eq!(info.log_enabled, Some(true));
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        directory
    }

    fn synthetic_stock_v113() -> Vec<u8> {
        use m241_core::firmware::image::{
            BUILD_NUMBER_OFFSET, DEVICE_TYPE_OFFSET, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
            FAMILY_SIZE, FW_SIZE_OFFSET, GPS_EPOCH_UNIX,
        };
        let profile = PROFILE_V1_13;
        let mut data = vec![0xFF_u8; profile.size];
        data[..VECTOR.len()].copy_from_slice(&VECTOR);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY_SIZE].fill(0);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..].fill(0);
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
        let marker = profile.name.as_bytes();
        data[profile.version_marker..profile.version_marker + marker.len()].copy_from_slice(marker);
        let stock = GPS_EPOCH_UNIX.to_le_bytes();
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + stock.len()].copy_from_slice(&stock);
        }
        let declared = u32::try_from(profile.size).unwrap_or(0).to_le_bytes();
        data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + declared.len()].copy_from_slice(&declared);
        data[BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2].copy_from_slice(&67_u16.to_le_bytes());
        data
    }

    fn fake_da() -> (Vec<u8>, DaDigest) {
        use m241_flash::da::{DA_IMAGE_HEAD, DA_IMAGE_SIZE};
        let mut image = vec![0u8; DA_IMAGE_SIZE];
        image[..DA_IMAGE_HEAD.len()].copy_from_slice(&DA_IMAGE_HEAD);
        let digest = DaDigest::for_test_da(&sha256_hex(&image));
        (image, digest)
    }

    #[derive(Debug, Clone)]
    struct SharedDa(std::rc::Rc<std::cell::RefCell<m241_flash::FakeDa>>);

    impl Transport for SharedDa {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.0.borrow_mut().read(buf)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.0.borrow_mut().write_all(buf)
        }
        fn flush(&mut self) -> Result<()> {
            self.0.borrow_mut().flush()
        }
        fn clear_input(&mut self) -> Result<()> {
            self.0.borrow_mut().clear_input()
        }
        fn set_baud(&mut self, baud: u32) -> Result<()> {
            self.0.borrow_mut().set_baud(baud)
        }
        fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
            self.0.borrow_mut().set_control_lines(asserted)
        }
    }

    #[test]
    fn fw_patch_and_fw_inspect_agree_on_a_stock_and_patched_pair() {
        let dir = temp_dir("m241-app-fw-patch");
        let backup_path = dir.join("backup.bin");
        let patched_path = dir.join("patched.bin");
        let stock = synthetic_stock_v113();
        std::fs::write(&backup_path, &stock).expect("write backup");

        let backup_str = backup_path.display().to_string();
        let patched_str = patched_path.display().to_string();

        // Refusing to overwrite the source image.
        let overwrite_err = fw_patch(&backup_str, &backup_str).expect_err("overwrite refused");
        assert!(matches!(overwrite_err, Error::Safety { .. }));

        let outcome = fw_patch(&backup_str, &patched_str).expect("patch succeeds");
        assert_eq!(outcome.changed_offsets, vec![0x402F6, 0x402F7, 0x59C2C]);
        assert_eq!(outcome.bytes, stock.len() as u64);

        let inspect_patched = fw_inspect(&patched_str, Some(&backup_str)).expect("inspect patched");
        assert_eq!(inspect_patched.profile.as_deref(), Some("V1.13"));
        assert_eq!(inspect_patched.wnro_patched, Some(true));
        assert_eq!(inspect_patched.diff_count, Some(3));
        assert_eq!(inspect_patched.outside_patch_offsets, Some(false));
        assert!(inspect_patched.arm_vector_ok);
        assert_eq!(inspect_patched.recommended_mode, FlashMode::Strict);

        let inspect_unpatched = fw_inspect(&backup_str, None).expect("inspect backup alone");
        assert_eq!(inspect_unpatched.wnro_patched, Some(false));
        assert_eq!(inspect_unpatched.recommended_mode, FlashMode::Unverified);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fw_inspect_warns_on_missing_arm_vector_and_bad_physical_size() {
        let dir = temp_dir("m241-app-fw-inspect-bad");
        let custom_path = dir.join("custom.bin");
        std::fs::write(&custom_path, vec![0xAA_u8; 512]).expect("write custom");
        let dto = fw_inspect(&custom_path.display().to_string(), None).expect("inspect custom");
        assert!(!dto.arm_vector_ok);
        assert_eq!(dto.recommended_mode, FlashMode::Unverified);
        assert!(
            dto.warnings.iter().any(|w| w.contains("リセットベクタ")),
            "expected ARM vector warning, got {:?}",
            dto.warnings
        );

        let odd_path = dir.join("odd.bin");
        std::fs::write(&odd_path, [0x11_u8; 3]).expect("write odd");
        let blocked = fw_inspect(&odd_path.display().to_string(), None).expect("inspect odd");
        assert!(!blocked.warnings.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fw_flash_and_fw_probe_da_refuse_wrong_confirmation_words_before_port_open() {
        let err = fw_probe_da("/dev/rfcomm0", "/nonexistent/da.bin", false, "write")
            .expect_err("wrong confirm word refused");
        assert!(matches!(err, Error::Safety { .. }));

        let err = fw_flash(
            "/dev/rfcomm0",
            "/nonexistent/img.bin",
            "/nonexistent/bak.bin",
            "/nonexistent/da.bin",
            "strict",
            false,
            "probe",
        )
        .expect_err("wrong confirm word refused");
        assert!(matches!(err, Error::Safety { .. }));
    }

    #[test]
    fn fw_dump_probe_da_and_flash_work_against_fake_da() {
        let dir = temp_dir("m241-app-fake-da");
        let dump_path = dir.join("dump.bin");
        let stock = synthetic_stock_v113();
        let (da, da_digest) = fake_da();
        let fake = std::rc::Rc::new(std::cell::RefCell::new(m241_flash::FakeDa::with_firmware(
            &stock,
        )));

        let dumped = fw_dump_on(SharedDa(fake.clone()), &dump_path, true).expect("dump");
        assert_eq!(dumped.bytes, stock.len() as u64);
        assert_eq!(dumped.profile.as_deref(), Some("V1.13"));
        assert_eq!(dumped.wnro_patched, Some(false));

        let probe = fw_probe_da_on(
            SharedDa(fake.clone()),
            "/dev/rfcomm0",
            &da,
            da_digest.clone(),
            true,
            PROBE_CONFIRMATION,
        )
        .expect("probe_da");
        assert_eq!(probe.port, "/dev/rfcomm0");
        assert_eq!(probe.flash_id, "00C2:22BA");
        assert_eq!(probe.da_version, "v3.8");

        let patched_path = dir.join("patched.bin");
        fw_patch(
            &dump_path.display().to_string(),
            &patched_path.display().to_string(),
        )
        .expect("patch");
        let patched = std::fs::read(&patched_path).expect("read patched");

        // `probe_da` leaves the simulated chip running the DA in RAM; start a
        // fresh BootROM-resident `FakeDa` for the flash session.
        let flash_fake = std::rc::Rc::new(std::cell::RefCell::new(
            m241_flash::FakeDa::with_firmware(&stock),
        ));
        let outcome = fw_flash_on(
            SharedDa(flash_fake.clone()),
            "/dev/rfcomm0",
            &stock,
            &patched,
            &da,
            da_digest,
            FlashMode::Strict,
            true,
            WRITE_CONFIRMATION,
        )
        .expect("strict flash");
        assert_eq!(outcome.mode, FlashMode::Strict);
        assert_eq!(outcome.bytes_written, stock.len());
        assert!(outcome.verified_read_back);
        assert_eq!(flash_fake.borrow().memory(), &patched[..]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
