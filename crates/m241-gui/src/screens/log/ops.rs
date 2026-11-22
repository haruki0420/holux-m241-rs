//! The ログ操作 console: the CLI's log and NMEA commands as screen controls.
//!
//! One panel on the Log screen, with a chip row rather than a second rail: the
//! staircase is the navigation's signature, and a local mode row borrows the
//! export chips' language instead. Each mode names the CLI command it replaces,
//! so a person can check the two against each other.
//!
//! What runs where:
//!
//! * download and the erase backup use the PMTK182 path on a thread;
//! * the erase itself cannot run here — `m241-gui` does not link `m241-flash`,
//!   which is the crate that owns `LogEraser` — so the confirmation stops at a
//!   refusal that says so;
//! * project save/load, `log read`, NMEA repair and the NMEA relay are offline
//!   file work, except the relay's serial source.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{RichText, Ui, Vec2};
use m241_core::ExportFormat;

use super::LogScreen;
use super::device::{self, DownloadJob, DownloadMsg, DownloadPurpose, DumpOutcome};
use super::nmea::{self, OffsetSpec};
use super::project::{self, ProjectDocument};
use super::proxy::{ProxyConfig, ProxyHandle, ProxySnapshot, ProxySource};
use crate::screens::common::{self, Status};
use crate::theme::{self, metric, size, space};

/// How long a running transfer or relay waits before the next frame.
const REPAINT_INTERVAL: Duration = Duration::from_millis(100);

/// The refusal the erase action ends at. The write path is a separate crate this
/// one deliberately does not depend on (see `tools/check_safety.py`), so the
/// screen can take a backup and a confirmation but cannot send the erase.
const ERASE_REFUSAL: &str = "バックアップは保存済みですが、この画面から消去は実行していません。消去の経路（m241-flash）は GUI にリンクされていないためです。CLI の `m241 log erase` と同じ手順ですが、実行には端末のログを消すことへのユーザーの明示的な承認が別途必要です。";

/// Which operation the console shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OpsMode {
    /// Read the log off the unit (CLI `log download`).
    #[default]
    Download,
    /// Back up and erase (CLI `log erase`).
    Erase,
    /// JSON project save/load (CLI `log project save|load`).
    Project,
    /// NMEA date repair (CLI `nmea repair`).
    Repair,
    /// NMEA relay (CLI `nmea proxy`).
    Proxy,
    /// Read and export (CLI `log read`).
    Read,
}

impl OpsMode {
    const ALL: [Self; 6] = [
        Self::Download,
        Self::Erase,
        Self::Project,
        Self::Repair,
        Self::Proxy,
        Self::Read,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Download => "吸い出し",
            Self::Erase => "消去",
            Self::Project => "プロジェクト",
            Self::Repair => "NMEA 修復",
            Self::Proxy => "NMEA 中継",
            Self::Read => "読み書き",
        }
    }

    fn cli_note(self) -> &'static str {
        match self {
            Self::Download => {
                "CLI の `m241 log download` と同じ PMTK182 経路です。実機での GUI 吸い出しは未検証です。"
            }
            Self::Erase => {
                "CLI の `m241 log erase` と同じ手順（バックアップ→確認→消去）です。消去の実行だけはこの画面にリンクされていません。"
            }
            Self::Project => {
                "CLI の `m241 log project save|load` と同じ JSON 形式です。読込後は下部の「書き出す」で同じ設定のまま書き出せます。"
            }
            Self::Repair => "CLI の `m241 nmea repair` と同じ日付補正とチェックサム再計算です。",
            Self::Proxy => {
                "CLI の `m241 nmea proxy` と同じ中継です。既定ではループバックにだけ待ち受けます。実機での GUI 中継は未検証です。"
            }
            Self::Read => {
                "CLI の `m241 log read` と同じ読み取り・書き出しです（--nmea / --format / --utc-offset-days / --output）。"
            }
        }
    }
}

