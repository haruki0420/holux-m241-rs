//! Photo clock shift and the photos it applies to (DESIGN.md §5;
//! `07-eztour-spec.ja.md` §6.1-7).
//!
//! The screen exposes ezTour's three scopes and its day/hour/minute/second shift
//! through `geotag::plan_with_time_shift`, so the plan the person reviews and the
//! plan `apply` writes are the same decision. A photo outside the scope is not
//! dropped: it is planned at its existing capture time, which is the core
//! contract this module must not weaken.
//!
//! The camera rule is printed on the screen rather than paraphrased in code: the
//! comparison `CameraModel` makes is exact Make/Model equality, and a name that
//! cannot be read is "unknown", never a near match. Resolving the camera reads
//! the base photo, and (when the camera changes) the folder once; the set is
//! cached so moving the shift control does not read every JPEG again.
//!
//! Limits worth knowing: the Make/Model grouping rule is unconfirmed against a
//! real camera (`docs/dev/06-open-questions.md`), and re-planning reads the
//! photos because the core planner takes paths, not parsed EXIF. A folder of
//! several hundred photos therefore makes one re-plan noticeably slower; the
//! controls re-plan when a value settles rather than on every pointer step.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use m241_core::Track;
use m241_core::log::geotag::{
    CameraModel, GeotagOptions, GeotagPlan, PhotoShiftScope, PhotoTimeShift, plan_with_time_shift,
};

use super::PhotoEntry;
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

/// The camera a `byCamera` plan resolved, and the photos whose Exif names it.
#[derive(Debug, Clone)]
struct ResolvedCamera {
    model: CameraModel,
    photos: BTreeSet<PathBuf>,
}

impl ResolvedCamera {
    /// The Make/Model as the screen shows it; at least one half is always set.
    fn label(&self) -> String {
        match (&self.model.make, &self.model.model) {
            (Some(make), Some(model)) => format!("{make} {model}"),
            (Some(make), None) => make.clone(),
            (None, Some(model)) => model.clone(),
            (None, None) => String::from("（不明）"),
        }
    }
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
    /// The base camera `byCamera` resolved, with the photos it names.
    resolved: Option<ResolvedCamera>,
    /// Photos of the current plans whose clocks the shift moves.
    moved: usize,
    /// Whether the controls have a value the plans were not built from yet.
    ///
    /// The person may press 「写真に位置」 in the pinned footer before the control
    /// that changed has settled, so the write path re-plans while this is set.
    pub(super) pending: bool,
    /// Whether the plans on screen are older than the controls. A failed
    /// re-plan keeps the previous entries so the list and controls survive, and
    /// this flag stops that older plan from being written.
    pub(super) plan_stale: bool,
}

impl PhotoShiftSettings {
    /// Plan `photos` with the shift restricted to the current scope.
    ///
    /// `selected_photo` is the photo the rail highlights; `byCamera` uses it to
    /// read the Make/Model the scope compares against, because a camera model
    /// cannot be typed in by hand without inventing a normalisation rule.
    ///
    /// # Errors
    ///
    /// The scope's own refusal, as a Japanese sentence: no highlighted photo,
    /// a base photo whose Make/Model cannot be read, an unreadable photo, a
    /// shift that does not fit in seconds, or (for `選択`) a non-zero shift with
    /// no photo checked.
    pub(super) fn plan(
        &mut self,
        photos: &[PathBuf],
        track: &Track,
        options: &GeotagOptions,
        selected_photo: Option<usize>,
    ) -> Result<Vec<GeotagPlan>, String> {
        let scope = match self.kind {
            ScopeKind::All => PhotoShiftScope::AllPhotos,
            ScopeKind::Selected => {
                let selected: Vec<PathBuf> = self
                    .selected
                    .iter()
                    .filter_map(|index| photos.get(*index).cloned())
                    .collect();
                if selected.is_empty() && self.shift_seconds().is_some_and(|seconds| seconds != 0) {
                    return Err(String::from(
                        "時計を動かす写真がありません。一覧で写真をチェックしてください",
                    ));
                }
                PhotoShiftScope::SelectedPhotos(selected)
            }
            ScopeKind::ByCamera => {
                PhotoShiftScope::ByCamera(self.resolve_camera(photos, selected_photo)?)
            }
        };
        plan_with_time_shift(photos, track, options, self.shift, &scope)
            .map_err(|err| format!("写真を再計画できません: {err}"))
    }

