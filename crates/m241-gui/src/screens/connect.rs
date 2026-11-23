//! 接続 — the Connect screen (DESIGN.md §5).
//!
//! What this screen is for is answering one question before anything destructive
//! can happen: *is there a device, and what is it?* The port list is real — it is
//! the one thing on the screen that can be measured without a logger attached, so
//! it is measured rather than mocked. The device and log panels show nothing until
//! something has been read from a unit, because a fabricated figure here would be
//! read as a measurement.
//!
//! Sample values exist for looking at the layout without a device. They are always
//! marked 「見本」 on the same screen (PRODUCT.md: unverified is a first-class state,
//! and invented data is not a state at all).

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use egui::{Align2, FontId, RichText, Sense, Ui, Vec2};
use m241_core::protocol::Port;
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};
use m241_core::protocol::port::Transport;
use m241_core::{DeviceInfo, Error as CoreError, LogSummary};
use m241_flash::da::Confirmation;
use m241_flash::flow::ProbeDaReport;

use crate::device::{self, DoctorReport, DropFile, JobEvent, PortFinding};
use crate::screens::common::{self, Status};
use crate::theme::{self, metric, size, space};

/// Read-only identity and log-usage snapshot returned by probing a port.
///
/// Public because `tests/render.rs` hands one back through `start_probe_with`;
/// nothing outside this crate constructs it otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProbe {
    /// Device identity from `$PMTK605` → `$PMTK705`.
    pub info: DeviceInfo,
    /// Bytes currently used in the device log (`$PMTK182,2,8`).
    pub log_used_bytes: usize,
    /// Whether recording is currently enabled (`$PMTK182,2,1`), when answered.
    pub log_enabled: Option<bool>,
}

/// Structured failure from probing a device: device message first, recovery hint second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError {
    /// Verbatim error from the OS or protocol layer.
    pub message: String,
    /// Actionable recovery hint in Japanese.
    pub hint: String,
}

/// Current state of the non-blocking device probe.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProbeState {
    #[default]
    Idle,
    Probing {
        port: String,
    },
    Connected(DeviceProbe),
    Failed(ProbeError),
}

/// What the last `device doctor` run produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DoctorState {
    /// Not run in this session.
    #[default]
    Idle,
    /// Running in the background.
    Running,
    /// Finished; a partial report is still a report.
    Done(Box<DoctorReport>),
    /// The job itself failed.
    Failed(String),
}

/// What the last `fw probe-da` run produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DaProbeState {
    /// Not run in this session.
    #[default]
    Idle,
    /// The DA is being loaded and read.
    Running,
    /// Finished.
    Done(ProbeDaReport),
    /// Refused or failed; the device's words are kept.
    Failed(String),
}

/// Which device job the Connect screen is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectJob {
    Doctor,
    ProbeDa,
    Restart,
}

impl ConnectJob {
    fn label(self) -> &'static str {
        match self {
            Self::Doctor => "診断",
            Self::ProbeDa => "DA の確認",
            Self::Restart => "再起動",
        }
    }
}

/// The Connect screen's own state.
///
/// Held here rather than in [`crate::App`] so the screen can be tested on its own,
/// and so a background probe owns only this screen's data.
#[derive(Debug)]
pub struct ConnectScreen {
    ports: Listing,
    selected: Option<String>,
    probe: ProbeState,
    probe_rx: Option<mpsc::Receiver<std::result::Result<DeviceProbe, ProbeError>>>,
    sample: bool,
    doctor: DoctorState,
    drop_dir: String,
    extract_to: String,
    da_path: String,
    da_probe: DaProbeState,
    probe_confirmation: String,
    restart_confirmation: String,
    message: Option<(Status, String)>,
    job: Option<(ConnectJob, mpsc::Receiver<JobEvent>)>,
    /// Whether the diagnostics block is expanded.
    diagnostics_open: bool,
}

impl Default for ConnectScreen {
    fn default() -> Self {
        Self {
            ports: Listing::default(),
            selected: None,
            probe: ProbeState::default(),
            probe_rx: None,
            sample: false,
            doctor: DoctorState::default(),
            drop_dir: String::new(),
            extract_to: String::new(),
            da_path: String::from("DA_v3.8.4.bin"),
            da_probe: DaProbeState::default(),
            probe_confirmation: String::new(),
            restart_confirmation: String::new(),
            message: None,
            job: None,
            diagnostics_open: false,
        }
    }
}

/// What the last port enumeration produced.
///
/// Three states, not `Vec` plus a bool: "not read yet", "read and empty" and
/// "could not read" are different sentences to put in front of a person, and the
/// middle one is the one that gets lost when emptiness is the only signal.
#[derive(Debug, Default)]
enum Listing {
    /// Enumeration has not been attempted in this session.
    #[default]
    Unread,
    /// Enumeration succeeded; the list may be empty.
    Read(Vec<String>),
    /// Enumeration itself failed. The device's words are kept verbatim.
    Failed(String),
}

impl ConnectScreen {
    /// The word that unlocks `fw probe-da`.
    ///
    /// Loading the DA and jumping to it is inside AGENTS.md §2.5: the running DA
    /// can erase and program flash. The CLI requires `--yes`; the GUI requires
    /// this typed word instead, and only the user may type it.
    pub const PROBE_CONFIRMATION: &'static str = "PROBE";

    /// The word that unlocks the `0xD9` restart (`fw da-restart`).
    pub const RESTART_CONFIRMATION: &'static str = "RESTART";

