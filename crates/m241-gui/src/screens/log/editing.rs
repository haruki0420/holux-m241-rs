//! トラック編集: split / merge / delete through the core `TrackEditor`.
//!
//! The editor holds the tracks as a list with an undo/redo stack; the screen
//! draws their concatenation, so a split changes track boundaries without
//! hiding points. Every mutation is confirmed before it runs: a split or a
//! delete is undoable here, but the person is told what will change first.
//!
//! A range split costs two undo steps (one split at each edge), because the core
//! commits each split separately. Undo therefore walks back through the range
//! split in two clicks; the screen's status line says what the last step did.

use egui::{RichText, Ui};
use m241_core::Track;
use m241_core::log::edit::{TrackEditor, merge_tracks};

use crate::screens::common::{self, Status};
use crate::theme::{self, size, space};

/// A parsed edit, armed by its button and run by 「実行」.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditCommand {
    /// Split the range `start..end` out of `track`; `end == None` is one cut.
    Split {
        track: usize,
        start: usize,
        end: Option<usize>,
    },
    /// Concatenate every track in editing order.
    MergeAll,
    /// Delete `points` from `track`.
    Delete { track: usize, points: Vec<usize> },
}

impl EditCommand {
    /// The sentence the confirmation prompt shows.
    ///
    /// Every point number here is 1-based, the way the fields and the list show
    /// them; the command carries 0-based indices and the summary converts once.
    fn summary(&self) -> String {
        match self {
            Self::Split {
                track,
                start,
                end: Some(end),
            } => format!(
                "トラック {} の {}〜{end} 点目を分割",
                track.saturating_add(1),
                start.saturating_add(1)
            ),
            Self::Split {
                track,
                start,
                end: None,
            } => format!(
                "トラック {} の {} 点目の前で分割",
                track.saturating_add(1),
                start.saturating_add(1)
            ),
            Self::MergeAll => String::from("すべてのトラックを編集順に結合"),
            Self::Delete { track, points } => format!(
                "トラック {} から {} 点（{}）を削除",
                track.saturating_add(1),
                points.len(),
                join(points)
            ),
        }
    }
}

/// Editing state: the history-bearing editor, the range fields, the prompt.
#[derive(Debug, Default)]
pub(super) struct TrackEditState {
    editor: TrackEditor,
    /// Zero-based track the range fields apply to.
    pub(super) track_index: usize,
    /// First point of the split range, 1-based as the field shows it.
    pub(super) split_from: String,
    /// Last point of the split range (1-based); empty for a single cut at
    /// `split_from`.
    pub(super) split_to: String,
    /// Comma- or space-separated 1-based point numbers to delete.
    pub(super) delete_list: String,
    pending: Option<EditCommand>,
    /// Inline result of the last request, confirmation or undo.
    pub(super) status: Option<(Status, String)>,
}

impl TrackEditState {
    /// Re-seed the editor from the screen's current track, dropping history.
    pub(super) fn reset(&mut self, track: &Track) {
        self.editor = TrackEditor::from_track(track.clone());
        self.track_index = 0;
        self.split_from.clear();
        self.split_to.clear();
        self.delete_list.clear();
        self.pending = None;
        self.status = None;
    }

    /// The tracks the editor holds, concatenated for the map and tables.
    pub(super) fn merged(&self) -> Track {
        merge_tracks(self.editor.tracks())
    }

    /// The number of tracks the split/merge operations produced.
    pub(super) fn track_count(&self) -> usize {
        self.editor.tracks().len()
    }

    /// Arm a split from the current range fields.
    pub(super) fn request_split(&mut self) {
        match self.split_command() {
            Ok(command) => {
                self.pending = Some(command);
                self.status = None;
            }
            Err(message) => self.status = Some((Status::Err, message)),
        }
    }

    /// Arm a merge of every track.
    pub(super) fn request_merge_all(&mut self) {
        if self.track_count() < 2 {
            self.status = Some((
                Status::Err,
                String::from("結合するトラックがありません（分割すると増えます）"),
            ));
            return;
        }
        self.pending = Some(EditCommand::MergeAll);
        self.status = None;
    }

    /// Arm a delete from the current point list.
    pub(super) fn request_delete(&mut self) {
        match self.delete_command() {
            Ok(command) => {
                self.pending = Some(command);
                self.status = None;
            }
            Err(message) => self.status = Some((Status::Err, message)),
        }
    }

    /// Run the armed edit, returning the concatenated track when it changed.
    pub(super) fn confirm(&mut self) -> Option<Track> {
        let command = self.pending.take()?;
        match self.execute(&command) {
            Ok(()) => {
                self.status = Some((Status::Ok, self.describe_after(&command)));
                Some(self.merged())
            }
            Err(message) => {
                self.status = Some((Status::Err, format!("編集できませんでした: {message}")));
                None
            }
        }
    }

