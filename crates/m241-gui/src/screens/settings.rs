//! 設定 — the Settings screen (DESIGN.md §5).
//!
//! Plain rows, no rail block: map cache size and location, units, theme, language,
//! reduced motion. Units and reduced motion are live in-app controls held in
//! [`AppSettings`]; the remaining fixed rows explain honestly why they cannot be
//! changed yet (DESIGN.md §7).

use egui::{RichText, Ui};

use crate::screens::common::{self, Status};
use crate::theme::{self, size, space};

/// Display unit system for speed, distance, and altitude on the Log screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitSystem {
    /// Kilometres, km/h, and metres.
    #[default]
    Metric,
    /// Miles, mph, and feet.
    Imperial,
}

impl UnitSystem {
    /// Label shown in the Settings control.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Metric => "km/h・m",
            Self::Imperial => "mph・ft",
        }
    }

    /// Convert a speed in km/h to the active unit.
    #[must_use]
    pub fn speed_value(self, kmh: f32) -> f32 {
        match self {
            Self::Metric => kmh,
            Self::Imperial => kmh * 0.621_371_2,
        }
    }

    /// Short unit suffix for speed.
    #[must_use]
    pub const fn speed_unit(self) -> &'static str {
        match self {
            Self::Metric => "km/h",
            Self::Imperial => "mph",
        }
    }

    /// Format a speed reading, or `—` when absent.
    #[must_use]
    pub fn format_speed(self, kmh: Option<f32>) -> String {
        kmh.map_or_else(
            || String::from("—"),
            |value| format!("{:.1} {}", self.speed_value(value), self.speed_unit()),
        )
    }

    /// Format a distance in kilometres using the active unit.
    #[must_use]
    pub fn format_distance_km(self, km: f64) -> String {
        match self {
            Self::Metric => format!("{km:.1} km"),
            Self::Imperial => format!("{:.1} mi", km * 0.621_371_192),
        }
    }

    /// Format an altitude in metres using the active unit, or `—` when absent.
    #[must_use]
    pub fn format_altitude(self, metres: Option<f32>) -> String {
        metres.map_or_else(
            || String::from("—"),
            |value| match self {
                Self::Metric => format!("{value:.0} m"),
                Self::Imperial => format!("{:.0} ft", value * 3.280_84),
            },
        )
    }
}

/// Application-wide display and motion preferences held on [`crate::App`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppSettings {
    /// Unit system for speed, distance, and altitude.
    pub units: UnitSystem,
    /// When true, rail tab transitions drop to 0 ms and playback omits rapid steps.
    pub reduced_motion: bool,
}

impl AppSettings {
    /// Default rail tab transition duration in seconds (150 ms).
    pub const DEFAULT_RAIL_TRANSITION_SECS: f32 = 0.15;

    /// Rail tab transition duration in seconds: `0.0` when reduced motion is enabled,
    /// otherwise `0.15` (150 ms).
    #[must_use]
    pub const fn rail_transition_secs(self) -> f32 {
        if self.reduced_motion {
            0.0
        } else {
            Self::DEFAULT_RAIL_TRANSITION_SECS
        }
    }
}

/// One settings row: what it is, its default value, and a short explanation.
#[derive(Clone, Copy)]
struct Row {
    label: &'static str,
    value: &'static str,
    note: &'static str,
}

const ROW_CACHE: Row = Row {
    label: "地図キャッシュ",
    value: "0 B",
    note: "地図タイルをまだ読んでいないため、キャッシュは空です",
};

const ROW_UNITS: Row = Row {
    label: "単位",
    value: "km/h・m",
    note: "ログ画面の速度・距離・標高の表示単位を切り替えます",
};

const ROW_THEME: Row = Row {
    label: "テーマ",
    value: "暗色のみ",
    note: "明色テーマは未設計のため暗色テーマのみ対応",
};

const ROW_LANG: Row = Row {
    label: "言語",
    value: "日本語",
    note: "日本語以外の表示は未検証",
};

const ROW_MOTION: Row = Row {
    label: "視差低減",
    value: "オフ",
    note: "オンにするとタブ切替の移動時間を 0 ms にします",
};

/// The rows DESIGN.md §5 names, in its order (checked in tests).
#[cfg(test)]
const ROWS: [Row; 5] = [ROW_CACHE, ROW_UNITS, ROW_THEME, ROW_LANG, ROW_MOTION];