    /// Draw the screen body. The rail is drawn by the shell, not here.
    pub fn ui(&mut self, ui: &mut Ui) {
        if self.poll_probe() || self.poll_job() {
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }

        // The port actions are the reason this screen exists, so they are pinned
        // below the scrolling body instead of sitting at the bottom of it: at
        // 1024×640 the old in-panel row could be scrolled away from the panel it
        // acts on (DESIGN.md §5 — the action belongs to the screen, not the list).
        common::footer(ui, "m241-connect-actions", |ui| self.action_footer(ui));

        // At 1024×640 the body is taller than the window, and the "next step" block
        // used to end at its first line — below the fold, and therefore unreachable
        // rather than merely cropped. Scrolling the body keeps it reachable.
        common::scrolling(ui, "m241-connect-body", |ui| {
            // The heading is the screen's name, not the product's: the title bar
            // already says "Holux M-241", and a heading that repeated it made the
            // screen read as a splash page rather than as the 接続 tab.
            common::heading(
                ui,
                "接続",
                "ファームの日付が 19.6 年ずれています。端末をつなぎ、まずファームを吸い出してください。",
            );
            // The panels' captions carry 「見本」 themselves, so this screen does
            // not spend a second band on it: the design's 1280×800 page has to
            // reach the "next step" list without scrolling (DESIGN.md §5).
            // Lazy first enumeration: asking the OS for ports every frame would spin a
            // udev walk at 60 Hz for a list that changes when a cable moves.
            if matches!(&self.ports, Listing::Unread) {
                self.refresh();
            }

            ui.add_space(space::L);
            self.port_panel(ui);

            ui.add_space(space::M);
            self.diagnostics_panel(ui);

            ui.add_space(space::M);
            ui.columns_const::<2, _>(|columns| {
                let [left, right] = columns;
                self.device_panel(left);
                self.log_panel(right);
            });

            ui.add_space(space::M);
            self.next_steps(ui);
        });
    }

    /// Whether a device job (doctor, probe-da, restart) is running.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The doctor's state, for a caller that has just run it.
    #[must_use]
    pub fn doctor_state(&self) -> &DoctorState {
        &self.doctor
    }

    /// The DA probe's state.
    #[must_use]
    pub fn da_probe_state(&self) -> &DaProbeState {
        &self.da_probe
    }

    /// Put the doctor panel in a specific state, for the screenshot run.
    pub fn set_doctor_state(&mut self, state: DoctorState) {
        self.doctor = state;
    }

    /// Put the DA-probe panel in a specific state, for the screenshot run.
    pub fn set_da_probe_state(&mut self, state: DaProbeState) {
        self.da_probe = state;
    }

    /// Expand or collapse the diagnostics block.
    pub fn set_diagnostics_open(&mut self, open: bool) {
        self.diagnostics_open = open;
    }

    /// Start `device doctor` for the selected port and the typed drop directory.
    ///
    /// # Errors
    ///
    /// A job already running.
    pub fn start_doctor(&mut self) -> Result<(), String> {
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        let job = device::Job::Doctor {
            port: self.selected.clone(),
            drop_dir: self.drop_dir.trim().to_owned(),
            extract_to: self.extract_to.trim().to_owned(),
        };
        self.doctor = DoctorState::Running;
        self.message = Some((Status::Info, String::from("診断しています…")));
        self.start_job(ConnectJob::Doctor, job);
        Ok(())
    }

    /// Start `fw probe-da`. The typed word is checked here, not only by the button.
    ///
    /// # Errors
    ///
    /// The word missing, no port selected, a missing DA path, or a job running.
    pub fn start_probe_da(&mut self) -> Result<(), String> {
        if self.probe_confirmation.trim() != Self::PROBE_CONFIRMATION {
            return Err(format!(
                "DA を確認するには {} と入力してください",
                Self::PROBE_CONFIRMATION
            ));
        }
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        let Some(port) = self.selected.clone() else {
            return Err(String::from("ポートを選択してください"));
        };
        if self.da_path.trim().is_empty() {
            return Err(String::from("DA のファイルを指定してください"));
        }
        let job = device::Job::ProbeDa {
            port,
            da: self.da_path.trim().to_owned(),
            confirmation: Confirmation::granted(),
            assume_brom: false,
        };
        self.da_probe = DaProbeState::Running;
        self.message = Some((Status::Info, String::from("DA を RAM に読み込んでいます…")));
        self.start_job(ConnectJob::ProbeDa, job);
        Ok(())
    }

    /// Send the one-byte `0xD9` restart. The typed word is checked here too.
    ///
    /// # Errors
    ///
    /// The word missing, no port selected, or a job running.
    pub fn start_restart(&mut self) -> Result<(), String> {
        if self.restart_confirmation.trim() != Self::RESTART_CONFIRMATION {
            return Err(format!(
                "再起動するには {} と入力してください",
                Self::RESTART_CONFIRMATION
            ));
        }
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        let Some(port) = self.selected.clone() else {
            return Err(String::from("ポートを選択してください"));
        };
        self.message = Some((Status::Info, String::from("0xD9 を送っています…")));
        self.start_job(ConnectJob::Restart, device::Job::Restart { port });
        Ok(())
    }

    /// Spawn a job and keep its channel.
    fn start_job(&mut self, kind: ConnectJob, job: device::Job) {
        self.job = Some((kind, device::start(job)));
    }

    /// Poll the job channel. Returns `true` while a job is still running.
    pub fn poll_job(&mut self) -> bool {
        let Some(kind) = self.job.as_ref().map(|(kind, _)| *kind) else {
            return false;
        };
        let (events, disconnected) = match self.job.as_ref() {
            Some((_, rx)) => device::poll_channel(rx),
            None => return false,
        };
        let mut finished = false;
        for event in events {
            if self.apply_job_event(kind, event) {
                finished = true;
                break;
            }
        }
        if !finished && disconnected {
            // The worker sends its own failure before returning; a disconnect
            // without one is a thread that died, and the unit's state is unknown.
            self.fail(
                kind,
                String::from(
                    "処理が結果を返さずに終了しました。端末の状態は不明です。\
                     吸い出しで確認するか、復旧の手順に進んでください",
                ),
            );
            finished = true;
        }
        if finished {
            self.job = None;
        }
        self.job.is_some()
    }

    /// Apply one job event. Returns `true` when the job ended.
    fn apply_job_event(&mut self, kind: ConnectJob, event: JobEvent) -> bool {
        match event {
            JobEvent::DoctorDone(report) => {
                self.doctor = DoctorState::Done(Box::new(report));
                self.message = Some((Status::Ok, String::from("診断が終わりました")));
                true
            }
            JobEvent::ProbeDone(report) => {
                let settled = report.settled.map_or_else(
                    || String::from("判定できません（候補が 0 件または複数一致）"),
                    |address| format!("0x{address:08X} にイメージがあります"),
                );
                self.da_probe = DaProbeState::Done(report);
                self.message = Some((
                    Status::Ok,
                    format!("DA の確認: {settled}。フラッシュは書いていません"),
                ));
                true
            }
            JobEvent::Restarted => {
                self.message = Some((
                    Status::Info,
                    String::from(
                        "0xD9 を送りました。応答はないため、再起動を確認するには「接続して確認」か吸い出しを実行してください",
                    ),
                ));
                true
            }
            JobEvent::Failed(message) => {
                self.fail(kind, message);
                true
            }
            JobEvent::Flow(_)
            | JobEvent::Dumped(_)
            | JobEvent::DumpFirst { .. }
            | JobEvent::DumpMismatch { .. }
            | JobEvent::WriteDone(_)
            | JobEvent::RecoverDone(_)
            | JobEvent::BootRomAnswered => true,
        }
    }