/// Everything the console owns. A single field on [`LogScreen`], so the screen's
/// own state stays about the track.
#[derive(Debug)]
pub(super) struct LogOps {
    mode: OpsMode,
    status: Option<(Status, String)>,
    /// A download or backup in flight.
    download: Option<DownloadJob>,
    /// Bytes received so far.
    progress: Option<(usize, usize)>,
    port: String,
    ports: Vec<String>,
    dump_path: String,
    force: bool,
    backup_path: String,
    backup: Option<(String, usize)>,
    erase_confirm: bool,
    project_path: String,
    project_log: String,
    project_photos: String,
    project_output: String,
    project_offset: String,
    project_nmea: bool,
    project_format: ExportFormat,
    project_kmz: bool,
    repair_input: String,
    repair_output: String,
    repair_offset: String,
    proxy_from_file: bool,
    proxy_port: String,
    proxy_baud: String,
    proxy_file: String,
    proxy_bind: String,
    proxy_allow_non_loopback: bool,
    proxy_offset: String,
    proxy: Option<ProxyHandle>,
    read_input: String,
    read_nmea: bool,
    read_format: ExportFormat,
    read_offset: String,
    read_output: String,
    read_force: bool,
}

impl Default for LogOps {
    fn default() -> Self {
        Self {
            mode: OpsMode::default(),
            status: None,
            download: None,
            progress: None,
            port: String::new(),
            ports: Vec::new(),
            dump_path: String::from("m241-log.bin"),
            force: false,
            backup_path: String::from("m241-log-backup.bin"),
            backup: None,
            erase_confirm: false,
            project_path: String::from("m241-project.json"),
            project_log: String::new(),
            project_photos: String::new(),
            project_output: String::from("m241-track.gpx"),
            project_offset: String::from("0"),
            project_nmea: false,
            project_format: ExportFormat::Gpx,
            project_kmz: false,
            repair_input: String::new(),
            repair_output: String::from("repaired.nmea"),
            repair_offset: String::from("auto"),
            proxy_from_file: false,
            proxy_port: String::new(),
            proxy_baud: String::from("38400"),
            proxy_file: String::new(),
            proxy_bind: String::from("127.0.0.1:10110"),
            proxy_allow_non_loopback: false,
            proxy_offset: String::from("auto"),
            proxy: None,
            read_input: String::new(),
            read_nmea: false,
            read_format: ExportFormat::Gpx,
            read_offset: String::from("auto"),
            read_output: String::from("m241-track.gpx"),
            read_force: false,
        }
    }
}

