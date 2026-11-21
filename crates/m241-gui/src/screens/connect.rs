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

use std::sync::mpsc;
use std::time::Duration;

use egui::{RichText, Ui, Vec2};
use m241_core::protocol::Port;
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};
use m241_core::protocol::port::Transport;
use m241_core::{DeviceInfo, Error as CoreError, LogSummary};

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

/// The Connect screen's own state.
///
/// Held here rather than in [`crate::App`] so the screen can be tested on its own,
/// and so a background probe owns only this screen's data.
#[derive(Debug, Default)]
pub struct ConnectScreen {
    ports: Listing,
    selected: Option<String>,
    probe: ProbeState,
    probe_rx: Option<mpsc::Receiver<std::result::Result<DeviceProbe, ProbeError>>>,
    sample: bool,
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
    /// Draw the screen body. The rail is drawn by the shell, not here.
    pub fn ui(&mut self, ui: &mut Ui) {
        if self.poll_probe() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }

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
            // The sample band sits under the heading, above every figure: it cannot
            // be scrolled away from the numbers it qualifies, and a crop that keeps
            // the figures keeps the word too.
            if self.sample {
                ui.add_space(space::S);
                common::sample_marker(ui);
            }

            // Lazy first enumeration: asking the OS for ports every frame would spin a
            // udev walk at 60 Hz for a list that changes when a cable moves.
            if matches!(&self.ports, Listing::Unread) {
                self.refresh();
            }

            ui.add_space(space::XL);
            self.port_panel(ui);

            ui.add_space(space::L);
            ui.columns_const::<2, _>(|columns| {
                let [left, right] = columns;
                self.device_panel(left);
                self.log_panel(right);
            });

            ui.add_space(space::L);
            self.next_steps(ui);
        });
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
            let probing = matches!(self.probe, ProbeState::Probing { .. });
            ui.horizontal_wrapped(|ui| {
                let selected = self
                    .selected
                    .clone()
                    .unwrap_or_else(|| String::from("ポートを選択"));
                egui::ComboBox::from_id_salt("m241-port")
                    .selected_text(RichText::new(selected).size(size::UI).monospace())
                    .width((ui.available_width() * 0.35).max(metric::RAIL_BLOCK))
                    .show_ui(ui, |ui| {
                        if let Listing::Read(ports) = &self.ports {
                            for port in ports {
                                ui.selectable_value(&mut self.selected, Some(port.clone()), port);
                            }
                        }
                    });

                if ui
                    .add(
                        egui::Button::new(RichText::new("再読込").size(size::UI))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                {
                    self.refresh();
                }

                let can_probe = self.selected.is_some() && !probing;
                if ui
                    .add_enabled(
                        can_probe,
                        egui::Button::new(RichText::new("接続して確認").size(size::UI))
                            .corner_radius(theme::RADIUS)
                            .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                    )
                    .clicked()
                    && let Some(port) = self.selected.clone()
                {
                    self.start_probe(port);
                }

                self.port_status(ui);
            });

            if matches!(&self.ports, Listing::Read(ports) if ports.is_empty()) {
                ui.add_space(space::S);
                ui.label(
                    RichText::new(
                        "シリアルポートが見つかりません。USB ケーブルの接続と端末の電源を確認し、「再読込」を押してください。",
                    )
                    .size(size::UI)
                    .color(theme::MUTE),
                );
            }

            // Diagnostics, as one caption line rather than a third panel: the
            // question "why is my logger not listed" is answered by a driver name,
            // not by a table.
            ui.add_space(space::S);
            ui.label(
                RichText::new(
                    "診断: CP210x / PL2303 / FTDI / CH340 のいずれかとして見えていれば接続できます",
                )
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
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
                    ("firmware", info.release),
                    ("build", info.build),
                    ("model", info.model),
                    ("flash", String::from("512 KiB")),
                    ("日付", format!("{} 日ずれ", 7168)),
                ] {
                    common::figure_line(ui, label, &value);
                }
                return;
            }

            match &self.probe {
                ProbeState::Connected(probe) => {
                    common::status_line(ui, Status::Ok, "接続済み（読み取り専用）");
                    ui.add_space(space::S);
                    common::figure_line(ui, "firmware", &probe.info.release);
                    common::figure_line(ui, "build", &probe.info.build);
                    common::figure_line(ui, "model", &probe.info.model);
                    if let Some(sdk) = &probe.info.sdk {
                        common::figure_line(ui, "SDK", sdk);
                    }
                    let release_note = if probe.info.is_known_release() {
                        "既知リリース（日付修正対象）"
                    } else {
                        "未知のリリース（吸い出して確認）"
                    };
                    common::figure_line(ui, "状態", release_note);
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
                        "未接続 — 端末の情報はまだ読んでいません",
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
                    ("期間", String::from("11/14 → 11/15")),
                    ("記録", String::from("有効")),
                ] {
                    common::figure_line(ui, label, &value);
                }
                return;
            }

            match &self.probe {
                ProbeState::Connected(probe) => {
                    common::status_line(ui, Status::Ok, "ログ使用量を取得しました");
                    ui.add_space(space::S);
                    common::figure_line(ui, "使用量", &common::bytes(probe.log_used_bytes));
                    let enabled_label = match probe.log_enabled {
                        Some(true) => "有効",
                        Some(false) => "停止中",
                        None => "未取得",
                    };
                    common::figure_line(ui, "記録", enabled_label);
                }
                ProbeState::Probing { .. } => {
                    common::status_line(ui, Status::Info, "ログ使用量を確認中…");
                }
                ProbeState::Failed(_) => {
                    common::status_line(ui, Status::Info, "未取得 — 接続確認に失敗しました");
                }
                ProbeState::Idle => {
                    common::status_line(ui, Status::Info, "未取得 — ログはまだ読んでいません");
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

    fn next_steps(&mut self, ui: &mut Ui) {
        ui.add_space(space::XS);
        ui.label(
            RichText::new("次にすること")
                .size(size::TITLE)
                .color(theme::INK),
        );
        common::hairline(ui);
        ui.add_space(space::S);

        for (label, note, unverified) in [
            ("ファームを吸い出す", "読み取りのみ・安全", false),
            ("日付定数を書き換える", "PC 内で完結", false),
            ("書き戻す", "本体を書き換える", true),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(size::UI).color(theme::INK));
                if unverified {
                    // DESIGN.md §6: an unverified feature wears the word, in mute,
                    // never amber. Amber is for danger, not for doubt.
                    ui.label(
                        RichText::new("未検証")
                            .size(size::CAPTION)
                            .color(theme::MUTE),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(note).size(size::CAPTION).color(theme::MUTE));
                });
            });
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
    fn probe_error_keeps_device_message_first_and_provides_japanese_hint() {
        let err = probe_error_for(CoreError::Timeout {
            what: String::from("$PMTK705 identity reply"),
            timeout: Duration::from_secs(5).into(),
        });
        assert!(err.message.contains("$PMTK705"));
        assert!(err.hint.contains("電源"));
    }
}