    /// Record a failed job in the state the job belongs to.
    fn fail(&mut self, kind: ConnectJob, message: String) {
        match kind {
            ConnectJob::Doctor => self.doctor = DoctorState::Failed(message.clone()),
            ConnectJob::ProbeDa => self.da_probe = DaProbeState::Failed(message.clone()),
            ConnectJob::Restart => {}
        }
        self.message = Some((Status::Err, format!("{}: {message}", kind.label())));
    }

    /// Current probe state, for callers and tests.
    #[must_use]
    pub fn probe_state(&self) -> &ProbeState {
        &self.probe
    }

    /// Poll the background probe channel, returning `true` while a probe is still running.
    pub fn poll_probe(&mut self) -> bool {
        let Some(rx) = self.probe_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(result)) => {
                self.probe = ProbeState::Connected(result);
                self.probe_rx = None;
                self.sample = false;
                false
            }
            Ok(Err(err)) => {
                self.probe = ProbeState::Failed(err);
                self.probe_rx = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.probe = ProbeState::Failed(ProbeError {
                    message: String::from("確認処理が結果を返さずに終了しました"),
                    hint: String::from(
                        "USB ケーブルの接続を確認し、再度「接続して確認」を押してください。",
                    ),
                });
                self.probe_rx = None;
                false
            }
        }
    }

    /// Start a non-blocking read-only probe of the selected serial port.
    pub fn start_probe(&mut self, port: String) {
        if matches!(self.probe, ProbeState::Probing { .. }) {
            // Re-entering would drop the receiver of the probe already running and
            // start a second thread: whichever finished last would win, and its port
            // may not be the one on screen. The button is disabled while probing, but
            // the guard belongs to the function, not to the button.
            return;
        }
        self.start_probe_with(port, probe_serial_port);
    }

    /// Start a non-blocking probe using a supplied worker closure (used by tests).
    pub fn start_probe_with<F>(&mut self, port: String, task: F)
    where
        F: FnOnce(&str) -> std::result::Result<DeviceProbe, ProbeError> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        self.probe = ProbeState::Probing { port: port.clone() };
        self.probe_rx = Some(rx);
        self.sample = false;
        std::thread::spawn(move || {
            let result = task(&port);
            // Nobody is listening when `start_probe` refused a re-entry after this
            // one; the send failing is that case, not an error worth reporting.
            let _ = tx.send(result);
        });
    }

    /// Fill the panels with the sample unit's figures.
    ///
    /// Public because the headless render tests need every screen state to be
    /// reachable without a pointer, and because a future `--demo` flag has to start
    /// somewhere. The screen still draws the 「見本」 marker, so the values are not
    /// presented as a measurement just because code set them.
    pub fn show_sample(&mut self) {
        self.sample = true;
    }

    /// Re-enumerate the serial ports.
    fn refresh(&mut self) {
        self.ports = match Port::list() {
            Ok(ports) => {
                // Keep a selection only if that port still exists: a stale
                // selection would let the next screen address a device that is
                // no longer plugged in.
                if let Some(selected) = &self.selected
                    && !ports.iter().any(|port| port == selected)
                {
                    self.selected = None;
                }
                if self.selected.is_none() {
                    self.selected = ports.first().cloned();
                }
                Listing::Read(ports)
            }
            Err(err) => Listing::Failed(err.to_string()),
        };
    }

    fn port_panel(&mut self, ui: &mut Ui) {
        common::panel(ui, "ポート", |ui| {
            ui.horizontal_wrapped(|ui| {
                let selected = self
                    .selected
                    .clone()
                    .unwrap_or_else(|| String::from("ポートを選択"));
                egui::ComboBox::from_id_salt("m241-port")
                    .selected_text(RichText::new(selected).size(size::UI).monospace())
                    .width((ui.available_width() * 0.45).max(metric::RAIL_BLOCK))
                    .show_ui(ui, |ui| {
                        if let Listing::Read(ports) = &self.ports {
                            for port in ports {
                                ui.selectable_value(&mut self.selected, Some(port.clone()), port);
                            }
                        }
                    });
                // The device's link settings, so the panel states the protocol it
                // expects instead of leaving the row half empty (DESIGN.md §5).
                ui.label(
                    RichText::new("38400 bps・8N1")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });

            // Guidance appears only when there is nothing to pick. An empty list
            // and a failed enumeration are different failures and get different
            // sentences: an empty list is the cable or the power, while a failed
            // enumeration is the OS (driver, permission) and re-plugging the
            // cable cannot fix it. Sharing one sentence made the panel contradict
            // the footer, which states the reason.
            if let Some((message, hint)) = self.port_guidance() {
                ui.add_space(space::S);
                ui.label(RichText::new(message).size(size::UI).color(theme::MUTE));
                ui.add_space(space::S);
                ui.label(RichText::new(hint).size(size::CAPTION).color(theme::MUTE));
            }
        });
    }

    /// What to say under the port selector when there is nothing to pick.
    ///
    /// Returns one line about the failure and one diagnostic line. The failed case
    /// repeats the reason the footer's status line shows, because the two lines sit
    /// on the same screen and "no port found" beside "could not list ports: …"
    /// reads as two contradictory problems.
    fn port_guidance(&self) -> Option<(String, &'static str)> {
        match &self.ports {
            Listing::Read(ports) if ports.is_empty() => Some((
                String::from(
                    "シリアルポートが見つかりません。USB ケーブルの接続と端末の電源を確認し、「再読込」を押してください。",
                ),
                "診断: CP210x / PL2303 / FTDI / CH340 のいずれかとして見えていれば接続できます",
            )),
            Listing::Failed(reason) => Some((
                format!("ポートを列挙できません: {reason}"),
                "診断: ドライバの導入状況と、ポートへのアクセス権を確認してください。ケーブルの抜き差しでは直りません。",
            )),
            _ => None,
        }
    }

    /// The pinned action row: the probe state on the left, the two port actions
    /// on the right, in the design's secondary-then-primary order.
    fn action_footer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            self.port_status(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let probing = matches!(self.probe, ProbeState::Probing { .. });
                let can_probe = self.selected.is_some() && !probing;
                if common::primary_button(ui, "接続して確認", can_probe).clicked()
                    && let Some(port) = self.selected.clone()
                {
                    self.start_probe(port);
                }
                // Reloading during a probe would re-enumerate the list under a
                // running probe: the probe keeps its own port, but the selection
                // and the list can disagree until it finishes. The wait is a
                // fraction of a second, so the button is disabled rather than
                // allowed to race the list it is not part of.
                let can_refresh = !probing;
                if common::secondary_button(ui, "再読込", can_refresh).clicked() {
                    self.refresh();
                }
            });
        });
    }

    /// The state line: what enumeration or probe actually returned.
    fn port_status(&self, ui: &mut Ui) {
        if let ProbeState::Probing { port } = &self.probe {
            common::status_line(ui, Status::Info, &format!("{port} を確認中…"));
            return;
        }
        match &self.ports {
            Listing::Unread => common::status_line(ui, Status::Info, "ポートを調べています"),
            Listing::Failed(reason) => {
                common::status_line(
                    ui,
                    Status::Err,
                    &format!("ポートを列挙できません: {reason}"),
                );
            }
            Listing::Read(ports) if ports.is_empty() => {
                common::status_line(ui, Status::Err, "ポートが見つかりません");
            }
            Listing::Read(ports) => {
                let count = ports.len();
                common::status_line(ui, Status::Info, &format!("{count} 件のポート"));
            }
        }
    }

    fn device_panel(&mut self, ui: &mut Ui) {
        let sample = self.sample;
        common::sample_panel(ui, "デバイス", sample, |ui| {
            if sample {
                let info = sample_device();
                // One figure per row with the value on the panel's right edge,
                // matching `log_panel` beside it (DESIGN.md §2, §5: right-aligned
                // figures).
                for (label, value) in [
                    ("ファーム", info.release),
                    ("ビルド", info.build),
                    ("モデル", info.model),
                    ("フラッシュ", String::from("512 KiB")),
                    ("日付", format!("{} 日ずれ", 7168)),
                ] {
                    common::figure_row(ui, label, &value);
                }
                return;
            }

            match &self.probe {
                ProbeState::Connected(probe) => {
                    common::status_line(ui, Status::Ok, "接続済み（読み取り専用）");
                    ui.add_space(space::S);
                    common::figure_row(ui, "ファーム", &probe.info.release);
                    common::figure_row(ui, "ビルド", &probe.info.build);
                    common::figure_row(ui, "モデル", &probe.info.model);
                    if let Some(sdk) = &probe.info.sdk {
                        common::figure_row(ui, "SDK", sdk);
                    }
                    let release_note = if probe.info.is_known_release() {
                        "既知リリース（日付修正対象）"
                    } else {
                        "未知のリリース（吸い出して確認）"
                    };
                    common::figure_row(ui, "状態", release_note);
                }
                ProbeState::Probing { port } => {
                    common::status_line(ui, Status::Info, &format!("{port} から端末情報を取得中…"));
                }
                ProbeState::Failed(err) => {
                    // DESIGN.md §6: the device's own words first, the hint second.
                    common::status_line(ui, Status::Err, &err.message);
                    ui.add_space(space::S);
                    ui.label(RichText::new(&err.hint).size(size::UI).color(theme::MUTE));
                }
                ProbeState::Idle => {
                    // No device: say what is missing, in the order to check it
                    // (DESIGN.md §6, "No port found").
                    common::status_line(
                        ui,
                        Status::Info,
                        "未接続（端末の情報はまだ読んでいません）",
                    );
                    ui.add_space(space::S);
                    for (index, line) in [
                        "1. USB ケーブルを挿す",
                        "2. 端末の電源を入れる",
                        "3. 上の「再読込」を押し、「接続して確認」を実行する",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.push_id(index, |ui| {
                            ui.label(RichText::new(*line).size(size::UI).color(theme::MUTE));
                        });
                    }
                }
            }
        });
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        let sample = self.sample;
        common::sample_panel(ui, "ログ", sample, |ui| {
            if sample {
                let summary = sample_summary();
                // One figure per row with the value on the panel's right edge,
                // matching `device_panel` beside it so both side-by-side panels
                // share the same label-left / value-right alignment (DESIGN.md §2, §5).
                for (label, value) in [
                    ("使用量", common::bytes(212_860)),
                    ("点数", summary.records.to_string()),
                    ("位置", summary.fixes.to_string()),
                    ("期間", String::from("11/14 → 11/15")),
                    ("記録", String::from("有効")),
                ] {
                    common::figure_row(ui, label, &value);
                }
                return;
            }

            match &self.probe {
                ProbeState::Connected(probe) => {
                    common::status_line(ui, Status::Ok, "ログ使用量を取得しました");
                    ui.add_space(space::S);
                    common::figure_row(ui, "使用量", &common::bytes(probe.log_used_bytes));
                    let enabled_label = match probe.log_enabled {
                        Some(true) => "有効",
                        Some(false) => "停止中",
                        None => "未取得",
                    };
                    common::figure_row(ui, "記録", enabled_label);
                }
                ProbeState::Probing { .. } => {
                    common::status_line(ui, Status::Info, "ログ使用量を確認中…");
                }
                ProbeState::Failed(_) => {
                    common::status_line(ui, Status::Info, "未取得（接続確認に失敗しました）");
                }
                ProbeState::Idle => {
                    common::status_line(ui, Status::Info, "未取得（ログはまだ読んでいません）");
                    ui.add_space(space::S);
                    ui.label(
                        RichText::new(
                            "端末をつないで「接続して確認」を押すとログ使用量を表示します。",
                        )
                        .size(size::UI)
                        .color(theme::MUTE),
                    );
                }
            }
        });
    }

    /// The diagnostics block: `device doctor`, `fw probe-da`, `fw da-restart`.
    ///
    /// The CLI's three device-side commands, without a terminal. Every control
    /// says which command it is the same as, so a reader who has only the manual
    /// can still tell what will happen. Kept as one function because the three
    /// sections share one `message` line and one busy flag; splitting them would
    /// pass the same `&mut self` to three pieces of one panel.
    fn diagnostics_panel(&mut self, ui: &mut Ui) {
        // Collapsed by default: the port list and the device panels are the
        // screen's first answer, and an always-open block pushed them below the
        // fold at 1280x800. The actions are one click away and the row says
        // which CLI commands the block holds.
        let title = "診断・保守（device doctor / fw probe-da / fw da-restart）";
        if !self.diagnostics_open {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(size::CAPTION).color(theme::MUTE));
                if common::secondary_button(ui, "開く", true).clicked() {
                    self.diagnostics_open = true;
                }
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(size::CAPTION).color(theme::MUTE));
            if common::secondary_button(ui, "閉じる", true).clicked() {
                self.diagnostics_open = false;
            }
        });
        ui.add_space(space::S);
        let doctor = self.doctor.clone();
        let da_probe = self.da_probe.clone();
        common::panel(ui, "診断・保守", |ui| {
            ui.label(
                RichText::new("ポートの一覧・端末の識別・ベンダーファイルの確認を行います。")
                    .size(size::UI)
                    .color(theme::MUTE),
            );
            ui.add_space(space::M);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("配布物の場所")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.drop_dir)
                        .hint_text("ベンダー配布物のディレクトリ（任意）")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() * 0.4),
                );
                ui.label(
                    RichText::new("DA の抽出先")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.extract_to)
                        .hint_text("空なら抽出しない")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() * 0.3),
                );
            });
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                if common::secondary_button(ui, "診断する", !self.busy()).clicked()
                    && let Err(reason) = self.start_doctor()
                {
                    self.message = Some((Status::Err, reason));
                }
                ui.label(
                    RichText::new("CLI の m241 device doctor と同じ処理です")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });

            match &doctor {
                DoctorState::Idle => {
                    common::status_line(ui, Status::Info, "まだ診断していません");
                }
                DoctorState::Running => {
                    common::status_line(ui, Status::Info, "診断しています…");
                }
                DoctorState::Failed(reason) => {
                    common::status_line(ui, Status::Err, reason);
                }
                DoctorState::Done(report) => {
                    doctor_report(ui, report);
                }
            }

            ui.add_space(space::M);
            ui.label(
                RichText::new("DA の確認（fw probe-da）")
                    .size(size::TITLE)
                    .color(theme::INK),
            );
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                ui.label(RichText::new("DA").size(size::CAPTION).color(theme::MUTE));
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.da_path)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - metric::ACTION_WIDTH - space::L),
                );
            });
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("確認のため {} と入力", Self::PROBE_CONFIRMATION))
                        .size(size::UI)
                        .color(theme::INK),
                );
                let _response = ui.add_sized(
                    Vec2::new(metric::ACTION_WIDTH * 1.5, metric::ROW),
                    egui::TextEdit::singleline(&mut self.probe_confirmation)
                        .hint_text(Self::PROBE_CONFIRMATION)
                        .font(egui::TextStyle::Monospace),
                );
                let ready = self.probe_confirmation.trim() == Self::PROBE_CONFIRMATION
                    && self.selected.is_some()
                    && !self.busy();
                if common::secondary_button(ui, "DA を確認", ready).clicked()
                    && let Err(reason) = self.start_probe_da()
                {
                    self.message = Some((Status::Err, reason));
                }
            });
            ui.label(
                RichText::new(
                    "CLI の m241 fw probe-da --yes と同じ処理です。DA を RAM に載せるだけで、\
                     フラッシュは書き換えません。",
                )
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
            match &da_probe {
                DaProbeState::Idle => {}
                DaProbeState::Running => {
                    common::status_line(ui, Status::Info, "DA を読み込んでいます…");
                }
                DaProbeState::Failed(reason) => {
                    common::status_line(ui, Status::Err, reason);
                }
                DaProbeState::Done(report) => da_probe_report(ui, report),
            }

            ui.add_space(space::M);
            ui.label(
                RichText::new("0xD9 再起動（fw da-restart）")
                    .size(size::TITLE)
                    .color(theme::INK),
            );
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                let _response = ui.add_sized(
                    Vec2::new(metric::ACTION_WIDTH * 1.5, metric::ROW),
                    egui::TextEdit::singleline(&mut self.restart_confirmation)
                        .hint_text(Self::RESTART_CONFIRMATION)
                        .font(egui::TextStyle::Monospace),
                );
                let ready = self.restart_confirmation.trim() == Self::RESTART_CONFIRMATION
                    && self.selected.is_some()
                    && !self.busy();
                if common::secondary_button(ui, "0xD9 を送る", ready).clicked()
                    && let Err(reason) = self.start_restart()
                {
                    self.message = Some((Status::Err, reason));
                }
                ui.label(
                    RichText::new("CLI の m241 fw da-restart と同じ処理です（応答はありません）")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });

            if let Some((status, text)) = &self.message {
                ui.add_space(space::S);
                common::status_line(ui, *status, text);
            }
        });
    }

    fn next_steps(&mut self, ui: &mut Ui) {
        ui.add_space(space::M);
        ui.label(
            RichText::new("次にすること")
                .size(size::TITLE)
                .color(theme::INK),
        );
        ui.add_space(space::S);

        // A ruled list, not cards: the manual's numbered next steps (DESIGN.md §5,
        // density rule). Row height 32, a hairline between rows, the arrow at the
        // right edge so the list has a direction.
        let width = ui.available_width();
        for (label, note, unverified) in [
            ("ファームを吸い出す", "読み取りのみ・安全", false),
            ("日付定数を書き換える", "PC 内で完結", false),
            ("書き戻す", "本体を書き換える", true),
        ] {
            let (rect, _response) =
                ui.allocate_exact_size(Vec2::new(width, metric::ROW), Sense::hover());
            let painter = ui.painter_at(rect);
            let label_rect = painter.text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(size::UI),
                theme::INK,
            );
            if unverified {
                // DESIGN.md §6: an unverified feature wears the word, in mute,
                // never amber. Amber is for danger, not for doubt.
                painter.text(
                    label_rect.right_center() + Vec2::new(space::S, 0.0),
                    Align2::LEFT_CENTER,
                    "未検証",
                    FontId::proportional(size::CAPTION),
                    theme::MUTE,
                );
            }
            // No `→` after the note. DESIGN.md §5's mock drew an arrow as a
            // direction mark, but on screen it reads as "this row goes somewhere"
            // while the row is not clickable or focusable (design review,
            // Should 3). The note stays at the right edge; §10 records the
            // departure from the figure.
            painter.text(
                rect.right_center(),
                Align2::RIGHT_CENTER,
                note,
                FontId::proportional(size::CAPTION),
                theme::MUTE,
            );
            painter.hline(
                rect.x_range(),
                rect.bottom(),
                egui::Stroke::new(metric::HAIRLINE, theme::LINE),
            );
        }

        ui.add_space(space::M);
        ui.checkbox(
            &mut self.sample,
            RichText::new("見本の値を表示（実機から読んだ値ではありません）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
    }
}

