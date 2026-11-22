//! 設定 — the Settings screen (DESIGN.md §5).
//!
//! Plain rows, no rail block: map cache size and location, units, theme, language,
//! reduced motion. Units and reduced motion are live in-app controls held in
//! [`AppSettings`]; the remaining fixed rows explain honestly why they cannot be
//! changed yet (DESIGN.md §7).

use egui::{RichText, Ui};
use m241_core::firmware::PROFILE_V1_13;
use m241_core::firmware::patch::EFFECTIVE_EPOCH_OFFSET;

use crate::screens::common;
use crate::screens::log::tiles;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppSettings {
    /// Unit system for speed, distance, and altitude.
    pub units: UnitSystem,
    /// When true, rail tab transitions drop to 0 ms and playback steps at half
    /// the rate (500 ms instead of 250 ms).
    pub reduced_motion: bool,
    /// When true, the Log screen's map fetches OpenStreetMap tiles for its
    /// background. Default on: the map is the reason that screen exists, and
    /// turning it off is for a metered or offline connection rather than the
    /// normal state.
    pub map_tiles: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            units: UnitSystem::Metric,
            reduced_motion: false,
            map_tiles: true,
        }
    }
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

/// The cache's size row. Its value is measured from the tile cache, so the row
/// is drawn from a live figure rather than a constant (DESIGN.md §5, §10).
const ROW_CACHE_LABEL: &str = "地図キャッシュ";

/// Where the cache lives, as the OS resolves it.
const ROW_CACHE_LOCATION_LABEL: &str = "地図キャッシュの場所";

/// The map tiles' on/off switch. The value is live, like 動きの低減.
const ROW_TILES_LABEL: &str = "地図タイルを使う（要インターネット）";

/// The clear-cache action's label.
const ROW_CLEAR_LABEL: &str = "地図キャッシュを消去";

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

/// The 動きの低減 row's label. Its value is the live setting, so the row is drawn
/// from the buttons rather than from a constant that could disagree with them.
///
/// 「動きの低減」, not 「視差低減」: the setting is reduced motion (turn movement
/// down), and 「視差」 means parallax — a user who wants less motion would not find
/// it under that name (design review, Should 6).
const ROW_MOTION_LABEL: &str = "動きの低減";

/// The rows DESIGN.md §5 names, in its order (checked in tests).
///
/// 動きの低減 is absent from this list on purpose: unlike the fixed rows it has no
/// static value, because its value is whatever the buttons currently show.
#[cfg(test)]
const FIXED_ROWS: [Row; 3] = [ROW_UNITS, ROW_THEME, ROW_LANG];

/// Draw the screen body. The rail is drawn by the shell, not here.
pub fn ui(ui: &mut Ui, settings: &mut AppSettings) {
    // Version and the session-scope sentence are pinned to the bottom: they say
    // what this screen cannot promise, and a footnote that scrolls away is not a
    // footnote (DESIGN.md §5, §6).
    common::footer(ui, "m241-settings-footer", |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "アプリのバージョン　{}",
                    crate::screens::firmware::version()
                ))
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("変更はこのセッション内だけ有効です")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
        });
    });

    common::scrolling(ui, "m241-settings-body", |ui| {
        common::heading(
            ui,
            "設定",
            "単位と動きの低減はアプリ内で切り替えられます（終了後は既定値に戻ります）。",
        );
        ui.add_space(space::L);

        // Plain rows, no rail block (DESIGN.md §5): a ruled settings table with
        // 32pt rows, a 28pt caption header, and the note under the row it
        // explains.
        common::table_header(
            ui,
            &[
                ("項目", 0.5, egui::Align::Min),
                ("値", 0.5, egui::Align::Max),
            ],
        );
        cache_rows(ui);
        option_row(
            ui,
            ROW_TILES_LABEL,
            &tiles_note(settings.map_tiles),
            settings,
            |ui, settings| {
                for (value, label) in [(true, "オン"), (false, "オフ")] {
                    if common::choice_button(ui, label, settings.map_tiles == value).clicked() {
                        settings.map_tiles = value;
                    }
                }
            },
        );
        option_row(
            ui,
            ROW_UNITS.label,
            ROW_UNITS.note,
            settings,
            |ui, settings| {
                for candidate in [UnitSystem::Imperial, UnitSystem::Metric] {
                    if common::choice_button(ui, candidate.label(), settings.units == candidate)
                        .clicked()
                    {
                        settings.units = candidate;
                    }
                }
            },
        );
        fixed_row(ui, ROW_THEME);
        fixed_row(ui, ROW_LANG);
        let motion_note = if settings.reduced_motion {
            "オン（0 ms。タブ切替の移動アニメーションを無効化中）".to_owned()
        } else {
            format!(
                "オフ（{} ms。選択タブが 4px 飛び出します）",
                (AppSettings::DEFAULT_RAIL_TRANSITION_SECS * 1000.0).round() as u32
            )
        };
        option_row(
            ui,
            ROW_MOTION_LABEL,
            &motion_note,
            settings,
            |ui, settings| {
                for (value, label) in [(true, "オン"), (false, "オフ")] {
                    if common::choice_button(ui, label, settings.reduced_motion == value).clicked()
                    {
                        settings.reduced_motion = value;
                    }
                }
            },
        );
        clear_cache_row(ui);

        // The colophon of a reference manual: the facts a person needs when the
        // box and the release note are long gone. The figures are read from
        // `m241-core` — the profile's name and the measured patch offset — so a
        // changed profile changes this page. The byte count is the V1.13 patch's
        // own figure (two epoch bytes plus the marker digit); Settings never opens
        // an image, so it cannot count a `PatchPlan` here.
        //
        // The session-scope sentence lives in the heading and the footer, so it
        // is not repeated here as a status line: that line cost the last row of
        // this block at 1280×800.
        ui.add_space(space::M);
        ui.label(
            RichText::new("このアプリについて")
                .size(size::TITLE)
                .color(theme::INK),
        );
        ui.add_space(space::S);
        common::hairline(ui);
        for (label, value) in [
            ("対応ファーム", String::from(PROFILE_V1_13.name)),
            ("修正する位置", patch_offset_value()),
        ] {
            common::figure_row_compact(ui, label, &value);
        }
    });
}

