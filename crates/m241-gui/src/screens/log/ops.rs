//! The ログ操作 console: the CLI's log and NMEA commands as screen controls.
//!
//! One panel on the Log screen, with a chip row rather than a second rail: the
//! staircase is the navigation's signature, and a local mode row borrows the
//! export chips' language instead. Each mode names the CLI command it replaces,
//! so a person can check the two against each other.
//!
//! # State and the panels that own it
//!
//! Each panel owns the fields it draws — port, overwrite, paths and its own
//! status — in a small struct below, so a refusal can only name a control that
//! the same panel shows. Download and the erase backup are the same transfer, so
//! both panels draw the port, the overwrite checkbox and the progress: either one
//! can be the panel a person is standing on when the transfer asks for them. The
//! one shared worker is the thread; a message is labelled with the panel that
//! started it and only that panel's status line shows it.
//!
//! What runs where:
//!
//! * download and the erase backup use the PMTK182 path on a thread;
//! * the log erase itself cannot run from this panel. The GUI links
//!   `m241-flash` for the Firmware screen's write path (user decision,
//!   2026-11-23), but the log console calls no erase client: `log erase`
//!   destroys the user's track, which needs the user's own instruction, so the
//!   confirmation stops at a refusal that says so;
//! * project save/load, `log read`, NMEA repair and the NMEA relay are offline
//!   file work, except the relay's serial source. The file work itself lives in
//!   `device`, `project`, `nmea` and `proxy`; the panels format the result.

use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
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

/// The refusal the erase action ends at.
///
/// The GUI links `m241-flash` for the Firmware screen's write path (user
/// decision, 2026-11-23), but this console calls no erase client, and `log erase`
/// destroys the user's track — which needs the user's own instruction — so the
/// screen takes a backup and a confirmation and stops here.
const ERASE_REFUSAL: &str = "バックアップは保存済みですが、この画面から消去は実行していません。GUI はファーム書き込み用に m241-flash をリンクしていますが、この画面からログ消去の経路は呼び出していません。CLI の `m241 log erase` と同じ手順ですが、実行には端末のログを消すことへのユーザーの明示的な承認が別途必要です。";

/// The port row's label, on both transfer panels.
///
/// A refusal that asks for the port quotes this exact string, so the label it
/// names is the label the panel draws (see [`TransferPanel::refusal`]).
const PORT_LABEL: &str = "ポート";

/// The overwrite checkbox's label, on both transfer panels.
///
/// Quoted by the refusal that fires when a destination exists, for the same
/// reason as [`PORT_LABEL`]: the earlier layout drew it only on the download
/// panel, so the erase panel asked for a control it did not have.
const OVERWRITE_LABEL: &str = "保存先が存在しても上書きする";

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

/// A status sentence and the panel that produced it.
///
/// The mode is part of the value so an error from the repair panel cannot appear
/// under the erase panel; the console draws only the current mode's entry.
#[derive(Debug)]
struct PanelStatus {
    mode: OpsMode,
    status: Status,
    message: String,
}

/// The port, destination and overwrite settings of one transfer panel.
///
/// Download and the erase backup are two instances rather than one shared value:
/// each panel draws its own controls, so the sentence that asks for the port or
/// for the overwrite has a field or checkbox beside it.
#[derive(Debug)]
struct TransferPanel {
    port: String,
    ports: Vec<String>,
    destination: String,
    force: bool,
}

impl TransferPanel {
    fn new(destination: &str) -> Self {
        Self {
            port: String::new(),
            ports: Vec::new(),
            destination: String::from(destination),
            force: false,
        }
    }

    /// Why this transfer cannot start, or `None` when it is ready.
    ///
    /// The overwrite sentence quotes [`OVERWRITE_LABEL`] because the checkbox
    /// with that label is drawn by this panel, whichever panel that is.
    fn refusal(&self) -> Option<String> {
        let destination = self.destination.trim();
        if destination.is_empty() {
            return Some(String::from("保存先を入力してください"));
        }
        if !self.force {
            let path = PathBuf::from(destination);
            if path.exists() {
                return Some(format!(
                    "{} はすでに存在します。上書きするには「{OVERWRITE_LABEL}」を入れてください",
                    path.display()
                ));
            }
        }
        None
    }
}

/// The download panel's state: one transfer whose result becomes the track.
#[derive(Debug)]
struct DownloadPanel {
    transfer: TransferPanel,
}

/// The erase panel's state: a mandatory backup, then a confirmation, then a
/// refusal that names the crate boundary.
#[derive(Debug)]
struct ErasePanel {
    transfer: TransferPanel,
    /// The saved backup, as (path, bytes).
    saved: Option<(String, usize)>,
    confirm: bool,
}

impl ErasePanel {
    /// The erase button is armed only by a saved backup plus the confirmation.
    fn ready(&self) -> bool {
        self.saved.is_some() && self.confirm
    }
}

/// The project panel's fields, in the CLI project schema's order.
#[derive(Debug)]
struct ProjectPanel {
    path: String,
    log: String,
    photos: String,
    output: String,
    offset: String,
    nmea: bool,
    format: ExportFormat,
    kmz: bool,
}

impl Default for ProjectPanel {
    fn default() -> Self {
        Self {
            path: String::from("m241-project.json"),
            log: String::new(),
            photos: String::new(),
            output: String::from("m241-track.gpx"),
            offset: String::from("0"),
            nmea: false,
            format: ExportFormat::Gpx,
            kmz: false,
        }
    }
}

/// The NMEA repair panel's fields.
#[derive(Debug)]
struct RepairPanel {
    input: String,
    output: String,
    offset: String,
}

impl Default for RepairPanel {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::from("repaired.nmea"),
            offset: String::from("auto"),
        }
    }
}

/// The NMEA relay panel's fields.
#[derive(Debug)]
struct ProxyPanel {
    from_file: bool,
    port: String,
    baud: String,
    file: String,
    bind: String,
    allow_non_loopback: bool,
    offset: String,
    /// `--max-lines`: stop after this many lines. Empty means run until stopped.
    max_lines: String,
    handle: Option<ProxyHandle>,
}

impl Default for ProxyPanel {
    fn default() -> Self {
        Self {
            from_file: false,
            port: String::new(),
            baud: String::from("38400"),
            file: String::new(),
            bind: String::from("127.0.0.1:10110"),
            allow_non_loopback: false,
            offset: String::from("auto"),
            max_lines: String::new(),
            handle: None,
        }
    }
}