/// Open `port_name` at 38400 bps and perform a read-only device & log-usage probe.
fn probe_serial_port(port_name: &str) -> std::result::Result<DeviceProbe, ProbeError> {
    let port = Port::open(port_name, NMEA_BAUD).map_err(probe_error_for)?;
    let mut client = PmtkClient::new(port);
    probe_client(&mut client)
}

/// Query `$PMTK605` (`request_identity`) and `$PMTK182,2,8` (`log_used_bytes`)
/// through an open [`PmtkClient`], without issuing any write or erase commands.
///
/// Read-only is the whole contract of this function (AGENTS.md §2.1): the probe
/// runs again on every click of 「接続して確認」, so a `$PMTK182,5` (stop recording)
/// or `$PMTK182,6` (erase) added here would silently change the device on a
/// command that only claims to look. `probe_client_sends_only_read_requests`
/// pins the three sentences that may go out.
fn probe_client<T: Transport>(
    client: &mut PmtkClient<T>,
) -> std::result::Result<DeviceProbe, ProbeError> {
    let info = client.request_identity().map_err(probe_error_for)?;
    let log_used_bytes = client.log_used_bytes().map_err(probe_error_for)?;
    let log_enabled = client.log_enabled().ok();
    Ok(DeviceProbe {
        info,
        log_used_bytes,
        log_enabled,
    })
}