    /// Undo one core edit, returning the track when there was one.
    pub(super) fn undo(&mut self) -> Option<Track> {
        self.pending = None;
        if self.editor.undo() {
            self.status = Some((Status::Ok, String::from("元に戻しました")));
            Some(self.merged())
        } else {
            self.status = Some((Status::Info, String::from("元に戻す操作はありません")));
            None
        }
    }

    /// Redo one core edit, returning the track when there was one.
    pub(super) fn redo(&mut self) -> Option<Track> {
        self.pending = None;
        if self.editor.redo() {
            self.status = Some((Status::Ok, String::from("やり直しました")));
            Some(self.merged())
        } else {
            self.status = Some((Status::Info, String::from("やり直す操作はありません")));
            None
        }
    }

    /// Draw the editing block; `Some(track)` when an edit or history move ran.
    pub(super) fn ui(&mut self, ui: &mut Ui) -> Option<Track> {
        let count = self.track_count();
        if count == 0 {
            common::status_line(ui, Status::Info, "編集するトラックがありません");
            return None;
        }
        self.track_index = self.track_index.min(count.saturating_sub(1));

        let mut outcome: Option<Track> = None;
        ui.add_space(space::XS);
        ui.horizontal_wrapped(|ui| {
            if common::tool_button(ui, "元に戻す", false, self.editor.can_undo()).clicked() {
                outcome = self.undo();
            }
            if common::tool_button(ui, "やり直す", false, self.editor.can_redo()).clicked() {
                outcome = self.redo();
            }
        });
        if outcome.is_some() {
            return outcome;
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("対象").size(size::CAPTION).color(theme::MUTE));
            // The editor indexes tracks from 0; the field shows 1..=count so the
            // number on screen matches the confirmation sentence below.
            let mut shown = self.track_index.saturating_add(1);
            if ui
                .add(egui::DragValue::new(&mut shown).range(1..=count.max(1)))
                .changed()
            {
                self.track_index = shown.saturating_sub(1).min(count.saturating_sub(1));
            }
            ui.label(
                RichText::new(format!("/ {count} トラック"))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
        // The field labels sit outside the boxes: a 30pt box clips its own hint
        // text to an ellipsis, which loses start-versus-end in the rail.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("分割").size(size::CAPTION).color(theme::MUTE));
            for (label, value) in [("開始", &mut self.split_from), ("終了", &mut self.split_to)]
            {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                let _response = ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(30.0)
                        .hint_text("点")
                        .font(egui::TextStyle::Monospace),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            if common::tool_button(ui, "範囲を分割", false, true).clicked() {
                self.request_split();
            }
            ui.label(
                RichText::new("点番号は 1 始まり。終了が空なら 1 箇所")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("削除").size(size::CAPTION).color(theme::MUTE));
            let width = (ui.available_width() - 64.0).clamp(56.0, 96.0);
            let _response = ui.add(
                egui::TextEdit::singleline(&mut self.delete_list)
                    .hint_text("点番号")
                    .desired_width(width)
                    .font(egui::TextStyle::Monospace),
            );
            if common::tool_button(ui, "削除", false, true).clicked() {
                self.request_delete();
            }
        });
        ui.add_space(space::XS);
        if common::tool_button(ui, "全トラックを結合", false, count >= 2).clicked() {
            self.request_merge_all();
        }

        if let Some(command) = &self.pending {
            common::status_line(
                ui,
                Status::Waiting,
                &format!("{} を実行しますか？", command.summary()),
            );
            ui.horizontal_wrapped(|ui| {
                if common::primary_button(ui, "実行", true).clicked() {
                    outcome = self.confirm();
                }
                if common::secondary_button(ui, "キャンセル", true).clicked() {
                    self.pending = None;
                }
            });
        }
        if let Some((status, message)) = &self.status {
            common::status_line(ui, *status, message);
        }
        outcome
    }

    /// Parse and validate the range fields into a command.
    ///
    /// The fields are 1-based point numbers; `EditCommand` carries 0-based
    /// indices. The end is an inclusive 1-based number, which is the exclusive
    /// 0-based split boundary the core takes (`end == length` means "to the last
    /// point", so no second cut is needed).
    fn split_command(&self) -> Result<EditCommand, String> {
        let track = self.editor.track(self.track_index).ok_or_else(|| {
            String::from("対象のトラックが見つかりません。対象を選び直してください")
        })?;
        let length = track.points.len();
        let start = parse_point(&self.split_from, "分割開始")?;
        if start == 0 || start > length {
            return Err(format!(
                "分割開始は 1..={length} の範囲で指定してください（{length} 点）"
            ));
        }
        let start = start - 1;
        let end = if self.split_to.trim().is_empty() {
            None
        } else {
            let end = parse_point(&self.split_to, "分割終了")?;
            if end == 0 || end > length {
                return Err(format!(
                    "分割終了は 1..={length} の範囲で指定してください（{length} 点）"
                ));
            }
            Some(end)
        };
        match end {
            Some(end) if end < start.saturating_add(1) => {
                return Err(String::from(
                    "分割終了は分割開始以降の点番号にしてください（例: 開始 10・終了 20）",
                ));
            }
            Some(end) if start == 0 && end == length => {
                return Err(String::from(
                    "範囲が全点なので分割されません。開始を 2 以上にするか、終了を最終点より前にしてください",
                ));
            }
            None if start == 0 => {
                return Err(String::from(
                    "分割開始が 1 です。終了を指定するか 2 以上を入力してください",
                ));
            }
            _ => {}
        }
        Ok(EditCommand::Split {
            track: self.track_index,
            start,
            end,
        })
    }