    /// The camera the base photo names, caching the set of photos it matches.
    fn resolve_camera(
        &mut self,
        photos: &[PathBuf],
        selected_photo: Option<usize>,
    ) -> Result<CameraModel, String> {
        let base = selected_photo
            .and_then(|index| photos.get(index))
            .ok_or_else(|| {
                String::from("カメラ一致は一覧で写真を 1 枚選んでから指定してください")
            })?;
        let bytes =
            std::fs::read(base).map_err(|err| format!("{} を読めません: {err}", base.display()))?;
        let camera = CameraModel::from_jpeg(&bytes).ok_or_else(|| {
            format!(
                "{} からカメラ名（Make/Model）を読めません。別の写真を基準にするか、範囲を「全写真」にしてください",
                base.display()
            )
        })?;
        if self
            .resolved
            .as_ref()
            .is_none_or(|resolved| resolved.model != camera)
        {
            let mut matches = BTreeSet::new();
            for photo in photos {
                let bytes = std::fs::read(photo)
                    .map_err(|err| format!("{} を読めません: {err}", photo.display()))?;
                if CameraModel::from_jpeg(&bytes).as_ref() == Some(&camera) {
                    matches.insert(photo.clone());
                }
            }
            self.resolved = Some(ResolvedCamera {
                model: camera.clone(),
                photos: matches,
            });
        }
        Ok(camera)
    }

    /// Whether `photo` names the camera the current `byCamera` plan already uses.
    ///
    /// Used by the rail: selecting another photo of the same camera leaves the
    /// plan identical, so the folder is not re-read for nothing.
    pub(super) fn by_camera_unchanged(&self, photo: &Path) -> bool {
        let Some(resolved) = &self.resolved else {
            return false;
        };
        std::fs::read(photo)
            .ok()
            .and_then(|bytes| CameraModel::from_jpeg(&bytes))
            .is_some_and(|camera| camera == resolved.model)
    }

    /// Record how many of `plans` the shift moves, for the next message.
    pub(super) fn note_plans(&mut self, plans: &[GeotagPlan]) {
        self.moved = self.shifted_count(plans);
    }

    /// The photos among `plans` whose clocks the shift moves and that will be
    /// written: in scope and matched to a point.
    pub(super) fn shifted_count(&self, plans: &[GeotagPlan]) -> usize {
        if !self.shift_seconds().is_some_and(|seconds| seconds != 0) {
            return 0;
        }
        match self.kind {
            ScopeKind::All => plans.iter().filter(|plan| plan.will_write()).count(),
            ScopeKind::Selected => plans
                .iter()
                .enumerate()
                .filter(|(index, plan)| self.selected.contains(index) && plan.will_write())
                .count(),
            ScopeKind::ByCamera => self.resolved.as_ref().map_or(0, |resolved| {
                plans
                    .iter()
                    .filter(|plan| plan.will_write() && resolved.photos.contains(&plan.photo))
                    .count()
            }),
        }
    }

    /// The resolved base camera in words, or `None` before a `byCamera` plan
    /// has resolved one.
    pub(super) fn camera_label(&self) -> Option<String> {
        self.resolved.as_ref().map(ResolvedCamera::label)
    }

    /// The scope in words, for the confirmation prompt and the shift note.
    fn scope_label(&self) -> String {
        match self.kind {
            ScopeKind::All => String::from("全写真"),
            ScopeKind::Selected => format!("選択 {} 枚", self.selected.len()),
            ScopeKind::ByCamera => self.camera_label().map_or_else(
                || String::from("カメラ（基準写真を選ぶと決まります）"),
                |label| format!("カメラ {label}"),
            ),
        }
    }

    /// The armed shift as one sentence, with the in-scope and out-of-scope
    /// counts of the photos that will be written. `None` when no shift is armed.
    pub(super) fn shift_note(&self, entries: &[PhotoEntry]) -> Option<String> {
        let seconds = self.shift.total_seconds()?;
        if seconds == 0 {
            return None;
        }
        let written = entries
            .iter()
            .filter(|entry| entry.plan.will_write())
            .count();
        let moved = self.moved.min(written);
        Some(format!(
            "シフト {seconds} 秒: 範囲内 {moved} 枚 / 範囲外 {} 枚（{}）",
            written - moved,
            self.scope_label()
        ))
    }

    fn shift_seconds(&self) -> Option<i64> {
        self.shift.total_seconds()
    }

    /// Fill the controls with a sample camera, for the screenshot renderer.
    ///
    /// No file is read: the entries are the sample photo block and the camera is
    /// invented, so the frame shows the controls without pretending a camera was
    /// resolved from a JPEG.
    pub(super) fn show_sample(&mut self, entries: &[PhotoEntry]) {
        self.kind = ScopeKind::ByCamera;
        self.shift = PhotoTimeShift::from_seconds(900);
        let photos: BTreeSet<PathBuf> = entries
            .iter()
            .map(|entry| entry.plan.photo.clone())
            .collect();
        self.resolved = Some(ResolvedCamera {
            model: CameraModel {
                make: Some(String::from("HOLUX")),
                model: Some(String::from("M-241")),
            },
            photos,
        });
        self.moved = entries
            .iter()
            .filter(|entry| entry.plan.will_write())
            .count();
        self.status = None;
        self.pending = false;
        self.plan_stale = false;
    }