/// Draw the screen body. The rail is drawn by the shell, not here.
pub fn ui(ui: &mut Ui, settings: &mut AppSettings) {
    common::scrolling(ui, "m241-settings-body", |ui| {
        common::heading(
            ui,
            "設定",
            "単位と視差低減はアプリ内で切り替えられます（終了後は既定値に戻ります）。",
        );
        ui.add_space(space::L);

        common::panel(ui, "表示と単位", |ui| {
            // 1. 地図キャッシュ (fixed)
            common::figure_line(ui, ROW_CACHE.label, ROW_CACHE.value);
            note_label(ui, ROW_CACHE.note);
            ui.add_space(space::S);

            // 2. 単位 (interactive)
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(ROW_UNITS.label)
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for candidate in [UnitSystem::Imperial, UnitSystem::Metric] {
                        let selected = settings.units == candidate;
                        if ui
                            .add(egui::Button::selectable(
                                selected,
                                RichText::new(candidate.label()).size(size::UI),
                            ))
                            .clicked()
                        {
                            settings.units = candidate;
                        }
                    }
                });
            });
            note_label(ui, ROW_UNITS.note);
            ui.add_space(space::S);

            // 3. テーマ (fixed)
            common::figure_line(ui, ROW_THEME.label, ROW_THEME.value);
            note_label(ui, ROW_THEME.note);
            ui.add_space(space::S);

            // 4. 言語 (fixed)
            common::figure_line(ui, ROW_LANG.label, ROW_LANG.value);
            note_label(ui, ROW_LANG.note);
            ui.add_space(space::S);

            // 5. 視差低減 (interactive)
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(ROW_MOTION.label)
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (value, label) in [(true, "オン"), (false, "オフ")] {
                        let selected = settings.reduced_motion == value;
                        if ui
                            .add(egui::Button::selectable(
                                selected,
                                RichText::new(label).size(size::UI),
                            ))
                            .clicked()
                        {
                            settings.reduced_motion = value;
                        }
                    }
                });
            });
            let motion_note = if settings.reduced_motion {
                "オン（0 ms — タブ切替の移動アニメーションを無効化中）"
            } else {
                "オフ（150 ms — 選択タブが 4px 飛び出します）"
            };
            note_label(ui, motion_note);
            ui.add_space(space::S);
        });

        ui.add_space(space::M);
        common::status_line(
            ui,
            Status::Info,
            "単位と視差低減の変更はこのセッション内にすぐ反映されます",
        );

        ui.add_space(space::L);
        common::hairline(ui);
        ui.add_space(space::S);
        ui.label(
            RichText::new(format!(
                "アプリのバージョン　{}",
                crate::screens::firmware::version()
            ))
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
    });
}

fn note_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(size::CAPTION).color(theme::MUTE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_names_its_value_and_why_it_cannot_change() {
        // A settings row that only says "not implemented" is a blank screen with a
        // label. The invariant: each row states a value, and each row that cannot be
        // changed says so in words (DESIGN.md §6, unverified feature).
        for row in &ROWS {
            assert!(!row.label.is_empty());
            assert!(!row.value.is_empty());
            assert!(
                !row.note.is_empty(),
                "{} has no explanation for its fixed value",
                row.label
            );
        }
    }

    #[test]
    fn the_rows_are_the_ones_the_design_names() {
        // DESIGN.md §5 lists exactly five settings; a sixth added here is a design
        // change rather than a detail.
        let labels: Vec<&str> = ROWS.iter().map(|row| row.label).collect();
        assert_eq!(
            labels,
            ["地図キャッシュ", "単位", "テーマ", "言語", "視差低減"]
        );
    }

    #[test]
    fn no_row_is_marked_with_amber() {
        // §3: unverified is mute, never danger. The rows are the easiest place in the
        // product to reach for an amber warning, which is exactly why this is asserted.
        for row in &ROWS {
            assert!(!row.note.contains("危険") && !row.note.contains("注意"));
        }
    }

    #[test]
    fn no_row_exposes_internal_document_names() {
        // User-facing copy must not leak repository file names or spec sections.
        for row in &ROWS {
            for field in [row.label, row.value, row.note] {
                assert!(
                    !field.contains(".md") && !field.contains('§'),
                    "settings copy exposed an internal reference: {field}"
                );
            }
        }
    }

    #[test]
    fn unit_system_formats_metric_and_imperial_values() {
        assert_eq!(UnitSystem::Metric.format_speed(Some(10.0)), "10.0 km/h");
        assert_eq!(UnitSystem::Imperial.format_speed(Some(10.0)), "6.2 mph");
        assert_eq!(UnitSystem::Imperial.format_speed(None), "—");

        assert_eq!(UnitSystem::Metric.format_distance_km(10.0), "10.0 km");
        assert_eq!(UnitSystem::Imperial.format_distance_km(10.0), "6.2 mi");

        assert_eq!(UnitSystem::Metric.format_altitude(Some(100.0)), "100 m");
        assert_eq!(UnitSystem::Imperial.format_altitude(Some(100.0)), "328 ft");
        assert_eq!(UnitSystem::Imperial.format_altitude(None), "—");
    }

    #[test]
    fn reduced_motion_drops_rail_transition_to_zero() {
        let mut settings = AppSettings::default();
        assert!((settings.rail_transition_secs() - 0.15).abs() < f32::EPSILON);
        settings.reduced_motion = true;
        assert!((settings.rail_transition_secs() - 0.0).abs() < f32::EPSILON);
    }
}