impl LogOps {
    /// Draw the console and apply whatever the last transfer produced.
    pub(super) fn ui(&mut self, ui: &mut Ui, screen: &mut LogScreen) {
        self.poll(screen);
        common::panel(ui, "ログ操作", |ui| {
            ui.label(
                RichText::new(self.mode.cli_note())
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            ui.add_space(space::S);
            ui.horizontal_wrapped(|ui| {
                for mode in OpsMode::ALL {
                    if common::tool_button(ui, mode.label(), self.mode == mode, true).clicked() {
                        self.mode = mode;
                    }
                }
            });
            ui.add_space(space::S);
            match self.mode {
                OpsMode::Download => self.download_panel(ui),
                OpsMode::Erase => self.erase_panel(ui),
                OpsMode::Project => self.project_panel(ui, screen),
                OpsMode::Repair => self.repair_panel(ui),
                OpsMode::Proxy => self.proxy_panel(ui),
                OpsMode::Read => self.read_panel(ui, screen),
            }
            if let Some((status, message)) = &self.status {
                ui.add_space(space::XS);
                common::status_line(ui, *status, message);
            }
        });
        if self.download.is_some() || self.proxy_running() {
            ui.ctx().request_repaint_after(REPAINT_INTERVAL);
        }
    }

    /// Take whatever the download thread has produced since the last frame.
    fn poll(&mut self, screen: &mut LogScreen) {
        let mut progress = None;
        let mut finished = None;
        let mut disconnected = false;
        if let Some(job) = self.download.as_ref() {
            loop {
                match job.receiver.try_recv() {
                    Ok(DownloadMsg::Progress { received, total }) => {
                        progress = Some((received, total));
                    }
                    Ok(DownloadMsg::Done(result)) => {
                        finished = Some((job.purpose, result));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(progress) = progress {
            self.progress = Some(progress);
        }
        if let Some((purpose, result)) = finished {
            self.download = None;
            self.progress = None;
            self.finish(purpose, result, screen);
        } else if disconnected {
            self.download = None;
            self.progress = None;
            self.status = Some((
                Status::Err,
                String::from("吸い出しのスレッドが終了しました。ポートを確認してください"),
            ));
        }
    }

    /// Apply a finished transfer.
    fn finish(
        &mut self,
        purpose: DownloadPurpose,
        result: Result<Vec<u8>, String>,
        screen: &mut LogScreen,
    ) {
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(message) => {
                self.status = Some((Status::Err, message));
                return;
            }
        };
        match purpose {
            DownloadPurpose::Dump => {
                match device::save_and_parse(&bytes, &self.dump_path, self.force) {
                    Ok(DumpOutcome::Empty) => {
                        self.status = Some((
                            Status::Info,
                            String::from("端末のログは空でした（0 バイト）。保存はしていません"),
                        ));
                    }
                    Ok(DumpOutcome::Saved(saved)) => {
                        let points = saved.track.points.len();
                        let damaged = saved.report.damaged_blocks;
                        screen.set_track(saved.track, saved.report);
                        screen.log_path = saved.path.display().to_string();
                        screen.destination = saved
                            .path
                            .with_extension(screen.format.extension())
                            .display()
                            .to_string();
                        self.status = Some((
                            Status::Ok,
                            format!(
                                "{} に {} を保存し、{points} 点を読み込みました（破損 {damaged} ブロック）",
                                saved.path.display(),
                                common::bytes(saved.bytes)
                            ),
                        ));
                    }
                    Err(message) => self.status = Some((Status::Err, message)),
                }
            }
            DownloadPurpose::Backup => {
                if bytes.is_empty() {
                    self.backup = None;
                    self.erase_confirm = false;
                    self.status = Some((
                        Status::Info,
                        String::from(
                            "端末のログは空でした（0 バイト）。バックアップも消去もしません",
                        ),
                    ));
                    return;
                }
                match device::write_dump(&self.backup_path, &bytes, self.force) {
                    Ok(path) => {
                        self.backup = Some((path.display().to_string(), bytes.len()));
                        self.erase_confirm = false;
                        self.status = Some((
                            Status::Ok,
                            format!(
                                "バックアップ {} を保存しました（{}）。消去はまだ行っていません",
                                path.display(),
                                common::bytes(bytes.len())
                            ),
                        ));
                    }
                    Err(message) => {
                        self.backup = None;
                        self.erase_confirm = false;
                        self.status = Some((Status::Err, message));
                    }
                }
            }
        }
    }

    /// Whether a transfer is in flight (all panels share the one worker).
    fn busy(&self) -> bool {
        self.download.is_some()
    }

    fn start_download(&mut self, purpose: DownloadPurpose) {
        if self.busy() {
            self.status = Some((Status::Info, String::from("吸い出しが進行中です")));
            return;
        }
        let destination = match purpose {
            DownloadPurpose::Dump => &self.dump_path,
            DownloadPurpose::Backup => &self.backup_path,
        };
        if destination.trim().is_empty() {
            self.status = Some((Status::Err, String::from("保存先を入力してください")));
            return;
        }
        if !self.force {
            let path = PathBuf::from(destination.trim());
            if path.exists() {
                self.status = Some((
                    Status::Err,
                    format!(
                        "{} はすでに存在します。上書きする場合は「上書き」を入れてください",
                        path.display()
                    ),
                ));
                return;
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        device::start_download(self.port.clone(), sender);
        self.download = Some(DownloadJob { purpose, receiver });
        self.progress = Some((0, 0));
        self.status = Some((
            Status::Info,
            String::from("吸い出しを開始しました。ログの転送中は端末の記録が止まります"),
        ));
    }

    fn download_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ポート")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.port)
                    .hint_text(RichText::new("auto または /dev/ttyUSB0").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            if common::tool_button(ui, "一覧", false, true).clicked() {
                self.list_ports();
            }
        });
        if !self.ports.is_empty() {
            ui.label(
                RichText::new(format!("見つかったポート: {}", self.ports.join("、")))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("保存先")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.dump_path)
                    .hint_text(RichText::new("m241-log.bin").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            let running = self.busy();
            if common::primary_button(ui, "吸い出す", !running).clicked() {
                self.start_download(DownloadPurpose::Dump);
            }
        });
        ui.checkbox(
            &mut self.force,
            RichText::new("保存先が存在しても上書きする").size(size::CAPTION),
        );
        if let Some((received, total)) = self.progress {
            let fraction = if total == 0 {
                0.0
            } else {
                (received as f32 / total as f32).clamp(0.0, 1.0)
            };
            let caption = if total == 0 {
                format!("{} B（点は転送後に数えます）", common::grouped(received))
            } else {
                format!(
                    "{} / {} B（{}%）",
                    common::grouped(received),
                    common::grouped(total),
                    (fraction * 100.0).round() as u32
                )
            };
            progress_bar(ui, fraction, caption);
        }
        ui.label(
            RichText::new(
                "転送後に点を作り、破損ブロック数を表示します。空のログはファイルを作りません。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
    }

    fn list_ports(&mut self) {
        match m241_core::protocol::Port::list() {
            Ok(ports) => {
                if self.port.trim().is_empty()
                    && let Some(first) = ports.first()
                {
                    self.port = first.clone();
                }
                self.ports = ports;
                self.status = Some((
                    Status::Info,
                    format!("ポートを {} 件見つけました", self.ports.len()),
                ));
            }
            Err(err) => {
                self.status = Some((Status::Err, format!("ポートを列挙できません: {err}")));
            }
        }
    }

    fn erase_panel(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new(
                "端末のログを消す前に、必ずバックアップを保存します。保存が終わるまで「消去」は有効になりません。",
            )
            .size(size::UI)
            .color(theme::INK),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("バックアップ先")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - metric::ACTION_WIDTH * 1.6 - space::S).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.backup_path)
                    .hint_text(RichText::new("m241-log-backup.bin").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            let running = self.busy();
            if common::secondary_button(ui, "バックアップを保存", !running).clicked() {
                self.start_download(DownloadPurpose::Backup);
            }
        });
        if let Some((path, bytes)) = self.backup.clone() {
            common::status_line(
                ui,
                Status::Ok,
                &format!(
                    "バックアップ {path} を保存しました（{}）。消去はまだ行っていません",
                    common::bytes(bytes)
                ),
            );
        }
        ui.checkbox(
            &mut self.erase_confirm,
            RichText::new("バックアップを保存済みで、この端末のログを消すことに同意します")
                .size(size::CAPTION),
        );
        let ready = self.erase_ready();
        ui.horizontal(|ui| {
            if common::secondary_button(ui, "消去", ready).clicked() {
                self.status = Some((Status::Err, String::from(ERASE_REFUSAL)));
            }
            if !ready {
                let reason = if self.backup.is_none() {
                    "先にバックアップを保存してください"
                } else {
                    "同意のチェックを入れると「消去」が有効になります"
                };
                ui.label(RichText::new(reason).size(size::CAPTION).color(theme::MUTE));
            }
        });
        ui.label(
            RichText::new(
                "消去の経路（m241-flash）は GUI にリンクされていません。この画面はバックアップと確認までを担い、実行は CLI と別承認に残します。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
    }

    /// The erase button is armed only by a saved backup plus the confirmation.
    fn erase_ready(&self) -> bool {
        self.backup.is_some() && self.erase_confirm
    }

    fn project_panel(&mut self, ui: &mut Ui, screen: &mut LogScreen) {
        for (label, value, hint, reserve) in [
            (
                "プロジェクト",
                &mut self.project_path,
                "m241-project.json",
                metric::ACTION_WIDTH * 3.0,
            ),
            ("ログ", &mut self.project_log, "track.bin", 0.0),
            (
                "写真",
                &mut self.project_photos,
                "写真フォルダ（任意）",
                0.0,
            ),
            ("書き出し先", &mut self.project_output, "track.gpx", 0.0),
            (
                "日付補正",
                &mut self.project_offset,
                "0（日数）",
                metric::ACTION_WIDTH,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                let width = (ui.available_width() - reserve - space::S).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(value)
                        .hint_text(RichText::new(hint).size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("形式").size(size::CAPTION).color(theme::MUTE));
            for format in ExportFormat::ALL {
                let selected = !self.project_kmz && self.project_format == format;
                if common::tool_button(ui, &format.extension().to_uppercase(), selected, true)
                    .clicked()
                {
                    self.project_kmz = false;
                    self.project_format = format;
                }
            }
            if common::tool_button(ui, "KMZ", self.project_kmz, true).clicked() {
                self.project_kmz = true;
            }
            ui.checkbox(
                &mut self.project_nmea,
                RichText::new("NMEA 入力").size(size::CAPTION),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if common::secondary_button(ui, "保存", true).clicked() {
                self.save_project();
            }
            if common::secondary_button(ui, "読込", true).clicked() {
                self.load_project(screen, false);
            }
            if common::secondary_button(ui, "読込んで書き出す", true).clicked() {
                self.load_project(screen, true);
            }
        });
    }

    fn save_project(&mut self) {
        let offset = match self.project_offset.trim().parse::<i32>() {
            Ok(offset) => offset,
            Err(_) => {
                self.status = Some((
                    Status::Err,
                    format!(
                        "日付補正 {:?} は整数の日数ではありません",
                        self.project_offset
                    ),
                ));
                return;
            }
        };
        let format = if self.project_kmz {
            "kmz"
        } else {
            self.project_format.extension()
        };
        self.status = Some(
            match project::save_project(
                &self.project_path,
                &self.project_log,
                &self.project_photos,
                &self.project_output,
                offset,
                format,
                self.project_nmea,
            ) {
                Ok(path) => (
                    Status::Ok,
                    format!("プロジェクト {} を保存しました（{format}）", path.display()),
                ),
                Err(message) => (Status::Err, message),
            },
        );
    }

    fn load_project(&mut self, screen: &mut LogScreen, write: bool) {
        let result = (|| -> Result<String, String> {
            let document = ProjectDocument::read(&self.project_path)?;
            let loaded = project::load_log(
                &document.log,
                document.input_is_nmea,
                OffsetSpec::Days(document.utc_offset_days),
            )?;
            let format = if document.export_format.eq_ignore_ascii_case("kmz") {
                None
            } else {
                Some(ExportFormat::parse(&document.export_format).ok_or_else(|| {
                    format!("{:?} は書き出し形式ではありません", document.export_format)
                })?)
            };
            self.project_log = document.log.display().to_string();
            self.project_offset = document.utc_offset_days.to_string();
            self.project_nmea = document.input_is_nmea;
            self.project_photos = document
                .photos
                .as_ref()
                .map(|photos| photos.display().to_string())
                .unwrap_or_default();
            self.project_output = document.output.display().to_string();
            screen.set_track(loaded.track, loaded.report);
            screen.log_path = document.log.display().to_string();
            match format {
                Some(format) => {
                    self.project_kmz = false;
                    self.project_format = format;
                    screen.set_export_format(format);
                    screen.photo_path = self.project_photos.clone();
                }
                None => {
                    self.project_kmz = true;
                    screen.set_kmz_export();
                    screen.photo_path = self.project_photos.clone();
                    if let Some(photos) = &document.photos {
                        let _loaded = screen.load_photos_from_path(photos);
                    }
                }
            }
            screen.set_destination(document.output.display().to_string());
            let mut message = format!("プロジェクトを読み込みました: {}", loaded.note);
            if write {
                screen.export()?;
                message.push_str(&format!("。{} に書き出しました", document.output.display()));
            }
            Ok(message)
        })();
        self.status = Some(match result {
            Ok(message) => (Status::Ok, message),
            Err(message) => (Status::Err, message),
        });
    }

    fn repair_panel(&mut self, ui: &mut Ui) {
        for (label, value, hint) in [
            ("入力", &mut self.repair_input, "capture.nmea"),
            ("出力", &mut self.repair_output, "repaired.nmea"),
            ("日付補正", &mut self.repair_offset, "auto または日数"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                let width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(value)
                        .hint_text(RichText::new(hint).size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        }
        if common::primary_button(ui, "修復", true).clicked() {
            self.run_repair();
        }
    }

    fn run_repair(&mut self) {
        let result = (|| -> Result<String, String> {
            let offset = OffsetSpec::parse(&self.repair_offset)?;
            let input = PathBuf::from(self.repair_input.trim());
            if input.as_os_str().is_empty() {
                return Err(String::from("入力ファイルを指定してください"));
            }
            let output = PathBuf::from(self.repair_output.trim());
            if output.as_os_str().is_empty() {
                return Err(String::from("出力ファイルを指定してください"));
            }
            if names_same_file(&input, &output) {
                return Err(String::from(
                    "入力と出力が同じファイルです。別の名前にしてください",
                ));
            }
            let text = std::fs::read_to_string(&input)
                .map_err(|err| format!("{} を読めません: {err}", input.display()))?;
            let mut changed = 0usize;
            let mut total = 0usize;
            let mut repaired_text = String::with_capacity(text.len() + 64);
            for line in text.split_inclusive('\n') {
                total += 1;
                let (repaired, did_change) = nmea::repair_line(line, offset);
                if did_change {
                    changed += 1;
                }
                repaired_text.push_str(&repaired);
            }
            if let Some(parent) = output
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("{} を作れません: {err}", parent.display()))?;
            }
            std::fs::write(&output, repaired_text.as_bytes())
                .map_err(|err| format!("{} に書けません: {err}", output.display()))?;
            Ok(if total == 0 {
                format!("{} は空でした（直す NMEA がありません）", input.display())
            } else {
                format!(
                    "{total} 行中 {changed} 行の日付を直して {} に保存しました",
                    output.display()
                )
            })
        })();
        self.status = Some(match result {
            Ok(message) => (Status::Ok, message),
            Err(message) => (Status::Err, message),
        });
    }

    fn proxy_running(&self) -> bool {
        self.proxy
            .as_ref()
            .is_some_and(|handle| handle.is_running())
    }

    fn proxy_snapshot(&self) -> Option<ProxySnapshot> {
        self.proxy.as_ref().map(ProxyHandle::snapshot)
    }

    fn proxy_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("入力").size(size::CAPTION).color(theme::MUTE));
            if common::tool_button(ui, "実機", !self.proxy_from_file, true).clicked() {
                self.proxy_from_file = false;
            }
            if common::tool_button(ui, "ファイル", self.proxy_from_file, true).clicked() {
                self.proxy_from_file = true;
            }
        });
        if self.proxy_from_file {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NMEA ファイル")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let width = (ui.available_width() - space::S).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy_file)
                        .hint_text(RichText::new("capture.nmea").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ポート")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let width = (ui.available_width() - 160.0).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy_port)
                        .hint_text(RichText::new("auto").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
                let _response = ui.add_sized(
                    Vec2::new(120.0, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy_baud)
                        .hint_text(RichText::new("baud").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("TCP 待受")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - 220.0).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.proxy_bind)
                    .hint_text(RichText::new("空欄なら TCP なし").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            ui.checkbox(
                &mut self.proxy_allow_non_loopback,
                RichText::new("ループバック以外を許可").size(size::CAPTION),
            );
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("日付補正")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - metric::ACTION_WIDTH * 2.0 - space::S).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.proxy_offset)
                    .hint_text(RichText::new("auto または日数").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            let running = self.proxy_running();
            if common::primary_button(ui, "開始", !running).clicked() {
                self.start_proxy();
            }
            if common::secondary_button(ui, "停止", running).clicked()
                && let Some(handle) = &mut self.proxy
            {
                handle.stop();
            }
        });
        if let Some(snapshot) = self.proxy_snapshot() {
            let state = if snapshot.running {
                "中継中"
            } else {
                "停止中"
            };
            common::status_line(
                ui,
                if snapshot.running {
                    Status::Ok
                } else {
                    Status::Info
                },
                &format!(
                    "{state} / クライアント {} / 受信 {} 文",
                    snapshot.clients, snapshot.lines
                ),
            );
            if let Some(bound) = &snapshot.bound {
                ui.label(
                    RichText::new(format!("待受 {bound}"))
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            }
            if let Some(ended) = &snapshot.ended {
                ui.label(RichText::new(ended).size(size::CAPTION).color(theme::MUTE));
            }
            egui::ScrollArea::vertical()
                .id_salt("m241-log-proxy")
                .max_height(120.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &snapshot.log {
                        ui.label(
                            RichText::new(line)
                                .size(size::CAPTION)
                                .monospace()
                                .color(theme::MUTE),
                        );
                    }
                });
        }
    }

    fn start_proxy(&mut self) {
        let result = (|| -> Result<(), String> {
            let offset = OffsetSpec::parse(&self.proxy_offset)?;
            let source = if self.proxy_from_file {
                let path = PathBuf::from(self.proxy_file.trim());
                if path.as_os_str().is_empty() {
                    return Err(String::from("中継する NMEA ファイルを指定してください"));
                }
                ProxySource::File(path)
            } else {
                let name = device::resolve_port_name(&self.proxy_port)?;
                let baud = self.proxy_baud.trim().parse::<u32>().map_err(|_| {
                    format!(
                        "ボーレート {:?} が数値ではありません",
                        self.proxy_baud.trim()
                    )
                })?;
                ProxySource::Port { name, baud }
            };
            let bind = self.proxy_bind.trim();
            let bind = if bind.is_empty() {
                None
            } else {
                Some(bind.to_owned())
            };
            let handle = ProxyHandle::start(ProxyConfig {
                source,
                offset,
                bind,
                allow_non_loopback: self.proxy_allow_non_loopback,
            })?;
            self.proxy = Some(handle);
            Ok(())
        })();
        self.status = Some(match result {
            Ok(()) => (Status::Ok, String::from("NMEA 中継を開始しました")),
            Err(message) => (Status::Err, message),
        });
    }

    fn read_panel(&mut self, ui: &mut Ui, screen: &mut LogScreen) {
        for (label, value, hint) in [
            (
                "入力",
                &mut self.read_input,
                "track.bin または capture.nmea",
            ),
            ("出力", &mut self.read_output, "m241-track.gpx"),
            ("日付補正", &mut self.read_offset, "auto または日数"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                let width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(value)
                        .hint_text(RichText::new(hint).size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("形式").size(size::CAPTION).color(theme::MUTE));
            for format in ExportFormat::ALL {
                let selected = self.read_format == format;
                if common::tool_button(ui, &format.extension().to_uppercase(), selected, true)
                    .clicked()
                {
                    self.read_format = format;
                }
            }
            ui.checkbox(
                &mut self.read_nmea,
                RichText::new("NMEA 0183 として読む").size(size::CAPTION),
            );
        });
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.read_force,
                RichText::new("出力先が存在しても上書きする").size(size::CAPTION),
            );
            if common::primary_button(ui, "読み込んで書き出す", true).clicked() {
                self.run_read(screen);
            }
        });
    }

