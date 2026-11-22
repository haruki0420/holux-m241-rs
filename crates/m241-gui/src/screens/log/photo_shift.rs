//! 写真の時刻シフトと適用範囲 (DESIGN.md §5; `07-eztour-spec.ja.md` §6.1-7).
//!
//! The screen exposes ezTour's three scopes and its day/hour/minute/second shift
//! through `geotag::plan_with_time_shift`, so the plan the person reviews and the
//! plan `apply` writes are the same decision. A photo outside the scope is not
//! dropped: it is planned at its existing capture time, which is the core
//! contract this module must not weaken.
//!
//! The camera rule is printed on the screen rather than paraphrased in code: the
//! comparison `CameraModel` makes is exact Make/Model equality, and a name that
//! cannot be read is "unknown", never a near match.

use std::collections::BTreeSet;
use std::path::PathBuf;

use egui::{RichText, Ui};
use m241_core::Track;
use m241_core::log::geotag::{
    CameraModel, GeotagOptions, GeotagPlan, PhotoShiftScope, PhotoTimeShift, plan_with_time_shift,
};

use crate::screens::common::{self, Status};
use crate::theme::{self, size, space};

/// Which photos a clock shift applies to, as the screen's segmented control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ScopeKind {
    /// ezTour's `allPhotos`.
    #[default]
    All,
    /// ezTour's `selPhotos`: the photos checked in the rail.
    Selected,
    /// ezTour's `byCamera`.
    ByCamera,
}

/// The shift controls' state, held beside the plans they produce.
#[derive(Debug, Default)]
pub(super) struct PhotoShiftSettings {
    /// Days, hours, minutes and seconds, summed by the core.
    pub(super) shift: PhotoTimeShift,
    /// Which photos move.
    pub(super) kind: ScopeKind,
    /// Indices into the current photo list for [`ScopeKind::Selected`].
    pub(super) selected: BTreeSet<usize>,
    /// Inline result of the most recent re-plan.
    pub(super) status: Option<(Status, String)>,
}

impl PhotoShiftSettings {
    /// Plan `photos` with the shift restricted to the current scope.
    ///
    /// `selected_photo` is the photo the rail highlights; `byCamera` uses it to
    /// read the Make/Model the scope compares against, because a camera model
    /// cannot be typed in by hand without inventing a normalisation rule.
    pub(super) fn plan(
        &self,
        photos: &[PathBuf],
        track: &Track,
        options: &GeotagOptions,
        selected_photo: Option<usize>,
    ) -> Result<Vec<GeotagPlan>, String> {
        let scope = match self.kind {
            ScopeKind::All => PhotoShiftScope::AllPhotos,
            ScopeKind::Selected => PhotoShiftScope::SelectedPhotos(
                self.selected
                    .iter()
                    .filter_map(|index| photos.get(*index).cloned())
                    .collect(),
            ),
            ScopeKind::ByCamera => {
                let photo = selected_photo
                    .and_then(|index| photos.get(index))
                    .ok_or_else(|| {
                        String::from("カメラ一致は一覧で写真を 1 枚選んでから指定してください")
                    })?;
                let bytes = std::fs::read(photo)
                    .map_err(|err| format!("{} を読めません: {err}", photo.display()))?;
                let camera = CameraModel::from_jpeg(&bytes).ok_or_else(|| {
                    format!("{} からカメラ名（Make/Model）を読めません", photo.display())
                })?;
                PhotoShiftScope::ByCamera(camera)
            }
        };
        plan_with_time_shift(photos, track, options, self.shift, &scope)
            .map_err(|err| format!("写真を再計画できません: {err}"))
    }

    /// Draw the shift and scope controls; `true` when either changed.
    pub(super) fn ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        ui.add_space(space::S);
        ui.horizontal_wrapped(|ui| {
            // `DragValue` is about 40pt wide by default and four of them plus
            // their labels wrapped onto a second row in the 220pt rail; 26 keeps
            // one line.
            ui.spacing_mut().interact_size.x = 26.0;
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(
                RichText::new("シフト")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            for (label, field) in [
                ("日", &mut self.shift.days),
                ("時", &mut self.shift.hours),
                ("分", &mut self.shift.minutes),
                ("秒", &mut self.shift.seconds),
            ] {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                changed |= ui
                    .add(
                        egui::DragValue::new(field)
                            .speed(1.0)
                            .range(-999_i64..=999_i64),
                    )
                    .changed();
            }
        });
        // The scope decides *which* clocks move; `byCamera` also needs its rule
        // next to the choice, because "same camera" is exact Make/Model equality.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("範囲").size(size::CAPTION).color(theme::MUTE));
            for (kind, label) in [
                (ScopeKind::All, "全写真"),
                (ScopeKind::Selected, "選択"),
                (ScopeKind::ByCamera, "カメラ"),
            ] {
                if common::tool_button(ui, label, self.kind == kind, true).clicked()
                    && self.kind != kind
                {
                    self.kind = kind;
                    self.status = None;
                    changed = true;
                }
            }
        });
        let rule = match self.kind {
            ScopeKind::All => None,
            ScopeKind::Selected => Some(format!(
                "一覧でチェックした {} 枚だけシフトします",
                self.selected.len()
            )),
            ScopeKind::ByCamera => Some(String::from(
                "Make/Model の完全一致。読めない名前は不明として動かしません",
            )),
        };
        if let Some(rule) = rule {
            note(ui, &rule);
        }
        if let Some(seconds) = self.shift.total_seconds() {
            if seconds != 0 {
                note(ui, &format!("合計 {seconds} 秒"));
            }
        } else {
            common::status_line(
                ui,
                Status::Err,
                "シフト値が大きすぎて秒に直せません。小さい値にしてください",
            );
        }

        if let Some((status, message)) = &self.status {
            common::status_line(ui, *status, message);
        }
        changed
    }
}

/// A footnote-weight line, the voice every other caption on the screen uses.
fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(size::CAPTION).color(theme::MUTE));
}