/// The bytes a V1.13 patch rewrites: two in the epoch constant and one in the
/// version marker.
///
/// Not derived from a loaded image because this screen never opens one. The test
/// below checks the two epoch bytes against `m241-core`'s constants, so a change
/// there fails here rather than leaving a stale count on the page.
const V1_13_CHANGED_BYTES: usize = 3;

/// The 修正する位置 row's value.
///
/// Built from [`EFFECTIVE_EPOCH_OFFSET`] rather than written as `0x402F4`: the
/// colophon promises figures that come from `m241-core`, and a literal would keep
/// pointing at the old address if the measured offset ever moved.
#[must_use]
fn patch_offset_value() -> String {
    format!("0x{EFFECTIVE_EPOCH_OFFSET:05X}（{V1_13_CHANGED_BYTES} バイトが変わります）")
}

/// A fixed settings row: the value on the right, the explanation underneath,
/// one hairline under the pair (DESIGN.md §3, row height 32).
fn fixed_row(ui: &mut Ui, row: Row) {
    common::figure_row_open(ui, row.label, row.value);
    note_label(ui, row.note);
    common::hairline(ui);
    ui.add_space(space::XS);
}

/// The tile cache's two rows: how much it holds, and where it is (DESIGN.md §5).
///
/// Both values come from the tile module: the size is measured, the path is the
/// one the fetcher writes to. A row that printed a constant would be a claim
/// about a cache that has stopped existing.
fn cache_rows(ui: &mut Ui) {
    let usage = tiles::cache_usage();
    let size = usage.map_or_else(|| String::from("—"), |usage| usage.label());
    dynamic_row(
        ui,
        ROW_CACHE_LABEL,
        &size,
        "タイルは 30 日で古くなり、200 MB を超えると古い順に消えます",
    );
    let location = tiles::CachePaths::locate().map_or_else(
        || String::from("—（この環境では決められません）"),
        |cache| cache.root.display().to_string(),
    );
    dynamic_row(
        ui,
        ROW_CACHE_LOCATION_LABEL,
        &location,
        "背景地図のタイルを保存する場所です",
    );
}

/// A settings row whose value is measured at draw time.
fn dynamic_row(ui: &mut Ui, label: &str, value: &str, note: &str) {
    common::figure_row_open(ui, label, value);
    note_label(ui, note);
    common::hairline(ui);
    ui.add_space(space::XS);
}

/// The 地図タイル row's explanation, which states the live value in words.
fn tiles_note(enabled: bool) -> String {
    if enabled {
        String::from("オン（OpenStreetMap のタイルを背景に表示します）")
    } else {
        String::from("オフ（模式キャンバスのみ。オフラインでもこの画面は使えます）")
    }
}