    /// Parse and validate the delete list into a command.
    ///
    /// The list is 1-based point numbers; the command carries 0-based indices.
    fn delete_command(&self) -> Result<EditCommand, String> {
        let track = self.editor.track(self.track_index).ok_or_else(|| {
            String::from("対象のトラックが見つかりません。対象を選び直してください")
        })?;
        let length = track.points.len();
        let mut points: Vec<usize> = self
            .delete_list
            .split(|character: char| {
                character == ',' || character == '、' || character.is_whitespace()
            })
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<usize>().map_err(|_| {
                    format!("{part:?} は点番号ではありません（1 始まりの数字をカンマ区切りで）")
                })
            })
            .collect::<Result<_, _>>()?;
        if points.is_empty() {
            return Err(String::from(
                "削除する点番号を入力してください（1 始まり。例: 3,5,9）",
            ));
        }
        if let Some(out_of_range) = points.iter().find(|point| **point == 0 || **point > length) {
            return Err(format!(
                "点番号 {out_of_range} は範囲外です（1..={length}）"
            ));
        }
        points.iter_mut().for_each(|point| *point -= 1);
        points.sort_unstable();
        points.dedup();
        Ok(EditCommand::Delete {
            track: self.track_index,
            points,
        })
    }

    /// Apply one command to the editor.
    fn execute(&mut self, command: &EditCommand) -> Result<(), String> {
        match command {
            EditCommand::Split { track, start, end } => {
                let length = self
                    .editor
                    .track(*track)
                    .map(|held| held.points.len())
                    .ok_or_else(|| {
                        String::from("対象のトラックが見つかりません。対象を選び直してください")
                    })?;
                // Split at the later edge first: the earlier index still refers
                // to the first half after that, which is what makes the range
                // land in one piece.
                if let Some(end) = end
                    && *end < length
                {
                    self.editor.split(*track, *end).map_err(edit_error)?;
                }
                if *start > 0 {
                    self.editor.split(*track, *start).map_err(edit_error)?;
                }
                Ok(())
            }
            EditCommand::MergeAll => {
                let indices: Vec<usize> = (0..self.track_count()).collect();
                self.editor.merge(&indices).map_err(edit_error)
            }
            EditCommand::Delete { track, points } => self
                .editor
                .delete_points(*track, points)
                .map_err(edit_error),
        }
    }

    /// The success sentence for a command that has just run.
    fn describe_after(&self, command: &EditCommand) -> String {
        let tracks = self.track_count();
        let points: usize = self
            .editor
            .tracks()
            .iter()
            .map(|track| track.points.len())
            .sum();
        match command {
            EditCommand::Split { .. } => {
                format!("分割しました（{tracks} トラック・{points} 点）")
            }
            EditCommand::MergeAll => {
                format!("結合しました（{tracks} トラック・{points} 点）")
            }
            EditCommand::Delete {
                points: removed, ..
            } => {
                format!("{} 点を削除しました（残り {points} 点）", removed.len())
            }
        }
    }
}

/// Parse one 1-based point number, naming the field it came from.
fn parse_point(text: &str, what: &str) -> Result<usize, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{what}を入力してください（1 始まりの点番号）"));
    }
    trimmed
        .parse::<usize>()
        .map_err(|_| format!("{what}は 1 以上の点番号で入力してください（例: 10。0 は使えません）"))
}

/// A core editing error plus the core's own next-step hint, when it has one.
///
/// `execute` pre-validates every range so these are fallbacks; the hint keeps
/// the sentence actionable in the case the pre-check and the core disagree.
fn edit_error(err: m241_core::Error) -> String {
    err.hint()
        .map_or_else(|| err.to_string(), |hint| format!("{err}。{hint}"))
}

/// `1, 3, 5` for the confirmation prompt, in the 1-based numbers the fields use.
fn join(points: &[usize]) -> String {
    let mut shown = String::new();
    for (index, point) in points.iter().enumerate() {
        if index > 0 {
            shown.push_str(", ");
        }
        shown.push_str(&point.saturating_add(1).to_string());
    }
    shown
}