    /// Draw the shift and scope controls.
    ///
    /// Returns `true` when a value settled and the plans should be rebuilt: a
    /// scope button, or a shift field that lost the pointer or its focus.
    /// Re-planning reads every JPEG, so dragging a field does not re-plan on
    /// each pointer step.
    pub(super) fn ui(&mut self, ui: &mut Ui, entries: &[PhotoEntry]) -> bool {
        let mut settled = false;
        // One step, not `S`: the list above already ends with a row's own
        // spacing, and the rail's viewport at 1280×800 has room for exactly this
        // much before 集計's heading.
        ui.add_space(space::XS);
        // Two rows of two fields, not one row of four: a 220pt rail cannot hold
        // four labelled `DragValue`s once a value reaches three digits, and a
        // wrapped fourth field reads as a mistake. The pairs keep the labels
        // attached and stay on one line for any value in the range.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.x = 26.0;
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(
                RichText::new("シフト")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            for (label, field) in [("日", &mut self.shift.days), ("時", &mut self.shift.hours)] {
                let (changed, field_settled) = shift_field(ui, label, field);
                self.pending |= changed;
                settled |= field_settled;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.x = 26.0;
            ui.spacing_mut().item_spacing.x = 2.0;
            // Align the second pair under the first: the caption's width plus one
            // field's spacing, as a sum of scale tokens (a raw 30 is a sixth
            // spacing step the design gate rightly refuses).
            ui.add_space(space::L + space::L);
            for (label, field) in [
                ("分", &mut self.shift.minutes),
                ("秒", &mut self.shift.seconds),
            ] {
                let (changed, field_settled) = shift_field(ui, label, field);
                self.pending |= changed;
                settled |= field_settled;
            }
        });
        // The scope decides *which* clocks move; `byCamera` also needs its rule
        // next to the choice, because "same camera" is exact Make/Model equality.
        ui.horizontal_wrapped(|ui| {
            // Tighter than the default: three scope buttons plus the 範囲 label
            // must stay on one line in the rail, including when the scrollbar is
            // showing.
            ui.spacing_mut().item_spacing.x = 2.0;
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
                    self.pending = true;
                    settled = true;
                }
            }
        });
        if let Some(rule) = self.rule() {
            note(ui, &rule);
        }
        match self.shift_seconds() {
            Some(seconds) if seconds != 0 => {
                let written = entries
                    .iter()
                    .filter(|entry| entry.plan.will_write())
                    .count();
                let moved = self.moved.min(written);
                note(
                    ui,
                    &format!(
                        "合計 {seconds} 秒: 範囲内 {moved} 枚 / 範囲外 {} 枚",
                        written - moved
                    ),
                );
            }
            Some(_zero) => {}
            None => common::status_line(
                ui,
                Status::Err,
                "シフト値が大きすぎて秒に直せません。小さい値にしてください",
            ),
        }

        if let Some((status, message)) = &self.status {
            common::status_line(ui, *status, message);
        }
        settled
    }

    /// The sentence under the scope buttons: which photos this choice moves.
    fn rule(&self) -> Option<String> {
        match self.kind {
            ScopeKind::All => None,
            ScopeKind::Selected => Some(format!(
                "一覧でチェックした {} 枚だけシフトします",
                self.selected.len()
            )),
            ScopeKind::ByCamera => Some(self.resolved.as_ref().map_or_else(
                || String::from("Make/Model の完全一致。基準写真を一覧で選んでください（未検証）"),
                |resolved| format!("基準カメラ: {}（未検証）", resolved.label()),
            )),
        }
    }
}

/// One labelled shift field: `(changed, settled)`.
///
/// `settled` is when the plan should be rebuilt. A pointer drag and a typed value
/// end at different moments; both end with either the button released, focus
/// elsewhere, or the value changed outside a text-edit focus.
fn shift_field(ui: &mut Ui, label: &str, field: &mut i64) -> (bool, bool) {
    ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
    let pointer_down = ui.input(|input| input.pointer.any_down());
    let response = ui.add(
        egui::DragValue::new(field)
            .speed(1.0)
            .range(-999_i64..=999_i64),
    );
    let settled = response.drag_stopped()
        || response.lost_focus()
        || (response.changed() && !pointer_down && !response.has_focus());
    (response.changed(), settled)
}

/// A footnote-weight line, the voice every other caption on the screen uses.
fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(size::CAPTION).color(theme::MUTE));
}