/// The `log read` panel's fields.
#[derive(Debug)]
struct ReadPanel {
    input: String,
    nmea: bool,
    format: ExportFormat,
    offset: String,
    output: String,
    force: bool,
}

impl Default for ReadPanel {
    fn default() -> Self {
        Self {
            input: String::new(),
            nmea: false,
            format: ExportFormat::Gpx,
            offset: String::from("auto"),
            output: String::from("m241-track.gpx"),
            force: false,
        }
    }
}

/// Everything the console owns. A single field on [`LogScreen`], so the screen's
/// own state stays about the track.
#[derive(Debug, Default)]
pub(super) struct LogOps {
    mode: OpsMode,
    /// The status of the panel that produced it; see [`PanelStatus`].
    status: Option<PanelStatus>,
    /// The one transfer worker (download or erase backup).
    job: Option<DownloadJob>,
    /// Bytes received so far.
    progress: Option<(usize, usize)>,
    download: DownloadPanel,
    erase: ErasePanel,
    project: ProjectPanel,
    repair: RepairPanel,
    proxy: ProxyPanel,
    read: ReadPanel,
}

impl Default for DownloadPanel {
    fn default() -> Self {
        Self {
            transfer: TransferPanel::new("m241-log.bin"),
        }
    }
}

impl Default for ErasePanel {
    fn default() -> Self {
        Self {
            transfer: TransferPanel::new("m241-log-backup.bin"),
            saved: None,
            confirm: false,
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
            // Six mode chips do not fit the drawer's 332pt on one row, and a
            // `horizontal_wrapped` cannot break between them: each chip is a
            // `ui.scope`, and a scope is placed without a wrap check. Two
            // explicit rows keep every mode reachable instead of squeezing the
            // last two into vertical text.
            ui.horizontal(|ui| self.mode_chips(ui, &OpsMode::ALL[..3]));
            ui.horizontal(|ui| self.mode_chips(ui, &OpsMode::ALL[3..]));
            ui.add_space(space::S);
            match self.mode {
                OpsMode::Download => self.download_panel(ui),
                OpsMode::Erase => self.erase_panel(ui),
                OpsMode::Project => self.project_panel(ui, screen),
                OpsMode::Repair => self.repair_panel(ui),
                OpsMode::Proxy => self.proxy_panel(ui),
                OpsMode::Read => self.read_panel(ui, screen),
            }
            // Only the panel that produced the sentence shows it: a repair error
            // under the erase panel reads as an erase error.
            if let Some(entry) = &self.status
                && entry.mode == self.mode
            {
                ui.add_space(space::XS);
                common::status_line(ui, entry.status, &entry.message);
            }
        });
        if self.job.is_some() || self.proxy_running() {
            ui.ctx().request_repaint_after(REPAINT_INTERVAL);
        }
    }

    /// One row of mode chips, the selected one marked.
    ///
    /// Split out so [`Self::ui`] can break the six modes across two rows
    /// explicitly; a wrapping layout cannot see inside a scoped button.
    fn mode_chips(&mut self, ui: &mut Ui, modes: &[OpsMode]) {
        for &mode in modes {
            if common::tool_button(ui, mode.label(), self.mode == mode, true).clicked() {
                self.mode = mode;
            }
        }
    }

    /// Record a sentence against the panel it belongs to.
    fn set_status(&mut self, mode: OpsMode, status: Status, message: impl Into<String>) {
        self.status = Some(PanelStatus {
            mode,
            status,
            message: message.into(),
        });
    }

    /// Take whatever the download thread has produced since the last frame.
    fn poll(&mut self, screen: &mut LogScreen) {
        let mut progress = None;
        let mut finished = None;
        let mut disconnected = None;
        if let Some(job) = self.job.as_ref() {
            let purpose = job.purpose();
            loop {
                match job.try_recv() {
                    Ok(DownloadMsg::Progress { received, total }) => {
                        progress = Some((received, total));
                    }
                    Ok(DownloadMsg::Done(result)) => {
                        finished = Some((purpose, result));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = Some(purpose);
                        break;
                    }
                }
            }
        }
        if let Some(progress) = progress {
            self.progress = Some(progress);
        }
        if let Some((purpose, result)) = finished {
            self.job = None;
            self.progress = None;
            self.finish(purpose, result, screen);
        } else if let Some(purpose) = disconnected {
            self.job = None;
            self.progress = None;
            self.set_status(
                purpose_mode(purpose),
                Status::Err,
                "吸い出しのスレッドが終了しました。ポートを確認してください",
            );
        }
    }

    /// Apply a finished transfer to the panel that started it.
    fn finish(
        &mut self,
        purpose: DownloadPurpose,
        result: Result<Vec<u8>, String>,
        screen: &mut LogScreen,
    ) {
        let mode = purpose_mode(purpose);
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(message) => {
                self.set_status(mode, Status::Err, message);
                return;
            }
        };
        match purpose {
            DownloadPurpose::Dump => self.finish_dump(&bytes, screen),
            DownloadPurpose::Backup => self.finish_backup(&bytes),
        }
    }

    /// Apply a downloaded log: save it, parse it and show it.
    fn finish_dump(&mut self, bytes: &[u8], screen: &mut LogScreen) {
        let destination = self.download.transfer.destination.clone();
        let force = self.download.transfer.force;
        match device::save_and_parse(bytes, &destination, force) {
            Ok(DumpOutcome::Empty) => {
                self.set_status(
                    OpsMode::Download,
                    Status::Info,
                    "端末のログは空でした（0 バイト）。保存はしていません",
                );
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
                self.set_status(
                    OpsMode::Download,
                    Status::Ok,
                    format!(
                        "{} に {} を保存し、{points} 点を読み込みました（破損 {damaged} ブロック）",
                        saved.path.display(),
                        common::bytes(saved.bytes)
                    ),
                );
            }
            Err(message) => self.set_status(OpsMode::Download, Status::Err, message),
        }
    }

    /// Apply an erase backup: save the copy the confirmation depends on.
    fn finish_backup(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            self.erase.saved = None;
            self.erase.confirm = false;
            self.set_status(
                OpsMode::Erase,
                Status::Info,
                "端末のログは空でした（0 バイト）。バックアップも消去もしません",
            );
            return;
        }
        let destination = self.erase.transfer.destination.clone();
        let force = self.erase.transfer.force;
        match device::write_dump(&destination, bytes, force) {
            Ok(path) => {
                self.erase.saved = Some((path.display().to_string(), bytes.len()));
                self.erase.confirm = false;
                self.set_status(
                    OpsMode::Erase,
                    Status::Ok,
                    format!(
                        "バックアップ {} を保存しました（{}）。消去はまだ行っていません",
                        path.display(),
                        common::bytes(bytes.len())
                    ),
                );
            }
            Err(message) => {
                self.erase.saved = None;
                self.erase.confirm = false;
                self.set_status(OpsMode::Erase, Status::Err, message);
            }
        }
    }

    /// Whether a transfer is in flight (both transfer panels share the worker).
    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn start_download(&mut self, purpose: DownloadPurpose) {
        let mode = purpose_mode(purpose);
        if self.busy() {
            self.set_status(mode, Status::Info, "吸い出しが進行中です");
            return;
        }
        let (refusal, port) = {
            let transfer = match purpose {
                DownloadPurpose::Dump => &self.download.transfer,
                DownloadPurpose::Backup => &self.erase.transfer,
            };
            (transfer.refusal(), transfer.port.clone())
        };
        if let Some(message) = refusal {
            self.set_status(mode, Status::Err, message);
            return;
        }
        self.job = Some(device::start_download(port, purpose));
        self.progress = Some((0, 0));
        self.set_status(
            mode,
            Status::Info,
            "吸い出しを開始しました。ログの転送中は端末の記録が止まります",
        );
    }

    fn download_panel(&mut self, ui: &mut Ui) {
        let busy = self.busy();
        let list_clicked = transfer_header(ui, &mut self.download.transfer);
        if list_clicked {
            self.report_port_list(OpsMode::Download);
        }
        let mut started = false;
        // The destination and the download action stack: the drawer is 332pt,
        // where a caption, a field and a 128pt button on one line sit exactly on
        // the clip edge.
        ui.label(
            RichText::new("保存先")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.add_sized(
            Vec2::new(ui.available_width(), metric::ROW),
            egui::TextEdit::singleline(&mut self.download.transfer.destination)
                .hint_text(RichText::new("m241-log.bin").size(size::CAPTION))
                .font(egui::TextStyle::Monospace),
        );
        if common::primary_button(ui, "吸い出す", !busy).clicked() {
            started = true;
        }
        if started {
            self.start_download(DownloadPurpose::Dump);
        }
        self.transfer_progress(ui);
        ui.label(
            RichText::new(
                "転送後に点を作り、破損ブロック数を表示します。空のログはファイルを作りません。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
    }

    fn erase_panel(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new(
                "端末のログを消す前に、必ずバックアップを保存します。保存が終わるまで「消去」は有効になりません。",
            )
            .size(size::UI)
            .color(theme::INK),
        );
        let busy = self.busy();
        let list_clicked = transfer_header(ui, &mut self.erase.transfer);
        if list_clicked {
            self.report_port_list(OpsMode::Erase);
        }
        let mut started = false;
        // The destination and the backup action stack: the drawer is 332pt and a
        // caption, a field and a 205pt button do not share one line there.
        ui.label(
            RichText::new("バックアップ先")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.add_sized(
            Vec2::new(ui.available_width(), metric::ROW),
            egui::TextEdit::singleline(&mut self.erase.transfer.destination)
                .hint_text(RichText::new("m241-log-backup.bin").size(size::CAPTION))
                .font(egui::TextStyle::Monospace),
        );
        if common::secondary_button(ui, "バックアップを保存", !busy).clicked() {
            started = true;
        }

        if started {
            self.start_download(DownloadPurpose::Backup);
        }
        // The progress lives with the panel that can show it, so an erase-panel
        // backup is not a silent minute.
        self.transfer_progress(ui);
        if let Some((path, bytes)) = self.erase.saved.clone() {
            common::status_line(
                ui,
                Status::Ok,
                &format!(
                    "バックアップ {path} を保存しました（{}）。消去はまだ行っていません",
                    common::bytes(bytes)
                ),
            );
        }
        // The consent sentence is a caption above its checkbox: the full
        // sentence at caption size is wider than the drawer, and a checkbox
        // does not wrap its label. The control itself stays one word, next to
        // the sentence it answers.
        ui.label(
            RichText::new("バックアップを保存済みで、この端末のログを消すことに同意します")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.checkbox(
            &mut self.erase.confirm,
            RichText::new("同意する").size(size::CAPTION),
        );
        let ready = self.erase.ready();
        let mut pressed = false;
        if common::secondary_button(ui, "消去", ready).clicked() {
            pressed = true;
        }
        if !ready {
            let reason = if self.erase.saved.is_none() {
                "先にバックアップを保存してください"
            } else {
                "同意のチェックを入れると「消去」が有効になります"
            };
            ui.label(RichText::new(reason).size(size::CAPTION).color(theme::MUTE));
        }
        if pressed {
            self.press_erase();
        }
        ui.label(
            RichText::new(
                "GUI はファーム書き込み用に m241-flash をリンクしていますが、この画面からログ消去は呼び出しません。バックアップと確認までを担い、実行は CLI とユーザーの明示的な承認に残します。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
    }

    /// Answer the erase button: the refusal, which is the only outcome this
    /// screen can produce. Both gates already decide whether the button is
    /// enabled, so a press can only land when they are both satisfied.
    fn press_erase(&mut self) {
        if self.erase.ready() {
            self.set_status(OpsMode::Erase, Status::Err, ERASE_REFUSAL);
        }
    }

    fn list_ports(transfer: &mut TransferPanel) -> Result<usize, String> {
        let ports = m241_core::protocol::Port::list()
            .map_err(|err| format!("ポートを列挙できません: {err}"))?;
        if transfer.port.trim().is_empty()
            && let Some(first) = ports.first()
        {
            transfer.port = first.clone();
        }
        transfer.ports = ports;
        Ok(transfer.ports.len())
    }

    /// Refresh the active transfer panel's port list and say how many were found.
    fn report_port_list(&mut self, mode: OpsMode) {
        let transfer = match mode {
            OpsMode::Download => &mut self.download.transfer,
            _ => &mut self.erase.transfer,
        };
        match Self::list_ports(transfer) {
            Ok(count) => self.set_status(
                mode,
                Status::Info,
                format!("ポートを {count} 件見つけました"),
            ),
            Err(message) => self.set_status(mode, Status::Err, message),
        }
    }

    /// Draw the transfer progress while the one worker is running.
    fn transfer_progress(&self, ui: &mut Ui) {
        let Some((received, total)) = self.progress else {
            return;
        };
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

    fn project_panel(&mut self, ui: &mut Ui, screen: &mut LogScreen) {
        // Each row is a caption and a field that fills the drawer: the buttons
        // moved to their own rows, so no row has to reserve width for one.
        for (label, value, hint) in [
            ("プロジェクト", &mut self.project.path, "m241-project.json"),
            ("ログ", &mut self.project.log, "track.bin"),
            ("写真", &mut self.project.photos, "写真フォルダ（任意）"),
            ("書き出し先", &mut self.project.output, "track.gpx"),
            ("日付補正", &mut self.project.offset, "0（日数）"),
        ] {
            ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
            let _response = ui.add_sized(
                Vec2::new(ui.available_width(), metric::ROW),
                egui::TextEdit::singleline(value)
                    .hint_text(RichText::new(hint).size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
        }
        // The chips take two rows in the drawer (a scope is placed without a
        // wrap check, so `horizontal_wrapped` cannot break between them), and
        // the read options and the three actions each get their own line.
        ui.label(RichText::new("形式").size(size::CAPTION).color(theme::MUTE));
        ui.horizontal(|ui| {
            for &format in &ExportFormat::ALL[..3] {
                self.project_format_chip(ui, format);
            }
        });
        ui.horizontal(|ui| {
            for &format in &ExportFormat::ALL[3..] {
                self.project_format_chip(ui, format);
            }
            if common::tool_button(ui, "KMZ", self.project.kmz, true).clicked() {
                self.project.kmz = true;
            }
        });
        ui.checkbox(
            &mut self.project.nmea,
            RichText::new("NMEA 入力").size(size::CAPTION),
        );
        let mut action = None;
        ui.horizontal(|ui| {
            if common::secondary_button(ui, "保存", true).clicked() {
                action = Some(ProjectAction::Save);
            }
            if common::secondary_button(ui, "読込", true).clicked() {
                action = Some(ProjectAction::Load);
            }
        });
        if common::secondary_button(ui, "読込んで書き出す", true).clicked() {
            action = Some(ProjectAction::LoadAndWrite);
        }
        match action {
            Some(ProjectAction::Save) => self.save_project(),
            Some(ProjectAction::Load) => self.load_project(screen, false),
            Some(ProjectAction::LoadAndWrite) => self.load_project(screen, true),
            None => {}
        }
    }

    /// One project output-format chip; KMZ is drawn separately because it is a
    /// writer choice, not an [`ExportFormat`].
    fn project_format_chip(&mut self, ui: &mut Ui, format: ExportFormat) {
        let selected = !self.project.kmz && self.project.format == format;
        if common::tool_button(ui, &format.extension().to_uppercase(), selected, true).clicked() {
            self.project.kmz = false;
            self.project.format = format;
        }
    }

    fn save_project(&mut self) {
        let offset = match self.project.offset.trim().parse::<i32>() {
            Ok(offset) => offset,
            Err(_) => {
                self.set_status(
                    OpsMode::Project,
                    Status::Err,
                    format!(
                        "日付補正 {:?} は整数の日数ではありません",
                        self.project.offset
                    ),
                );
                return;
            }
        };
        let format = if self.project.kmz {
            "kmz"
        } else {
            self.project.format.extension()
        };
        match project::save_project(&project::ProjectSave {
            project: &self.project.path,
            log: &self.project.log,
            photos: &self.project.photos,
            output: &self.project.output,
            utc_offset_days: offset,
            export_format: format,
            nmea: self.project.nmea,
        }) {
            Ok(path) => self.set_status(
                OpsMode::Project,
                Status::Ok,
                format!("プロジェクト {} を保存しました（{format}）", path.display()),
            ),
            Err(message) => self.set_status(OpsMode::Project, Status::Err, message),
        }
    }

    fn load_project(&mut self, screen: &mut LogScreen, write: bool) {
        let result = (|| -> Result<String, String> {
            let document = ProjectDocument::read(&self.project.path)?;
            self.refuse_unsupported_project(&document)?;
            let loaded = project::load_log(
                &document.log,
                document.input_is_nmea,
                OffsetSpec::Days(document.utc_offset_days),
            )?;
            let format = project_export_format(&document)?;
            let mut message = self.adopt_project(screen, &document, loaded, format);
            if write {
                screen.export()?;
                message.push_str(&format!("。{} に書き出しました", document.output.display()));
            }
            Ok(message)
        })();
        match result {
            Ok(message) => self.set_status(OpsMode::Project, Status::Ok, message),
            Err(message) => self.set_status(OpsMode::Project, Status::Err, message),
        }
    }

    /// The two refusals `log project load` makes before it reads the log: a KMZ
    /// project needs a raw dump and a photo folder, and the GUI must not succeed
    /// where the CLI refuses the same file.
    fn refuse_unsupported_project(&self, document: &ProjectDocument) -> Result<(), String> {
        if !document.export_format.eq_ignore_ascii_case("kmz") {
            return Ok(());
        }
        if document.input_is_nmea {
            return Err(format!(
                "{}: KMZ のプロジェクトは NMEA 入力ではなく生ダンプが必要です。CLI と同じく読み込みません",
                self.project.path
            ));
        }
        if document.photos.is_none() {
            return Err(format!(
                "{}: KMZ のプロジェクトには写真フォルダが必要ですが、photos が null です。CLI と同じく読み込みません",
                self.project.path
            ));
        }
        Ok(())
    }

    /// Copy a loaded project onto the panel's fields and the screen.
    fn adopt_project(
        &mut self,
        screen: &mut LogScreen,
        document: &ProjectDocument,
        loaded: project::LoadedLog,
        format: Option<ExportFormat>,
    ) -> String {
        self.project.log = document.log.display().to_string();
        self.project.offset = document.utc_offset_days.to_string();
        self.project.nmea = document.input_is_nmea;
        self.project.photos = document
            .photos
            .as_ref()
            .map(|photos| photos.display().to_string())
            .unwrap_or_default();
        self.project.output = document.output.display().to_string();
        let project::LoadedLog {
            track,
            report,
            note,
            facts,
        } = loaded;
        screen.set_track(track, report);
        // The stored project's own day correction and, for a capture, the line
        // counts are what `log read`/`log project load` report; the export bar
        // writes them into JSON the same way.
        screen.set_read_facts(facts);
        screen.log_path = document.log.display().to_string();
        self.project.kmz = format.is_none();
        match format {
            Some(format) => {
                self.project.format = format;
                screen.set_export_format(format);
                screen.photo_path = self.project.photos.clone();
            }
            None => {
                screen.set_kmz_export();
                screen.photo_path = self.project.photos.clone();
                if let Some(photos) = &document.photos {
                    let _loaded = screen.load_photos_from_path(photos);
                }
            }
        }
        screen.set_destination(document.output.display().to_string());
        format!("プロジェクトを読み込みました: {note}")
    }

    fn repair_panel(&mut self, ui: &mut Ui) {
        for (label, value, hint) in [
            ("入力", &mut self.repair.input, "capture.nmea"),
            ("出力", &mut self.repair.output, "repaired.nmea"),
            ("日付補正", &mut self.repair.offset, "auto または日数"),
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
            let offset = OffsetSpec::parse(&self.repair.offset)?;
            let input = PathBuf::from(self.repair.input.trim());
            let output = PathBuf::from(self.repair.output.trim());
            let summary = nmea::repair_file(&input, &output, offset)?;
            Ok(if summary.lines == 0 {
                format!("{} は空でした（直す NMEA がありません）", input.display())
            } else {
                format!(
                    "{} 行中 {} 行の日付を直して {} に保存しました",
                    summary.lines,
                    summary.changed,
                    output.display()
                )
            })
        })();
        match result {
            Ok(message) => self.set_status(OpsMode::Repair, Status::Ok, message),
            Err(message) => self.set_status(OpsMode::Repair, Status::Err, message),
        }
    }

    fn proxy_running(&self) -> bool {
        self.proxy
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_running())
    }

    fn proxy_snapshot(&self) -> Option<ProxySnapshot> {
        self.proxy.handle.as_ref().map(ProxyHandle::snapshot)
    }

    fn proxy_panel(&mut self, ui: &mut Ui) {
        self.proxy_source_controls(ui);
        self.proxy_bind_controls(ui);
        self.proxy_run_controls(ui);
        self.proxy_snapshot_view(ui);
    }

    /// The relay's source: the live unit or a captured file.
    fn proxy_source_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("入力").size(size::CAPTION).color(theme::MUTE));
            if common::tool_button(ui, "実機", !self.proxy.from_file, true).clicked() {
                self.proxy.from_file = false;
            }
            if common::tool_button(ui, "ファイル", self.proxy.from_file, true).clicked() {
                self.proxy.from_file = true;
            }
        });
        if self.proxy.from_file {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NMEA ファイル")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let width = (ui.available_width() - space::S).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy.file)
                        .hint_text(RichText::new("capture.nmea").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(PORT_LABEL)
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let width = (ui.available_width() - 160.0).max(120.0);
                let _response = ui.add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy.port)
                        .hint_text(RichText::new("auto").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
                let _response = ui.add_sized(
                    Vec2::new(120.0, metric::ROW),
                    egui::TextEdit::singleline(&mut self.proxy.baud)
                        .hint_text(RichText::new("baud").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
            });
        }
    }

    /// The optional TCP listener and the loopback gate.
    fn proxy_bind_controls(&mut self, ui: &mut Ui) {
        // Stacked for the drawer: a caption, a field and the loopback checkbox
        // are wider than 332pt on one line.
        ui.label(
            RichText::new("TCP 待受")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        let _response = ui.add_sized(
            Vec2::new(ui.available_width(), metric::ROW),
            egui::TextEdit::singleline(&mut self.proxy.bind)
                .hint_text(RichText::new("空欄なら TCP なし").size(size::CAPTION))
                .font(egui::TextStyle::Monospace),
        );
        ui.checkbox(
            &mut self.proxy.allow_non_loopback,
            RichText::new("ループバック以外を許可").size(size::CAPTION),
        );
    }

    /// The date correction, the optional line limit and start/stop.
    fn proxy_run_controls(&mut self, ui: &mut Ui) {
        // Stacked for the drawer: the offset, the line limit and the two
        // transport actions do not fit one 332pt row.
        ui.label(
            RichText::new("日付補正")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        let _response = ui.add_sized(
            Vec2::new(ui.available_width(), metric::ROW),
            egui::TextEdit::singleline(&mut self.proxy.offset)
                .hint_text(RichText::new("auto または日数").size(size::CAPTION))
                .font(egui::TextStyle::Monospace),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("最大行数")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let _response = ui.add_sized(
                Vec2::new(110.0, metric::ROW),
                egui::TextEdit::singleline(&mut self.proxy.max_lines)
                    .hint_text(RichText::new("無制限").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
        });
        let running = self.proxy_running();
        ui.horizontal(|ui| {
            if common::primary_button(ui, "開始", !running).clicked() {
                self.start_proxy();
            }
            if common::secondary_button(ui, "停止", running).clicked()
                && let Some(handle) = &mut self.proxy.handle
            {
                handle.stop();
            }
        });
    }

    /// The relay's state, bound socket, end reason and recent lines.
    fn proxy_snapshot_view(&mut self, ui: &mut Ui) {
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
            let offset = OffsetSpec::parse(&self.proxy.offset)?;
            let max_lines = match self.proxy.max_lines.trim() {
                "" => None,
                text => {
                    let lines = text
                        .parse::<usize>()
                        .map_err(|_| format!("最大行数 {text:?} が数値ではありません"))?;
                    if lines == 0 {
                        return Err(String::from("最大行数は 1 以上にしてください"));
                    }
                    Some(lines)
                }
            };
            let source = if self.proxy.from_file {
                let path = PathBuf::from(self.proxy.file.trim());
                if path.as_os_str().is_empty() {
                    return Err(String::from("中継する NMEA ファイルを指定してください"));
                }
                ProxySource::File(path)
            } else {
                let name = device::resolve_port_name(&self.proxy.port)?;
                let baud = self.proxy.baud.trim().parse::<u32>().map_err(|_| {
                    format!(
                        "ボーレート {:?} が数値ではありません",
                        self.proxy.baud.trim()
                    )
                })?;
                ProxySource::Port { name, baud }
            };
            let bind = self.proxy.bind.trim();
            let bind = if bind.is_empty() {
                None
            } else {
                Some(bind.to_owned())
            };
            let handle = ProxyHandle::start(ProxyConfig {
                source,
                offset,
                bind,
                allow_non_loopback: self.proxy.allow_non_loopback,
                max_lines,
            })?;
            self.proxy.handle = Some(handle);
            Ok(())
        })();
        match result {
            Ok(()) => self.set_status(OpsMode::Proxy, Status::Ok, "NMEA 中継を開始しました"),
            Err(message) => self.set_status(OpsMode::Proxy, Status::Err, message),
        }
    }

    fn read_panel(&mut self, ui: &mut Ui, screen: &mut LogScreen) {
        for (label, value, hint) in [
            (
                "入力",
                &mut self.read.input,
                "track.bin または capture.nmea",
            ),
            ("出力", &mut self.read.output, "m241-track.gpx"),
            ("日付補正", &mut self.read.offset, "auto または日数"),
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
        // The format chips take two rows in the drawer, and the read options
        // and the action each sit on their own line: five scoped chips plus a
        // checkbox cannot share 332pt (a scope is placed without a wrap check).
        ui.label(RichText::new("形式").size(size::CAPTION).color(theme::MUTE));
        ui.horizontal(|ui| {
            for &format in &ExportFormat::ALL[..3] {
                self.read_format_chip(ui, format);
            }
        });
        ui.horizontal(|ui| {
            for &format in &ExportFormat::ALL[3..] {
                self.read_format_chip(ui, format);
            }
        });
        ui.checkbox(
            &mut self.read.nmea,
            RichText::new("NMEA 0183 として読む").size(size::CAPTION),
        );
        let mut started = false;
        ui.checkbox(
            &mut self.read.force,
            RichText::new("出力先が存在しても上書きする").size(size::CAPTION),
        );
        if common::primary_button(ui, "読み込んで書き出す", true).clicked() {
            started = true;
        }
        if started {
            self.run_read(screen);
        }
    }

    /// One `log read` output-format chip.
    ///
    /// A method rather than a loop body so the two drawer rows share the one
    /// selection rule; the panel has no KMZ writer, so there is no archive chip.
    fn read_format_chip(&mut self, ui: &mut Ui, format: ExportFormat) {
        let selected = self.read.format == format;
        if common::tool_button(ui, &format.extension().to_uppercase(), selected, true).clicked() {
            self.read.format = format;
        }
    }

    fn run_read(&mut self, screen: &mut LogScreen) {
        let result = (|| -> Result<String, String> {
            let offset = OffsetSpec::parse(&self.read.offset)?;
            let (loaded, message) = project::read_and_export(
                &self.read.input,
                self.read.nmea,
                self.read.format,
                offset,
                &self.read.output,
                self.read.force,
            )?;
            let facts = loaded.facts;
            screen.set_track(loaded.track, loaded.report);
            // Set after `set_track`, which clears the facts: this read is the
            // source they describe.
            screen.set_read_facts(facts);
            screen.log_path = self.read.input.trim().to_owned();
            screen.set_export_format(self.read.format);
            screen.set_destination(self.read.output.trim().to_owned());
            Ok(format!("{message}。{}", loaded.note))
        })();
        match result {
            Ok(message) => self.set_status(OpsMode::Read, Status::Ok, message),
            Err(message) => self.set_status(OpsMode::Read, Status::Err, message),
        }
    }
}

/// What a project-panel button asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectAction {
    Save,
    Load,
    LoadAndWrite,
}

/// The mode a transfer's status belongs to.
fn purpose_mode(purpose: DownloadPurpose) -> OpsMode {
    match purpose {
        DownloadPurpose::Dump => OpsMode::Download,
        DownloadPurpose::Backup => OpsMode::Erase,
    }
}

/// The writer a project names: `None` for the KMZ archive, `Some` for a
/// track-only export format.
fn project_export_format(document: &ProjectDocument) -> Result<Option<ExportFormat>, String> {
    if document.export_format.eq_ignore_ascii_case("kmz") {
        return Ok(None);
    }
    ExportFormat::parse(&document.export_format)
        .map(Some)
        .ok_or_else(|| format!("{:?} は書き出し形式ではありません", document.export_format))
}

/// Draw the port row, the port list and the overwrite checkbox both transfer
/// panels share; return whether 一覧 was pressed.
///
/// The labels come from [`PORT_LABEL`] and [`OVERWRITE_LABEL`] because a refusal
/// quotes them; see [`TransferPanel::refusal`].
fn transfer_header(ui: &mut Ui, transfer: &mut TransferPanel) -> bool {
    let mut list_clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(PORT_LABEL)
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        let width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
        let _response = ui.add_sized(
            Vec2::new(width, metric::ROW),
            egui::TextEdit::singleline(&mut transfer.port)
                .hint_text(RichText::new("auto または /dev/ttyUSB0").size(size::CAPTION))
                .font(egui::TextStyle::Monospace),
        );
        if common::tool_button(ui, "一覧", false, true).clicked() {
            list_clicked = true;
        }
    });
    if !transfer.ports.is_empty() {
        ui.label(
            RichText::new(format!("見つかったポート: {}", transfer.ports.join("、")))
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
    }
    ui.checkbox(
        &mut transfer.force,
        RichText::new(OVERWRITE_LABEL).size(size::CAPTION),
    );
    list_clicked
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
    use super::super::project::{NmeaCounts, ReadFacts};
    use super::super::test_support::{ScratchDir, screen_texts, synthetic_dump};
    use super::*;
    use crate::screens::Screen;
    use egui::Vec2;

    fn sample_screen() -> crate::App {
        let mut app = crate::App::default();
        app.select(Screen::Log);
        app.log_mut().show_sample();
        app
    }

    /// An empty screen: the console sits at the top of the page, so the panel's
    /// own text is what a reader sees first.
    fn empty_screen(mode: OpsMode) -> crate::App {
        let mut app = crate::App::default();
        app.select(Screen::Log);
        app.log_mut().ops.mode = mode;
        app
    }

    fn texts_for(mode: OpsMode) -> Vec<String> {
        let mut app = empty_screen(mode);
        // The console is a drawer tab (DESIGN.md §5), so the frame under test is
        // the one with that tab open; the closed drawer draws no panel at all.
        let log = app.log_mut();
        log.open_drawer(super::super::LogTab::Ops);
        screen_texts(log)
    }

    fn status_of(ops: &LogOps) -> Option<(OpsMode, Status, &str)> {
        ops.status
            .as_ref()
            .map(|entry| (entry.mode, entry.status, entry.message.as_str()))
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

    /// The one action button a panel is built around.
    fn action_label(mode: OpsMode) -> &'static str {
        match mode {
            OpsMode::Download => "吸い出す",
            OpsMode::Erase => "バックアップを保存",
            OpsMode::Project => "読込んで書き出す",
            OpsMode::Repair => "修復",
            OpsMode::Proxy => "開始",
            OpsMode::Read => "読み込んで書き出す",
        }
    }

    #[test]
    fn every_operation_panel_renders_with_text() {
        let screenshot_dir = std::env::var_os("M241_GUI_OPS_SCREENSHOTS");
        for mode in OpsMode::ALL {
            let mut app = sample_screen();
            app.log_mut().ops.mode = mode;
            app.log_mut().open_drawer(super::super::LogTab::Ops);
            let image = crate::screenshot::render(&mut app, Screen::Log, Vec2::new(640.0, 400.0));
            assert!(
                image.contains_colour(theme::INK, 24),
                "{} panel rendered no text",
                mode.label()
            );
            // The action text is what tells the panels apart; an ink-colour count
            // would pass for a panel that drew only its chip row.
            let texts = texts_for(mode);
            assert!(
                texts.iter().any(|text| text == action_label(mode)),
                "{} panel did not draw its action: {texts:?}",
                mode.label()
            );
            // The modes are reviewed from an empty log: that is where the console
            // sits at the top of the page and every field is above the fold. The
            // write is opt-in so `cargo test` leaves no artifacts.
            if let Some(directory) = &screenshot_dir {
                let mut empty = crate::App::default();
                empty.select(Screen::Log);
                empty.log_mut().ops.mode = mode;
                empty.log_mut().open_drawer(super::super::LogTab::Ops);
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
    fn the_transfer_panels_draw_the_controls_their_refusals_name() {
        // The erase panel used to tell the user to tick 上書き and type a port
        // while drawing neither: both controls lived only on the download panel.
        // Each panel owns them now, and the refusal quotes the label it draws.
        let scratch = ScratchDir::new("erase-controls");
        let existing = scratch.join("backup.bin");
        std::fs::write(&existing, b"old").expect("write the existing backup");
        let mut ops = LogOps::default();
        ops.erase.transfer.destination = existing.display().to_string();
        let refusal = ops
            .erase
            .transfer
            .refusal()
            .expect("an existing destination needs the overwrite flag");
        assert!(
            refusal.contains(OVERWRITE_LABEL),
            "the refusal names the checkbox: {refusal}"
        );
        for mode in [OpsMode::Download, OpsMode::Erase] {
            let texts = texts_for(mode);
            assert!(
                texts.iter().any(|text| text == PORT_LABEL),
                "{} panel draws no port row: {texts:?}",
                mode.label()
            );
            assert!(
                texts.iter().any(|text| text == OVERWRITE_LABEL),
                "{} panel draws no overwrite checkbox: {texts:?}",
                mode.label()
            );
        }
    }

    #[test]
    fn a_status_from_another_panel_is_not_drawn() {
        // A repair error under the erase panel reads as an erase error.
        let tab = super::super::LogTab::Ops;
        let mut app = empty_screen(OpsMode::Erase);
        let mut ops = std::mem::take(&mut app.log_mut().ops);
        ops.set_status(OpsMode::Repair, Status::Err, "修復だけのエラー");
        app.log_mut().ops = ops;
        let log = app.log_mut();
        log.open_drawer(tab);
        let erase_texts = screen_texts(log);
        assert!(
            !erase_texts.iter().any(|text| text == "修復だけのエラー"),
            "the erase panel drew another panel's status: {erase_texts:?}"
        );
        let mut app = empty_screen(OpsMode::Repair);
        let mut ops = std::mem::take(&mut app.log_mut().ops);
        ops.set_status(OpsMode::Repair, Status::Err, "修復だけのエラー");
        app.log_mut().ops = ops;
        let log = app.log_mut();
        log.open_drawer(tab);
        let repair_texts = screen_texts(log);
        assert!(
            repair_texts.iter().any(|text| text == "修復だけのエラー"),
            "the repair panel did not draw its own status: {repair_texts:?}"
        );
    }

    #[test]
    fn the_erase_action_needs_a_saved_backup_and_the_confirmation() {
        // The button is armed by two independent things, so a stray click cannot
        // reach the destructive path with only one of them.
        let mut ops = LogOps::default();
        assert!(!ops.erase.ready());
        ops.erase.saved = Some((String::from("backup.bin"), 10));
        assert!(!ops.erase.ready());
        ops.erase.confirm = true;
        assert!(ops.erase.ready());

        // The press is the refusal, and it lands on the erase panel's status
        // (the sentence's own wording is pinned by the refusal's contract).
        ops.press_erase();
        assert_eq!(
            status_of(&ops).map(|(mode, status, _)| (mode, status)),
            Some((OpsMode::Erase, Status::Err))
        );
        let Some((_, _, message)) = status_of(&ops) else {
            panic!("the press must record a refusal");
        };
        assert_eq!(message, ERASE_REFUSAL);

        // Without both gates the press records nothing at all.
        let mut unarmed = LogOps::default();
        unarmed.press_erase();
        assert!(unarmed.status.is_none());
    }

    #[test]
    fn a_kmz_project_is_refused_for_the_same_inputs_the_cli_refuses() {
        let scratch = ScratchDir::new("project-kmz-load");
        let capture = scratch.join("capture.nmea");
        std::fs::write(&capture, "$GPGGA,A*00\r\n").expect("write the capture");
        let project = scratch.join("trip.json");
        let base = serde_json::json!({
            "format": "m241-project",
            "version": 1,
            "input": "nmea",
            "log": capture.to_string_lossy(),
            "photos": null,
            "utc_offset_days": 0,
            "output": scratch.join("track.kmz").to_string_lossy(),
            "export_format": "kmz",
        });
        std::fs::write(&project, base.to_string()).expect("write the project");

        let mut app = sample_screen();
        let mut ops = std::mem::take(&mut app.log_mut().ops);
        ops.project.path = project.display().to_string();
        let screen = app.log_mut();
        ops.load_project(&mut *screen, false);
        let Some((_, status, message)) = status_of(&ops) else {
            panic!("a kmz project over an NMEA capture must be refused");
        };
        assert_eq!(status, Status::Err);
        assert!(message.contains("NMEA"), "got {message}");

        // A dump project with `photos: null` is the CLI's other refusal.
        let dump = scratch.join("track.bin");
        std::fs::write(
            &dump,
            synthetic_dump(&[(1_700_000_000, 35.68, 139.76, 10.0)]),
        )
        .expect("write the dump");
        let document = serde_json::json!({
            "format": "m241-project",
            "version": 1,
            "input": "dump",
            "log": dump.to_string_lossy(),
            "photos": null,
            "utc_offset_days": 0,
            "output": scratch.join("track.kmz").to_string_lossy(),
            "export_format": "kmz",
        });
        std::fs::write(&project, document.to_string()).expect("write the project");
        ops.load_project(&mut *screen, false);
        let Some((_, status, message)) = status_of(&ops) else {
            panic!("a kmz project without photos must be refused");
        };
        assert_eq!(status, Status::Err);
        assert!(message.contains("写真"), "got {message}");
    }

    /// Run a finished transfer through a taken-out console, the way `ui` does.
    fn finish(log: &mut LogScreen, purpose: DownloadPurpose, result: Result<Vec<u8>, String>) {
        let mut ops = std::mem::take(&mut log.ops);
        ops.finish(purpose, result, log);
        log.ops = ops;
    }

    #[test]
    fn a_project_load_writes_the_read_report_fields_it_stored() {
        // `log project load` goes through `log read`, so its JSON output carries
        // the stored `utc_offset_days` and, for a capture, the `nmea` counts.
        // The GUI's project panel must write the same object.
        let scratch = ScratchDir::new("project-load-report");
        let capture = scratch.join("capture.nmea");
        let line = m241_core::protocol::pmtk::sentence(
            "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
        );
        std::fs::write(&capture, format!("$GPGSA,A,3,01,02,03*00\r\n{line}\r\n"))
            .expect("write the capture");
        let output = scratch.join("out.json");
        let project = scratch.join("trip.json");
        let document = serde_json::json!({
            "format": "m241-project",
            "version": 1,
            "input": "nmea",
            "log": capture.to_string_lossy(),
            "photos": null,
            "utc_offset_days": 1,
            "output": output.to_string_lossy(),
            "export_format": "json",
        });
        std::fs::write(&project, document.to_string()).expect("write the project");

        let mut app = sample_screen();
        let mut ops = std::mem::take(&mut app.log_mut().ops);
        ops.project.path = project.display().to_string();
        let screen = app.log_mut();
        ops.load_project(&mut *screen, true);
        assert!(
            ops.status
                .as_ref()
                .is_some_and(|entry| entry.status == Status::Ok),
            "the project load failed: {:?}",
            ops.status
        );
        let written: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&output).expect("open the export"))
                .expect("one JSON object");
        assert_eq!(written["offset_days"], 1);
        assert_eq!(written["nmea"]["imported"], 1);
        assert_eq!(written["nmea"]["ignored"], 1);
        assert_eq!(
            written["position"]["format"], "decimal",
            "a project load writes the CLI's position object: {written}"
        );
    }

    #[test]
    fn an_empty_download_file_is_a_stated_outcome_not_a_saved_file() {
        let scratch = ScratchDir::new("empty-download");
        let destination = scratch.join("empty.bin");
        let mut app = sample_screen();
        let log = app.log_mut();
        log.ops.download.transfer.destination = destination.display().to_string();
        finish(log, DownloadPurpose::Dump, Ok(Vec::new()));
        assert!(
            log.ops.status.as_ref().is_some_and(|entry| {
                entry.status == Status::Info && entry.message.contains("空")
            }),
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
        log.ops.download.transfer.destination = destination.display().to_string();
        finish(log, DownloadPurpose::Dump, Ok(bytes));
        assert_eq!(log.track().points.len(), 1);
        assert_eq!(log.log_path, destination.display().to_string());
        assert!(destination.exists());
        assert!(
            log.ops
                .status
                .as_ref()
                .is_some_and(|entry| entry.status == Status::Ok),
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
                |entry| entry.status == Status::Err && entry.message.contains("no device")
            ),
            "got {:?}",
            log.ops.status
        );
    }

    #[test]
    fn the_export_bar_writes_the_read_report_into_json() {
        // `log read --format json` carries `nmea` and `offset_days`; the export
        // bar after a read must write the same object, not the exporter's bare
        // one.
        let scratch = ScratchDir::new("read-report-export");
        let output = scratch.join("track.json");
        let mut app = sample_screen();
        let log = app.log_mut();
        log.set_export_format(ExportFormat::Json);
        log.set_read_facts(ReadFacts {
            offset_days: 1,
            nmea: Some(NmeaCounts {
                imported: 3,
                dropped: 1,
                ignored: 2,
            }),
            gpx: false,
        });
        log.set_destination(output.display().to_string());
        log.export().expect("export the read's track");

        let text = scratch.join("track.json");
        let document: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&text).expect("open the export"))
                .expect("one JSON object");
        assert_eq!(document["offset_days"], 1);
        assert_eq!(document["nmea"]["imported"], 3);
        assert_eq!(document["nmea"]["dropped"], 1);
        assert_eq!(document["nmea"]["ignored"], 2);
        assert_eq!(
            document["position"]["format"], "decimal",
            "the export bar writes the CLI's position object too: {document}"
        );

        // A track from another source clears the facts: a download's JSON is the
        // exporter's object, with no counts from a previous read.
        let log = app.log_mut();
        log.set_track(
            super::super::sample_track(),
            m241_core::log::ParseReport::default(),
        );
        let other = scratch.join("download.json");
        log.set_destination(other.display().to_string());
        log.export().expect("export the downloaded track");
        let document: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&other).expect("open the export"))
                .expect("one JSON object");
        assert!(document.get("nmea").is_none(), "got {document}");
        assert!(document.get("offset_days").is_none(), "got {document}");
    }
}
