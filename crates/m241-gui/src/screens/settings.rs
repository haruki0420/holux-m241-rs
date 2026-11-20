//! 設定 — the Settings screen (DESIGN.md §5).
//!
//! "Plain rows, no rail block: map cache size and location, units, theme, language,
//! reduced motion." The screen is written as those rows even though none of them can
//! be changed yet, because a settings screen that says only 「未実装」 answers none of
//! the questions a person opened it with. Each row states the current value, and a
//! row whose control does not exist says so in words rather than showing a control
//! that does nothing (DESIGN.md §7).
//!
//! Nothing here is persisted: the values are the ones that would ship as defaults,
//! and the screen says that in its one sentence instead of pretending otherwise.

use egui::{RichText, Ui};

use crate::screens::common::{self, Status};
use crate::theme::{self, size, space};

/// One settings row: what it is, what it currently is, and why it cannot change.
struct Row {
    label: &'static str,
    value: &'static str,
    /// The sentence under the figure saying what is missing, in the product's
    /// voice. Required for every row: an unchangeable setting with no explanation
    /// is indistinguishable from a broken control.
    note: &'static str,
}

/// The rows DESIGN.md §5 names, in its order.
const ROWS: [Row; 5] = [
    Row {
        label: "地図キャッシュ",
        value: "0 B",
        note: "地図タイルをまだ読んでいないため、キャッシュは空です",
    },
    Row {
        label: "単位",
        value: "km・km/h",
        note: "単位の切り替えは未検証",
    },
    Row {
        label: "テーマ",
        value: "暗色のみ",
        note: "明色テーマは未設計のため暗色テーマのみ対応",
    },
    Row {
        label: "言語",
        value: "日本語",
        note: "日本語以外の表示は未検証",
    },
    Row {
        label: "視差低減",
        value: "OS の設定に従う",
        note: "アプリ内での切り替えは未検証",
    },
];

/// Draw the screen body. The rail is drawn by the shell, not here.
pub fn ui(ui: &mut Ui) {
    common::scrolling(ui, "m241-settings-body", |ui| {
        common::heading(
            ui,
            "設定",
            "この画面の設定はまだ保存されません。表示は出荷時の既定値です。",
        );
        ui.add_space(space::L);

        common::panel(ui, "表示と単位", |ui| {
            for row in &ROWS {
                common::figure_line(ui, row.label, row.value);
                ui.label(
                    RichText::new(row.note)
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                ui.add_space(space::S);
            }
        });

        ui.add_space(space::M);
        common::status_line(
            ui,
            Status::Unverified,
            "保存と変更の操作は未検証 — いまは値の表示のみです",
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
}