/// The clear-cache action. Deleting local tiles is not a destructive device
/// action, so it carries no amber; the result is stated as text beside it.
fn clear_cache_row(ui: &mut Ui) {
    let id = ui.scope_id().with("m241-settings-cache-cleared");
    let mut status: Option<String> = ui.data(|data| data.get_temp(id));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(ROW_CLEAR_LABEL)
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if common::choice_button(ui, "消去", false).clicked() {
                status = Some(match tiles::clear_cache() {
                    Ok(()) => String::from("キャッシュを消去しました"),
                    Err(message) => message,
                });
                ui.data_mut(|data| data.insert_temp(id, status.clone()));
            }
        });
    });
    if let Some(message) = &status {
        note_label(ui, message);
    }
    common::hairline(ui);
    ui.add_space(space::XS);
}

/// A settings row whose value is a pair of live controls rather than a figure.
fn option_row(
    ui: &mut Ui,
    label: &str,
    note: &str,
    settings: &mut AppSettings,
    controls: impl FnOnce(&mut Ui, &mut AppSettings),
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            controls(ui, settings);
        });
    });
    note_label(ui, note);
    common::hairline(ui);
    ui.add_space(space::XS);
}

fn note_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(size::CAPTION).color(theme::MUTE));
}

#[cfg(test)]
mod tests {
    use super::*;
    use m241_core::firmware::image::{GPS_EPOCH_UNIX, GPS_EPOCH_UNIX_WNRO};

    #[test]
    fn every_row_names_its_value_and_why_it_cannot_change() {
        // A settings row that only says "not implemented" is a blank screen with a
        // label. The invariant: each row states a value, and each row that cannot be
        // changed says so in words (DESIGN.md §6, unverified feature).
        for row in &FIXED_ROWS {
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
        // DESIGN.md §5 lists the cache's size and location, units, theme, language
        // and reduced motion. The tile switch and the clear action are this
        // change's additions; the cache values are measured rather than written
        // here, so the labels below are the ones the page draws in order.
        let labels: Vec<&str> = [
            ROW_CACHE_LABEL,
            ROW_CACHE_LOCATION_LABEL,
            ROW_TILES_LABEL,
            ROW_UNITS.label,
            ROW_THEME.label,
            ROW_LANG.label,
            ROW_MOTION_LABEL,
            ROW_CLEAR_LABEL,
        ]
        .to_vec();
        assert_eq!(
            labels,
            [
                "地図キャッシュ",
                "地図キャッシュの場所",
                "地図タイルを使う（要インターネット）",
                "単位",
                "テーマ",
                "言語",
                "動きの低減",
                "地図キャッシュを消去"
            ]
        );
    }

    #[test]
    fn the_colophon_builds_the_patch_offset_from_the_core_constant() {
        // The row was a literal while its comment said every figure came from
        // `m241-core`, so a changed offset would have left the page pointing at
        // the old address. Two epoch bytes change and the marker digit changes.
        let differing = GPS_EPOCH_UNIX
            .to_le_bytes()
            .iter()
            .zip(GPS_EPOCH_UNIX_WNRO.to_le_bytes())
            .filter(|pair| *pair.0 != pair.1)
            .count();
        assert_eq!(
            differing + 1,
            V1_13_CHANGED_BYTES,
            "the V1.13 patch changes {differing} epoch bytes plus the marker digit"
        );
        let value = patch_offset_value();
        assert!(
            value.contains(&format!("0x{EFFECTIVE_EPOCH_OFFSET:05X}")),
            "the row does not name the core offset: {value}"
        );
        assert!(
            value.contains(&format!("{V1_13_CHANGED_BYTES} バイト")),
            "the row does not state the byte count: {value}"
        );
    }

    #[test]
    fn no_row_is_marked_with_amber() {
        // §3: unverified is mute, never danger. The rows are the easiest place in the
        // product to reach for an amber warning, which is exactly why this is asserted.
        for row in &FIXED_ROWS {
            assert!(!row.note.contains("危険") && !row.note.contains("注意"));
        }
    }

    #[test]
    fn no_row_exposes_internal_document_names() {
        // User-facing copy must not leak repository file names or spec sections.
        for row in &FIXED_ROWS {
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
    fn map_tiles_default_on_and_the_note_states_the_live_value() {
        // A default of off would leave the new map feature invisible to anyone
        // who never opens Settings; the note must state which way it is.
        assert!(AppSettings::default().map_tiles);
        assert!(tiles_note(true).starts_with("オン"));
        assert!(tiles_note(false).starts_with("オフ"));
        for note in [tiles_note(true), tiles_note(false)] {
            assert!(!note.contains(".md") && !note.contains('§'));
        }
    }

    #[test]
    fn reduced_motion_drops_rail_transition_to_zero() {
        let mut settings = AppSettings::default();
        assert!((settings.rail_transition_secs() - 0.15).abs() < f32::EPSILON);
        settings.reduced_motion = true;
        assert!((settings.rail_transition_secs() - 0.0).abs() < f32::EPSILON);
    }
}