/// Format a [`CoreError`] into the device/OS message first and an actionable hint second.
///
/// Matched without a wildcard arm: a hint is the sentence a person reads when the
/// connection failed, and a new [`CoreError`] variant that falls into a catch-all
/// produces a vague one that still compiles. Listing every variant makes
/// `m241-core` adding one a compile error here.
#[must_use]
fn probe_error_for(err: CoreError) -> ProbeError {
    let message = err.to_string();
    let hint = match err {
        CoreError::Port { .. } => String::from(
            "USB ケーブルの接続とシリアルポートへのアクセス権（Linux では dialout / uucp グループ等）を確認し、「再読込」後にやり直してください。",
        ),
        CoreError::Timeout { .. } => String::from(
            "端末の電源が入っているか、他のアプリがポートを開いていないか確認してください。起動直後は数秒待ってから再試行してください。",
        ),
        CoreError::Nmea { .. }
        | CoreError::Framing { .. }
        | CoreError::Rejected { .. }
        | CoreError::Log { .. } => String::from(
            "38400 bps の通信が不安定です。USB ケーブルの接触を確認し、端末の電源を入れ直してから再試行してください。",
        ),
        CoreError::Safety { .. }
        | CoreError::State(_)
        | CoreError::Profile { .. }
        | CoreError::Firmware { .. }
        | CoreError::Io { .. } => String::from(
            "接続と端末の状態を確認してから、もう一度「接続して確認」を押してください。",
        ),
    };
    ProbeError { message, hint }
}

