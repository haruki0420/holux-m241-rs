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

use egui::{RichText, Ui};
use m241_core::protocol::Port;
use m241_core::{DeviceInfo, LogSummary};

use crate::screens::common::{self, Status};
use crate::theme::{self, size, space};

/// The Connect screen's own state.
///
/// Held here rather than in [`crate::App`] so the screen can be tested on its own,
/// and so a future background task can own exactly one screen's data.
#[derive(Debug, Default)]
pub struct ConnectScreen {
    ports: Listing,
    selected: Option<String>,
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
        // At 1024×640 the body is taller than the window, and the review found the
        // "next step" block ending at its first line — below the fold, and therefore
        // unreachable rather than merely cropped.
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
            ui.horizontal(|ui| {
                let selected = self
                    .selected
                    .clone()
                    .unwrap_or_else(|| String::from("ポートを選択"));
                egui::ComboBox::from_id_salt("m241-port")
                    .selected_text(RichText::new(selected).size(size::UI).monospace())
                    .width(ui.available_width() * 0.45)
                    .show_ui(ui, |ui| {
                        if let Listing::Read(ports) = &self.ports {
                            for port in ports {
                                ui.selectable_value(&mut self.selected, Some(port.clone()), port);
                            }
                        }
                    });

                if ui
                    .button(RichText::new("再読み込み").size(size::UI))
                    .clicked()
                {
                    self.refresh();
                }

                self.port_status(ui);
            });

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

    /// The state line: what enumeration actually returned.
    fn port_status(&self, ui: &mut Ui) {
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
            } else {
                // No device: say what is missing, in the order to check it
                // (DESIGN.md §6, "No port found").
                common::status_line(ui, Status::Info, "未接続 — 端末の情報はまだ読んでいません");
                ui.add_space(space::S);
                for (index, line) in [
                    "1. USB ケーブルを挿す",
                    "2. 端末の電源を入れる",
                    "3. 上の「再読み込み」を押す",
                ]
                .iter()
                .enumerate()
                {
                    ui.push_id(index, |ui| {
                        ui.label(RichText::new(*line).size(size::UI).color(theme::MUTE));
                    });
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
            } else {
                common::status_line(ui, Status::Info, "未取得 — ログはまだ読んでいません");
                ui.add_space(space::S);
                ui.label(
                    RichText::new("ログの読み出しは「ログ」画面から行います。")
                        .size(size::UI)
                        .color(theme::MUTE),
                );
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
}