    fn run_read(&mut self, screen: &mut LogScreen) {
        let result = (|| -> Result<String, String> {
            let offset = OffsetSpec::parse(&self.read_offset)?;
            let (loaded, message) = project::read_and_export(
                &self.read_input,
                self.read_nmea,
                self.read_format,
                offset,
                &self.read_output,
                self.read_force,
            )?;
            screen.set_track(loaded.track, loaded.report);
            screen.log_path = self.read_input.trim().to_owned();
            screen.set_export_format(self.read_format);
            screen.set_destination(self.read_output.trim().to_owned());
            Ok(format!("{message}。{}", loaded.note))
        })();
        self.status = Some(match result {
            Ok(message) => (Status::Ok, message),
            Err(message) => (Status::Err, message),
        });
    }
}

/// Whether two paths name the same file, resolving relative spellings.
fn names_same_file(left: &Path, right: &Path) -> bool {
    match (std::path::absolute(left), std::path::absolute(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Draw the transfer bar with a measurement caption, not a motion.
fn progress_bar(ui: &mut Ui, fraction: f32, caption: String) {
    let _bar = ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(ui.available_width())
            .corner_radius(theme::RADIUS)
            .animate(false)
            .text(RichText::new(caption).size(size::CAPTION)),
    );
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ScratchDir, synthetic_dump};
    use super::*;
    use crate::screens::Screen;
    use egui::Vec2;

    fn sample_screen() -> crate::App {
        let mut app = crate::App::default();
        app.select(Screen::Log);
        app.log_mut().show_sample();
        app
    }

    /// An ASCII file stem for a mode's screenshot.
    fn slug(mode: OpsMode) -> &'static str {
        match mode {
            OpsMode::Download => "download",
            OpsMode::Erase => "erase",
            OpsMode::Project => "project",
            OpsMode::Repair => "repair",
            OpsMode::Proxy => "proxy",
            OpsMode::Read => "read",
        }
    }

    #[test]
    fn every_operation_panel_renders_with_text() {
        let screenshot_dir = std::env::var_os("M241_GUI_OPS_SCREENSHOTS");
        for mode in OpsMode::ALL {
            let mut app = sample_screen();
            app.log_mut().ops.mode = mode;
            let image = crate::screenshot::render(&mut app, Screen::Log, Vec2::new(640.0, 400.0));
            assert!(
                image.contains_colour(theme::INK, 24),
                "{} panel rendered no text",
                mode.label()
            );
            // The modes are reviewed from an empty log: that is where the console
            // sits at the top of the page and every field is above the fold. The
            // write is opt-in so `cargo test` leaves no artifacts.
            if let Some(directory) = &screenshot_dir {
                let mut empty = crate::App::default();
                empty.select(Screen::Log);
                empty.log_mut().ops.mode = mode;
                let image =
                    crate::screenshot::render(&mut empty, Screen::Log, Vec2::new(1024.0, 640.0));
                let directory = std::path::Path::new(directory);
                std::fs::create_dir_all(directory).expect("create the screenshot directory");
                let path = directory.join(format!("ops-{}.png", slug(mode)));
                std::fs::write(&path, image.to_png().expect("encode the PNG"))
                    .expect("write the screenshot");
            }
        }
    }

    #[test]
    fn the_erase_action_needs_a_saved_backup_and_the_confirmation() {
        // The button is armed by two independent things, so a stray click cannot
        // reach the destructive path with only one of them.
        let mut ops = LogOps::default();
        assert!(!ops.erase_ready());
        ops.backup = Some((String::from("backup.bin"), 10));
        assert!(!ops.erase_ready());
        ops.erase_confirm = true;
        assert!(ops.erase_ready());

        // And the refusal is explicit about the crate boundary and about the
        // user approval §2.5 of AGENTS.md requires.
        assert!(ERASE_REFUSAL.contains("m241-flash"), "{ERASE_REFUSAL}");
        assert!(
            ERASE_REFUSAL.contains("実行していません"),
            "{ERASE_REFUSAL}"
        );
        assert!(ERASE_REFUSAL.contains("承認"), "{ERASE_REFUSAL}");
    }

    /// Run a finished transfer through a taken-out console, the way `ui` does.
    fn finish(log: &mut LogScreen, purpose: DownloadPurpose, result: Result<Vec<u8>, String>) {
        let mut ops = std::mem::take(&mut log.ops);
        ops.finish(purpose, result, log);
        log.ops = ops;
    }

    #[test]
    fn an_empty_download_file_is_a_stated_outcome_not_a_saved_file() {
        let scratch = ScratchDir::new("empty-download");
        let destination = scratch.join("empty.bin");
        let mut app = sample_screen();
        let log = app.log_mut();
        log.ops.dump_path = destination.display().to_string();
        finish(log, DownloadPurpose::Dump, Ok(Vec::new()));
        assert!(
            log.ops
                .status
                .as_ref()
                .is_some_and(|(status, message)| *status == Status::Info && message.contains("空")),
            "got {:?}",
            log.ops.status
        );
        assert!(!destination.exists());
    }

    #[test]
    fn a_finished_download_replaces_the_track_and_names_its_file() {
        let scratch = ScratchDir::new("finished-download");
        let destination = scratch.join("track.bin");
        let bytes = synthetic_dump(&[(1_700_000_000, 35.68, 139.76, 10.0)]);
        let mut app = sample_screen();
        let log = app.log_mut();
        log.ops.dump_path = destination.display().to_string();
        finish(log, DownloadPurpose::Dump, Ok(bytes));
        assert_eq!(log.track().points.len(), 1);
        assert_eq!(log.log_path, destination.display().to_string());
        assert!(destination.exists());
        assert!(
            log.ops
                .status
                .as_ref()
                .is_some_and(|(status, _)| *status == Status::Ok),
            "got {:?}",
            log.ops.status
        );
    }

    #[test]
    fn a_failed_transfer_is_reported_and_keeps_the_previous_track() {
        let mut app = sample_screen();
        let before = app.log_mut().track().points.len();
        let log = app.log_mut();
        finish(log, DownloadPurpose::Dump, Err(String::from("no device")));
        assert_eq!(log.track().points.len(), before);
        assert!(
            log.ops.status.as_ref().is_some_and(
                |(status, message)| *status == Status::Err && message.contains("no device")
            ),
            "got {:?}",
            log.ops.status
        );
    }
}