/// Render a `device doctor` report: ports, identity, and the drop scan.
fn doctor_report(ui: &mut Ui, report: &DoctorReport) {
    common::status_line(ui, Status::Ok, "診断が終わりました");
    ui.add_space(space::S);
    common::figure_row(
        ui,
        "ポート",
        &format!(
            "{} 件（候補 {} 件）",
            report.ports.len(),
            report.ports.iter().filter(|port| port.likely).count()
        ),
    );
    for PortFinding { name, likely } in &report.ports {
        common::figure_row(
            ui,
            " ",
            &format!("{name}  {}", if *likely { "候補" } else { "—" }),
        );
    }
    if let Some(reason) = &report.ports_error {
        common::status_line(ui, Status::Err, reason);
    }
    match (&report.identity, &report.probe_error) {
        (Some(identity), _) => {
            common::figure_row(ui, "ファーム", &identity.release);
            common::figure_row(ui, "ビルド", &identity.build);
            common::figure_row(ui, "モデル", &identity.model);
        }
        (None, Some(reason)) => {
            common::status_line(ui, Status::Err, &format!("端末の識別に失敗: {reason}"));
        }
        (None, None) => {}
    }
    if !report.drop_dir.is_empty() {
        common::figure_row(ui, "配布物", &format!("{} 件", report.files.len()));
        for DropFile { path, size, kind } in &report.files {
            common::figure_row(
                ui,
                " ",
                &format!(
                    "{}  {}  {}",
                    kind.label(),
                    common::bytes(*size),
                    file_name(path)
                ),
            );
        }
        if let Some(source) = &report.da_source {
            common::figure_row(ui, "DA", &file_name(source));
        }
        if let Some(dest) = &report.extracted_to {
            common::status_line(ui, Status::Ok, &format!("DA を {dest} に抽出しました"));
        }
    }
    if let Some(reason) = &report.drop_error {
        common::status_line(ui, Status::Err, reason);
    }
}

/// Render a `fw probe-da` report: the DA's report and both candidate pages.
fn da_probe_report(ui: &mut Ui, report: &ProbeDaReport) {
    common::status_line(
        ui,
        Status::Ok,
        "DA が応答しました（フラッシュ書き込みなし）",
    );
    ui.add_space(space::S);
    common::figure_row(
        ui,
        "DA",
        &format!(
            "v{}.{} region 0x{:02X}  {:04X}:{:04X}  {} バイト",
            report.id.version >> 8,
            report.id.version & 0xFF,
            report.id.region,
            report.id.manufacturer,
            report.id.device,
            report.id.flash_bytes
        ),
    );
    for page in &report.pages {
        let value = match (&page.first_bytes, &page.error) {
            (Some(first), _) => format!(
                "0x{:08X}  {}  {}",
                page.address,
                hex_bytes(first),
                if page.matches_vector {
                    "ベクタ一致"
                } else {
                    "不一致"
                }
            ),
            (None, Some(reason)) => format!("0x{:08X}  読めません（{reason}）", page.address),
            (None, None) => format!("0x{:08X}  読めません", page.address),
        };
        common::figure_row(ui, "候補", &value);
    }
    let verdict = report.settled.map_or_else(
        || String::from("判定できません（候補が 0 件または複数一致）"),
        |address| format!("0x{address:08X} にイメージがあります"),
    );
    common::status_line(ui, Status::Info, &verdict);
}

/// Upper-case hex, space-separated.
fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last path component, for a figure row.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from(path))
}

/// The device the manual's figure shows, for looking at the layout.
///
/// Values are the ones `docs/dev/05-verification.md` records from the hardware
/// session, so the sample looks like a real unit rather than invented numbers.
fn sample_device() -> DeviceInfo {
    DeviceInfo {
        release: String::from("B-core_1.20"),
        build: String::from("0043"),
        model: String::from("01017-00F"),
        sdk: None,
    }
}

/// The log figures the manual's figure shows. Ten thousand records is the
/// measured download, so the sample's density matches a real trip.
fn sample_summary() -> LogSummary {
    LogSummary {
        records: 10_399,
        fixes: 10_201,
        damaged_block_count: 0,
        started_at: Some(1_763_000_000),
        ended_at: Some(1_763_010_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DropKind;

    fn screen() -> ConnectScreen {
        ConnectScreen::default()
    }

    #[test]
    fn the_screen_starts_without_having_read_anything() {
        // "Not read yet" is a state a person must be told about: a blank port list
        // and a failed read look identical if the screen only holds a `Vec`.
        let screen = screen();
        assert!(matches!(screen.ports, Listing::Unread));
        assert!(screen.selected.is_none());
    }

    #[test]
    fn a_failed_enumeration_keeps_the_device_words() {
        let mut screen = screen();
        screen.ports = Listing::Failed(String::from("permission denied"));
        let Listing::Failed(reason) = &screen.ports else {
            panic!("the fixture did not take");
        };
        assert!(
            reason.contains("permission denied"),
            "the failure was reworded instead of reported: {reason}"
        );
    }

    #[test]
    fn an_empty_port_list_asks_for_the_cable_and_the_power() {
        // An empty list is the "nothing is plugged in" failure: the next step is
        // physical. It must not be confused with a broken enumeration.
        let mut screen = screen();
        screen.ports = Listing::Read(Vec::new());
        let (message, hint) = screen
            .port_guidance()
            .expect("an empty list needs guidance");
        assert!(message.contains("見つかりません"), "got {message}");
        assert!(message.contains("ケーブル"), "got {message}");
        assert!(!message.contains("列挙できません"), "got {message}");
        assert!(hint.contains("CP210x"), "got {hint}");
    }

    #[test]
    fn a_failed_enumeration_reports_the_reason_instead_of_no_port_found() {
        // The Stage 1 review found both failures sharing one sentence, so the
        // panel said "no port found, check the cable" while the footer said "could
        // not list ports: permission denied". The reason is the whole message.
        let mut screen = screen();
        screen.ports = Listing::Failed(String::from("permission denied"));
        let (message, hint) = screen
            .port_guidance()
            .expect("a failed enumeration needs guidance");
        assert!(message.contains("列挙できません"), "got {message}");
        assert!(
            message.contains("permission denied"),
            "the reason was dropped: {message}"
        );
        assert!(
            !message.contains("見つかりません"),
            "a failed enumeration is not an empty list: {message}"
        );
        assert!(hint.contains("ドライバ"), "got {hint}");
    }

    #[test]
    fn a_readable_port_list_needs_no_guidance() {
        let mut screen = screen();
        screen.ports = Listing::Read(vec![String::from("/dev/ttyUSB0")]);
        assert!(screen.port_guidance().is_none());
    }

    #[test]
    fn the_sample_device_is_the_measured_unit() {
        // Guards the sample against drifting into invented hardware: these four
        // strings are the ones the hardware session in docs/dev/05-verification.md
        // recorded.
        let info = sample_device();
        assert_eq!(info.release, "B-core_1.20");
        assert_eq!(info.build, "0043");
        assert_eq!(info.model, "01017-00F");
        assert!(info.is_known_release());
    }

    #[test]
    fn the_sample_summary_does_not_claim_a_damaged_block() {
        // A sample that shows damage would teach the reader to expect damage.
        let summary = sample_summary();
        assert_eq!(summary.damaged_block_count, 0);
        assert!(summary.fixes <= summary.records);
    }

    #[derive(Debug, Default)]
    struct FakePmtkTransport {
        tx_log: Vec<u8>,
        rx: std::collections::VecDeque<u8>,
    }

    impl FakePmtkTransport {
        fn enqueue(&mut self, body: &str) {
            let line = m241_core::protocol::pmtk::line(body);
            self.rx.extend(line.as_bytes());
        }
    }

    impl Transport for FakePmtkTransport {
        fn read(&mut self, buf: &mut [u8]) -> m241_core::Result<usize> {
            let mut n = 0;
            for slot in buf.iter_mut() {
                if let Some(byte) = self.rx.pop_front() {
                    *slot = byte;
                    n += 1;
                } else {
                    break;
                }
            }
            Ok(n)
        }

        fn write_all(&mut self, buf: &[u8]) -> m241_core::Result<()> {
            self.tx_log.extend_from_slice(buf);
            let text = String::from_utf8_lossy(buf);
            if text.contains("PMTK605") {
                self.enqueue("PMTK705,B-core_1.20,0043,01017-00F,1.0");
            } else if text.contains("PMTK182,2,8") {
                self.enqueue("PMTK182,3,8,00033F7C");
            } else if text.contains("PMTK182,2,1") {
                self.enqueue("PMTK182,3,1,1");
            }
            Ok(())
        }

        fn flush(&mut self) -> m241_core::Result<()> {
            Ok(())
        }

        fn clear_input(&mut self) -> m241_core::Result<()> {
            self.rx.clear();
            Ok(())
        }

        fn set_baud(&mut self, _baud: u32) -> m241_core::Result<()> {
            Ok(())
        }

        fn set_control_lines(&mut self, _asserted: bool) -> m241_core::Result<()> {
            panic!("Connect probe must never pulse DTR/RTS");
        }
    }

    #[test]
    fn probe_client_queries_identity_and_log_usage_without_pulsing_control_lines() {
        let mut client = PmtkClient::new(FakePmtkTransport::default())
            .with_reply_timeout(Duration::from_millis(200));
        let probe = probe_client(&mut client).expect("fake probe succeeds");
        assert_eq!(probe.info.release, "B-core_1.20");
        assert_eq!(probe.info.build, "0043");
        assert_eq!(probe.info.model, "01017-00F");
        assert_eq!(probe.log_used_bytes, 0x0003_3F7C);
        assert_eq!(probe.log_enabled, Some(true));
    }

    /// Split what the client wrote into the sentence bodies it sent.
    ///
    /// The transport logs every byte in write order, and `pmtk::line` wraps a body
    /// as `$<body>*<checksum>\r\n`, so the bodies are what a test can compare
    /// against the protocol rather than a substring of the whole buffer.
    fn sent_bodies(tx: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(tx);
        text.split('$')
            .skip(1)
            .filter_map(|sentence| sentence.split('*').next())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn probe_client_sends_only_read_requests() {
        // AGENTS.md §2.1 and the branch's own claim: 「接続して確認」 looks, it never
        // writes. Pinning the exact set of sentences — not a `!contains` — is what
        // catches a `log_stop()` ($PMTK182,5), a log erase ($PMTK182,6) or a config
        // write ($PMTK182,0 / $PMTK182,1) being added to the probe later.
        let mut client = PmtkClient::new(FakePmtkTransport::default())
            .with_reply_timeout(Duration::from_millis(200));
        probe_client(&mut client).expect("fake probe succeeds");
        let bodies = sent_bodies(&client.transport_mut().tx_log);
        assert_eq!(
            bodies,
            vec![
                String::from("PMTK605"),
                String::from("PMTK182,2,8"),
                String::from("PMTK182,2,1"),
            ],
            "the read-only probe sent something other than its three read requests"
        );
    }

    #[test]
    fn a_second_probe_is_refused_while_one_is_running() {
        // Two probes at once would race to fill the panels, and the loser's port
        // could be the stale one left on screen. The guard lives in `start_probe`
        // itself, not only in the disabled button.
        let mut screen = screen();
        screen.start_probe_with(String::from("/dev/ttyFake0"), |_port| {
            std::thread::sleep(Duration::from_millis(20));
            Err(ProbeError {
                message: String::from("first"),
                hint: String::new(),
            })
        });
        screen.start_probe(String::from("/dev/ttyFake1"));
        assert_eq!(
            screen.probe_state(),
            &ProbeState::Probing {
                port: String::from("/dev/ttyFake0")
            },
            "the second start replaced the probe that was already running"
        );
    }

    #[test]
    fn non_blocking_background_probe_transitions_to_connected() {
        let mut screen = screen();
        screen.start_probe_with(String::from("/dev/ttyFake0"), |_port| {
            let mut client = PmtkClient::new(FakePmtkTransport::default())
                .with_reply_timeout(Duration::from_millis(200));
            probe_client(&mut client)
        });
        assert!(matches!(screen.probe_state(), ProbeState::Probing { .. }));

        for _ in 0..50 {
            if !screen.poll_probe() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            screen.probe_state(),
            ProbeState::Connected(DeviceProbe {
                log_used_bytes: 0x0003_3F7C,
                ..
            })
        ));
    }

    #[test]
    fn the_da_probe_refuses_without_its_typed_word() {
        // Loading the DA and jumping to it can program flash (AGENTS.md §2.5),
        // so the GUI's equivalent of `--yes` is this typed word and nothing else.
        let mut screen = screen();
        screen.selected = Some(String::from("/dev/ttyFake0"));
        let error = screen
            .start_probe_da()
            .expect_err("probe-da must refuse without PROBE");
        assert!(
            error.contains("PROBE"),
            "the refusal does not ask for PROBE: {error}"
        );
        assert!(!screen.busy(), "a refused probe started a job");
    }

    #[test]
    fn the_restart_refuses_without_its_typed_word() {
        let mut screen = screen();
        screen.selected = Some(String::from("/dev/ttyFake0"));
        let error = screen
            .start_restart()
            .expect_err("0xD9 must refuse without RESTART");
        assert!(
            error.contains("RESTART"),
            "the refusal does not ask for RESTART: {error}"
        );
        assert!(!screen.busy());
    }

    #[test]
    fn a_doctor_report_reaches_the_screen() {
        let mut screen = screen();
        let report = DoctorReport {
            ports: vec![PortFinding {
                name: String::from("/dev/ttyUSB0"),
                likely: true,
            }],
            drop_dir: String::from("vendor"),
            files: vec![DropFile {
                path: String::from("vendor/DA_v3.8.4.bin"),
                size: 14_896,
                kind: DropKind::Da,
            }],
            ..DoctorReport::default()
        };
        assert!(screen.apply_job_event(ConnectJob::Doctor, JobEvent::DoctorDone(report.clone())));
        assert_eq!(screen.doctor_state(), &DoctorState::Done(Box::new(report)));
        assert!(matches!(screen.message, Some((Status::Ok, _))));
    }

    #[test]
    fn a_failed_da_probe_keeps_the_devices_words() {
        let mut screen = screen();
        screen.apply_job_event(
            ConnectJob::ProbeDa,
            JobEvent::Failed(String::from("the DA report is not the vendor DA")),
        );
        let DaProbeState::Failed(reason) = screen.da_probe_state() else {
            panic!("the probe failure did not reach the screen");
        };
        assert!(
            reason.contains("vendor DA"),
            "the device's words were lost: {reason}"
        );
    }

    #[test]
    fn a_second_connect_job_is_refused_while_one_runs() {
        let mut screen = screen();
        let (_tx, rx) = mpsc::channel();
        screen.job = Some((ConnectJob::Doctor, rx));
        assert!(screen.start_doctor().is_err());
        assert!(screen.start_probe_da().is_err());
        assert!(screen.start_restart().is_err());
    }

    #[test]
    fn probe_error_keeps_device_message_first_and_provides_japanese_hint() {
        let err = probe_error_for(CoreError::Timeout {
            what: String::from("$PMTK705 identity reply"),
            timeout: Duration::from_secs(5).into(),
        });
        assert!(err.message.contains("$PMTK705"));
        assert!(err.hint.contains("電源"));
    }
}
