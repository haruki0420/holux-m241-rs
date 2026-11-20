//! ファーム — the Firmware screen (DESIGN.md §5).
//!
//! This is the screen the whole product exists for, and the only one that can end a
//! device. Three steps, in order, with the middle one happening on the PC and the
//! last one being the only destructive act in the product.
//!
//! # Where the amber is, and why exactly there
//!
//! DESIGN.md §3 reserves amber for three things and allows it nowhere else. They are
//! enumerated once, in [`Destructive::voice`], as pairs of a colour and the words
//! that go with it — so a use site cannot take the colour without the label, and the
//! inventory is a list a reviewer can count. The three are:
//!
//! 1. the button that writes to the device;
//! 2. the panel of the step that writes;
//! 3. the state "the device's state is undefined".
//!
//! # What actually happens today
//!
//! Steps 1 and 3 need the device; the session layer is not written and
//! [`m241_core::protocol::DownloadAgent`] refuses by design until the vendor
//! binary has been disassembled. Step 2 is real work on a file and is implemented:
//! it reads an image from disk, plans the patch with `m241_core::firmware`, shows the
//! byte-level difference, and writes the patched image beside the original. The
//! write button is present so the confirmation gate can be exercised and reviewed;
//! pressing it reaches the download agent's own refusal rather than a silent nothing.

use egui::{Color32, RichText, Sense, Stroke, Ui, Vec2};
use m241_core::firmware::{self, FirmwareImage, PROFILE_V1_13};
use m241_core::protocol::DownloadAgent;
use m241_core::{PatchPlan, VERSION};
use std::path::Path;

use crate::screens::common::{self, Status};
use crate::theme::{self, metric, size, space};

/// The three steps the wizard walks through (DESIGN.md §5, ファーム).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    /// Read the firmware off the device. Read-only.
    #[default]
    Dump,
    /// Rewrite the epoch constant in a file on the PC. No device involved.
    Patch,
    /// Write the patched image back. The one destructive act.
    Flash,
}

impl Step {
    /// Every step, in order, for the step row.
    pub const ALL: [Self; 3] = [Self::Dump, Self::Patch, Self::Flash];

    /// The label under the step row.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dump => "吸い出し",
            Self::Patch => "書き換え",
            Self::Flash => "書き戻し",
        }
    }

    /// Position in the row.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Dump => 0,
            Self::Patch => 1,
            Self::Flash => 2,
        }
    }

    /// The step after this one, or `None` on the last.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Dump => Some(Self::Patch),
            Self::Patch => Some(Self::Flash),
            Self::Flash => None,
        }
    }
}

/// The three places amber may appear (DESIGN.md §3).
///
/// One variant per permitted use, and [`Self::voice`] is the only function that
/// names [`theme::DANGER`]. That is what makes "amber appears exactly three times"
/// a property of the code rather than of a review: adding a fourth use means
/// adding a fourth variant, which is a change a reviewer sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destructive {
    /// The button that writes to the device.
    ConfirmButton,
    /// The panel that holds the write step.
    WriteStep,
    /// The device's state is not known.
    UndefinedState,
}

impl Destructive {
    /// The amber and the words that must accompany it.
    #[must_use]
    fn voice(self) -> (Color32, &'static str) {
        match self {
            Self::ConfirmButton => (theme::DANGER, "書き込む"),
            Self::WriteStep => (theme::DANGER, "書き込み"),
            Self::UndefinedState => (theme::DANGER, "端末の状態が不明"),
        }
    }
}

/// What the last dump attempt produced.
///
/// A dump is not one action but a session with the device, and the design gives it a
/// state machine of its own (§5): read once, ask for a power cycle, read again. The
/// variants below are that sequence, so the screen can say which half it is in
/// instead of showing one indeterminate bar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DumpState {
    /// Nothing has been read.
    #[default]
    Idle,
    /// The first pass is running: `read` of `total` bytes.
    FirstPass { read: usize, total: usize },
    /// The first pass finished; the unit must be power-cycled before the second.
    ///
    /// This is a state, not a message: the screen blocks the next action until the
    /// user says the unit is back, because a second read from a running unit is what
    /// makes the two-hash comparison meaningless.
    WaitingForPowerCycle,
    /// Both passes finished and their digests matched.
    Verified { bytes: usize, digest: String },
    /// Both passes finished and the digests differed.
    Mismatch { first: String, second: String },
    /// The read was refused or failed. The device's words are kept verbatim.
    Failed(String),
}

/// The Firmware screen's own state.
#[derive(Debug)]
pub struct FirmwareScreen {
    step: Step,
    dump: DumpState,
    /// Path the dump would be written to.
    dump_path: String,
    /// The image currently loaded from disk (or injected), if any.
    image: Option<Vec<u8>>,
    /// Path the image was read from, for the figure rows.
    source: String,
    /// The plan for [`Self::image`], or the reason there is none.
    plan: Option<PatchPlan>,
    /// The patched bytes, once step 2 has written them.
    patched: Option<Vec<u8>>,
    /// Where the patched image was written.
    patched_path: String,
    /// The typed confirmation. Compared exactly to [`Self::CONFIRMATION`].
    confirmation: String,
    /// Whether the device's state is unknown.
    undefined: bool,
    /// The last thing the screen said, and how it should be read.
    message: Option<(Status, String)>,
    /// Show sample data, so the layout can be reviewed without a device.
    sample: bool,
}

impl Default for FirmwareScreen {
    fn default() -> Self {
        Self {
            step: Step::Dump,
            dump: DumpState::Idle,
            dump_path: String::from("backup/stock.bin"),
            image: None,
            source: String::from("backup/stock.bin"),
            plan: None,
            patched: None,
            patched_path: String::new(),
            confirmation: String::new(),
            undefined: false,
            message: None,
            sample: false,
        }
    }
}

impl FirmwareScreen {
    /// The word the user must type before the write button unlocks.
    ///
    /// A confirmation dialogue is clicked by reflex; typing is not (PRODUCT.md, and
    /// AGENTS.md §2.2). The same word the CLI demands.
    pub const CONFIRMATION: &'static str = "WRITE";

    /// The step the wizard is on.
    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    /// Move to a step. Public so a test can drive a step without a click, and so the
    /// session layer can advance the wizard when its own work finishes.
    pub fn set_step(&mut self, step: Step) {
        self.step = step;
    }

    /// The dump's state.
    #[must_use]
    pub fn dump_state(&self) -> &DumpState {
        &self.dump
    }

    /// Put the screen in a specific dump state.
    ///
    /// The wizard's states are driven by the read that does not exist yet, so the
    /// state is settable from outside: that is what lets the render tests walk every
    /// state, including the two that only occur after a failure.
    pub fn set_dump_state(&mut self, state: DumpState) {
        self.dump = state;
    }

    /// Declare the device's state unknown, which arms the amber recovery panel.
    pub fn set_undefined(&mut self, undefined: bool) {
        self.undefined = undefined;
    }

    /// Fill the screen with the sample image and its real patch plan.
    ///
    /// Public for the headless render tests and for a future `--demo` flag.
    ///
    /// # Errors
    ///
    /// The planner's refusal, if the synthesised image is ever no longer recognised.
    pub fn show_sample(&mut self) -> Result<(), String> {
        self.sample = true;
        self.load_sample()
    }

    /// Report dump progress to the screen. Called by whatever runs the read.
    ///
    /// `read` and `total` are bytes; the progress bar is indeterminate when `total`
    /// is zero, which is what the screen shows before the device has said how big
    /// the image is.
    pub fn set_dump_progress(&mut self, read: usize, total: usize) {
        self.dump = DumpState::FirstPass { read, total };
    }

    /// The plan the screen is showing, if a load succeeded.
    #[must_use]
    pub fn plan(&self) -> Option<&PatchPlan> {
        self.plan.as_ref()
    }

    /// Whether the write button would be enabled, i.e. every gate is satisfied.
    ///
    /// Separated from the drawing so the gate can be tested without a click: a
    /// safety condition that only exists inside a widget closure is a condition
    /// nobody can assert on.
    #[must_use]
    pub fn write_armed(&self) -> bool {
        self.confirmation.trim() == Self::CONFIRMATION && self.patched.is_some() && !self.undefined
    }

    /// Load an image that is already in memory, plan the patch, and show step 2.
    ///
    /// # Errors
    ///
    /// The reasons `m241_core::firmware::plan` refuses: an unknown profile, a size
    /// that is not a known release, an epoch constant that is not the stock value.
    /// Each is displayed with its offset, because that is what makes it actionable.
    pub fn load_image(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let image = FirmwareImage::new(&bytes).map_err(|err| err.to_string())?;
        let plan = firmware::plan(&image).map_err(|err| err.to_string())?;
        self.plan = Some(plan);
        self.image = Some(bytes);
        self.patched = None;
        self.patched_path.clear();
        self.confirmation.clear();
        self.step = Step::Patch;
        Ok(())
    }

    /// Read an image from disk and plan it.
    ///
    /// Refuses files over [`Self::MAX_IMAGE_BYTES`]: a mistyped path can otherwise
    /// point at a multi-gigabyte file and this screen would read all of it into
    /// memory before noticing that it is not a firmware image.
    fn open(&mut self, path: &str) -> Result<(), String> {
        let path = path.trim();
        if path.is_empty() {
            return Err(String::from("パスを入力してください"));
        }
        let metadata =
            std::fs::metadata(path).map_err(|err| format!("{path} を読めません: {err}"))?;
        let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if length > Self::MAX_IMAGE_BYTES {
            return Err(format!(
                "{path} は {} バイトあります。ファームのイメージは 400 KiB 程度です",
                common::grouped(length)
            ));
        }
        let bytes = std::fs::read(path).map_err(|err| format!("{path} を読めません: {err}"))?;
        self.source = String::from(path);
        self.load_image(bytes)
    }

    /// Apply the plan and write the patched image beside the source.
    ///
    /// The offset in the default name keeps the backup intact: the product never
    /// overwrites the file it read, because that file is the only way back.
    fn write_patched(&mut self) -> Result<(), String> {
        let (Some(plan), Some(image)) = (self.plan.as_ref(), self.image.as_ref()) else {
            return Err(String::from("先にイメージを読み込んでください"));
        };
        let patched = plan.apply(image).map_err(|err| err.to_string())?;
        if self.patched_path.trim().is_empty() {
            self.patched_path = default_patched_name(&self.source);
        }
        std::fs::write(self.patched_path.trim(), &patched)
            .map_err(|err| format!("{} に書けません: {err}", self.patched_path.trim()))?;
        self.patched = Some(patched);
        Ok(())
    }

    /// Attempt the write.
    ///
    /// Returns the download agent's refusal. It is called rather than short-circuited
    /// on purpose: the button reaching a real guard, which names its own reason, is
    /// what stops this screen from one day growing a "well, try it anyway" path.
    fn attempt_flash(&mut self) -> String {
        let refusal = DownloadAgent::unsupported()
            .err()
            .map(|err| err.to_string())
            .unwrap_or_else(|| String::from("拒否されましたが理由がありません"));
        self.undefined = false;
        refusal
    }

    /// Draw the screen body. The rail is drawn by the shell, not here.
    pub fn ui(&mut self, ui: &mut Ui) {
        // The actions are pinned. At 1024×640 the wizard's own body is taller than
        // the window; the review found the checklist cut in half and 書き込む below
        // the bottom edge, and an action that cannot be reached is not a disabled
        // action, it is a missing one.
        common::footer(ui, "m241-firmware-actions", |ui| self.action_bar(ui));

        common::scrolling(ui, "m241-firmware-body", |ui| {
            common::heading(
                ui,
                "ファームを書き換える",
                "本体のファームを吸い出し、日付定数を書き換えて書き戻します。",
            );
            ui.add_space(space::L);
            self.step_row(ui);
            ui.add_space(space::L);

            if self.undefined {
                self.undefined_panel(ui);
                ui.add_space(space::L);
            }

            match self.step {
                Step::Dump => self.dump_step(ui),
                Step::Patch => self.patch_step(ui),
                Step::Flash => self.flash_step(ui),
            }

            if let Some((status, text)) = &self.message {
                ui.add_space(space::M);
                common::status_line(ui, *status, text);
            }

            ui.add_space(space::L);
            self.sample_toggle(ui);
        });
    }

    /// 中止 / 続行 on the first two steps, 中止 / 書き込む on the third.
    ///
    /// The footer form of the old `step_nav` and of the write row: the same
    /// controls, moved out of the scrolling body so they are on screen at every
    /// window height.
    fn action_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.step == Step::Flash {
                    let response = self.write_button(ui);
                    if response.clicked() {
                        let refusal = self.attempt_flash();
                        self.message = Some((Status::Err, refusal));
                    }
                } else if let Some(next) = self.step.next()
                    && ui
                        .add(
                            egui::Button::new(RichText::new("続行").size(size::UI))
                                .corner_radius(theme::RADIUS)
                                .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                        )
                        .clicked()
                {
                    self.step = next;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("中止").size(size::UI))
                            .corner_radius(theme::RADIUS)
                            .min_size(Vec2::new(metric::ACTION_WIDTH * 0.7, 0.0)),
                    )
                    .clicked()
                {
                    self.step = Step::Dump;
                    self.confirmation.clear();
                    self.undefined = false;
                }
            });
        });
    }

    /// 1 吸い出し 2 書き換え 3 書き戻し, drawn as a rail of its own.
    ///
    /// The step row is the only place a horizontal transition is allowed
    /// (DESIGN.md §3, motion thesis): it says where you are in a sequence. Ink for
    /// the current step, data for a finished one, rule for one not started — never
    /// amber, which is not a progress colour.
    fn step_row(&self, ui: &mut Ui) {
        let width = ui.available_width();
        // Two rows tall: the dots sit on the line and the labels hang below it, so a
        // one-row allocation clips exactly the words that say which step is which.
        let (rect, _response) =
            ui.allocate_exact_size(Vec2::new(width, metric::ROW * 2.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let padding = space::L;
        let count = Step::ALL.len();
        let span = rect.width() - padding * 2.0;
        // The line sits one row down, leaving the second row for the labels.
        let line_y = rect.top() + metric::ROW;
        let step_width = span / (count - 1) as f32;

        let positions: Vec<egui::Pos2> = (0..count)
            .map(|index| egui::pos2(rect.left() + padding + step_width * index as f32, line_y))
            .collect();
        if let (Some(first), Some(last)) = (positions.first(), positions.last()) {
            painter.line_segment([*first, *last], Stroke::new(metric::HAIRLINE, theme::RULE));
        }

        for step in Step::ALL {
            let Some(center) = positions.get(step.index()).copied() else {
                continue;
            };
            let reached = step.index() <= self.step.index();
            let colour = if step == self.step {
                theme::INK
            } else if reached {
                theme::DATA
            } else {
                theme::RULE
            };
            if step == self.step {
                painter.circle_filled(center, metric::POINT_DOT + 1.0, colour);
            } else {
                painter.circle_stroke(
                    center,
                    metric::POINT_DOT,
                    Stroke::new(metric::HAIRLINE, colour),
                );
            }
            painter.text(
                center + Vec2::new(0.0, space::M),
                egui::Align2::CENTER_TOP,
                format!("{} {}", step.index() + 1, step.label()),
                egui::FontId::proportional(size::CAPTION),
                if step == self.step {
                    theme::INK
                } else {
                    theme::MUTE
                },
            );
        }
    }

    /// Step 1: read the image off the device. Read-only, and not yet implemented.
    fn dump_step(&mut self, ui: &mut Ui) {
        common::panel(ui, "吸い出し", |ui| {
            ui.label(
                RichText::new("読み取りのみ。本体は書き換えません。")
                    .size(size::UI)
                    .color(theme::MUTE),
            );
            ui.add_space(space::M);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("保存先")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.dump_path)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - metric::ACTION_WIDTH - space::L),
                );
                // A native file dialog needs `rfd`, a dependency this build does not
                // carry; a button that does nothing would be worse than its absence.
                let _disabled = ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new("参照").size(size::UI))
                        .corner_radius(theme::RADIUS)
                        .min_size(Vec2::new(metric::ACTION_WIDTH * 0.6, 0.0)),
                );
            });

            ui.add_space(space::M);
            match &self.dump {
                DumpState::Idle => {
                    let _bar = ui.add(
                        egui::ProgressBar::new(0.0)
                            .desired_width(ui.available_width())
                            .corner_radius(theme::RADIUS)
                            .text(RichText::new("まだ読み取っていません").size(size::CAPTION)),
                    );
                }
                DumpState::FirstPass { read, total } => {
                    let fraction = if *total == 0 {
                        0.0
                    } else {
                        (*read as f32 / *total as f32).clamp(0.0, 1.0)
                    };
                    let _bar = ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(ui.available_width())
                            .corner_radius(theme::RADIUS)
                            .animate(false)
                            .text(
                                RichText::new(format!(
                                    "{} / {} バイト",
                                    common::grouped(*read),
                                    common::grouped(*total)
                                ))
                                .size(size::CAPTION),
                            ),
                    );
                }
                DumpState::WaitingForPowerCycle => {
                    // DESIGN.md §6: the instruction is an imperative, inline on the
                    // step, and never a modal.
                    common::status_line(ui, Status::Waiting, "端末の操作待ちです");
                    ui.add_space(space::S);
                    ui.label(
                        RichText::new("1 回目の読み取りが終わりました。")
                            .size(size::UI)
                            .color(theme::INK),
                    );
                    ui.label(
                        RichText::new(
                            "電池を抜いて 10 秒待ち、入れ直してから続行してください。\
                             2 回目は電源を入れ直した状態から読みます。",
                        )
                        .size(size::UI)
                        .color(theme::INK),
                    );
                }
                DumpState::Verified { bytes, digest } => {
                    common::status_line(ui, Status::Ok, "2 回の読み取りが一致しました");
                    ui.add_space(space::S);
                    common::figure_line(ui, "サイズ", &common::bytes(*bytes));
                    common::figure_line(ui, "SHA-256", digest);
                }
                DumpState::Mismatch { first, second } => {
                    common::status_line(ui, Status::Err, "2 回の読み取りが一致しません");
                    ui.add_space(space::S);
                    common::figure_line(ui, "1 回目", first);
                    common::figure_line(ui, "2 回目", second);
                    ui.label(
                        RichText::new("このイメージは使いません。読み直してください。")
                            .size(size::UI)
                            .color(theme::INK),
                    );
                }
                DumpState::Failed(reason) => {
                    // The device's own words first, our hint second (§6).
                    common::status_line(ui, Status::Err, reason);
                }
            }

            ui.add_space(space::M);
            ui.horizontal(|ui| {
                let _ignored = ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new("吸い出す").size(size::UI))
                        .corner_radius(theme::RADIUS)
                        .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                );
                ui.label(
                    RichText::new("未検証 — 端末の読み出しはセッション層の実装後です")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
            ui.add_space(space::S);
            ui.label(
                RichText::new(
                    "すでに `m241 fw dump` で吸い出したファイルがある場合は、\
                      次へ進んでそのパスを開いてください。",
                )
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
        });
    }

    /// Step 2: rewrite the constant in a file. Real work, no device.
    fn patch_step(&mut self, ui: &mut Ui) {
        common::panel(ui, "書き換え", |ui| {
            ui.label(
                RichText::new("PC の中のファイルだけを書き換えます。本体には触れません。")
                    .size(size::UI)
                    .color(theme::MUTE),
            );
            ui.add_space(space::M);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("イメージ")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.source)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - metric::ACTION_WIDTH - space::L),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("開く").size(size::UI))
                            .corner_radius(theme::RADIUS)
                            .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                    )
                    .clicked()
                {
                    let source = self.source.clone();
                    self.message = Some(match self.open(&source) {
                        Ok(()) => (Status::Ok, format!("{} を読み込みました", self.source)),
                        Err(reason) => (Status::Err, reason),
                    });
                }
            });

            ui.add_space(space::M);
            self.plan_panel(ui);
        });
    }

    /// The 差分 block: what the plan would change, byte by byte.
    fn plan_panel(&mut self, ui: &mut Ui) {
        let Some(plan) = self.plan.clone() else {
            common::status_line(ui, Status::Info, "イメージを読み込むと差分が出ます");
            return;
        };

        // A legal, expected outcome: the unit has already been patched. The plan
        // carries the reason so the screen can print it instead of "0 bytes" and
        // leave the reader wondering whether the tool failed.
        if !plan.changes_anything() {
            common::status_line(ui, Status::Ok, "このイメージは書き換え不要です");
            ui.add_space(space::S);
            let reason = plan
                .no_op_reason
                .clone()
                .unwrap_or_else(|| String::from("差分がありません"));
            ui.label(RichText::new(reason).size(size::UI).color(theme::INK));
            self.patched = None;
            return;
        }

        let offsets: Vec<String> = plan
            .edits
            .iter()
            .map(|edit| format!("0x{:05X}", edit.offset))
            .collect();
        ui.label(
            RichText::new(format!(
                "差分 {} バイト（{}）",
                plan.edits.len(),
                offsets.join(", ")
            ))
            .size(size::UI)
            .color(theme::INK),
        );
        ui.add_space(space::S);

        // Ruled table: header in caption, figures in mono, a hairline per row. The
        // offsets must align, which is why this column is monospace and right-aligned.
        ui.label(
            RichText::new("オフセット          変更前  変更後")
                .size(size::CAPTION)
                .color(theme::MUTE)
                .monospace(),
        );
        common::hairline(ui);
        for edit in &plan.edits {
            ui.label(
                RichText::new(format!(
                    "0x{:05X}          0x{:02X}    0x{:02X}",
                    edit.offset, edit.before, edit.after
                ))
                .size(size::UI)
                .color(theme::INK)
                .monospace(),
            );
            common::hairline(ui);
        }

        if let Some(profile) = plan.profile.as_ref() {
            ui.add_space(space::S);
            common::figure_line(ui, "プロファイル", &profile.to_string());
        }
        ui.add_space(space::S);
        common::figure_line(
            ui,
            "ずれの量",
            &m241_core::firmware::patch::shift_description(),
        );

        ui.add_space(space::M);
        if self.patched.is_some() {
            common::status_line(
                ui,
                Status::Ok,
                &format!(
                    "{} に書き換え済みのイメージを保存しました",
                    self.patched_path
                ),
            );
        } else {
            let _ignored = ui.add(
                egui::TextEdit::singleline(&mut self.patched_path)
                    .hint_text(RichText::new("保存先（空なら元ファイルの隣）").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(space::S);
            if ui
                .add(
                    egui::Button::new(RichText::new("書き換えたイメージを保存").size(size::UI))
                        .corner_radius(theme::RADIUS)
                        .min_size(Vec2::new(metric::ACTION_WIDTH * 2.0, 0.0)),
                )
                .clicked()
            {
                self.message = Some(match self.write_patched() {
                    Ok(()) => (Status::Ok, format!("{} に保存しました", self.patched_path)),
                    Err(reason) => (Status::Err, reason),
                });
            }
        }
    }

    /// Step 3: write to the device. The only destructive act in the product.
    fn flash_step(&mut self, ui: &mut Ui) {
        let (_border, title) = Destructive::WriteStep.voice();
        common::panel_with_border(ui, title, Destructive::WriteStep.voice().0, |ui| {
            ui.label(
                RichText::new(
                    "本体のファームを上書きします。失敗すると使えなくなる場合があります。",
                )
                .size(size::UI)
                .color(theme::INK),
            );
            ui.add_space(space::M);

            // The figure rows are the review a person does before typing WRITE:
            // which file, which size, which digest, which bytes differ.
            let file = file_label(&self.patched_path);
            let size = self
                .patched
                .as_ref()
                .map(|bytes| common::bytes(bytes.len()))
                .unwrap_or_else(|| String::from("—"));
            let digest = self
                .patched
                .as_ref()
                .map(|bytes| {
                    format!(
                        "{}…",
                        m241_core::firmware::sha256_hex(bytes)
                            .chars()
                            .take(8)
                            .collect::<String>()
                    )
                })
                .unwrap_or_else(|| String::from("—"));
            common::figure_line(ui, "firmware", &format!("{file}  {size}  {digest}"));
            table_row(ui, "DA", "未取得", "未検証 — 端末から読んでいません");
            let diff = self.plan.as_ref().map_or_else(
                || String::from("—"),
                |plan| format!("{} バイト", plan.edits.len()),
            );
            common::figure_line(ui, "差分", &diff);

            ui.add_space(space::M);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("確認のため {} と入力", Self::CONFIRMATION))
                        .size(size::UI)
                        .color(theme::INK),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.confirmation)
                        .hint_text(Self::CONFIRMATION)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(metric::ACTION_WIDTH * 2.0),
                );
            });

            ui.add_space(space::S);
            self.gate_rows(ui);
        });
    }

    /// The write button, the only amber-filled control in the product.
    ///
    /// Both states are readable, and amber stays on the surface and the frame.
    /// The review measured the old button at 2.2–2.4:1: `ink` on the amber fill
    /// is 1.8:1 by itself, and `add_enabled(false, …)` then multiplies both fill
    /// and label by `disabled_alpha`, which turned a bad pair into an unreadable
    /// one. So the label colour is chosen from the surface it sits on — `ground`
    /// on the amber face (7.9:1), `ink` on the dark face (14.3:1) — and the
    /// disabled dimming is overridden for this one control so the pair survives
    /// with the ratio it was designed for. The button is still non-interactive
    /// and still reported as disabled to assistive technology.
    fn write_button(&mut self, ui: &mut Ui) -> egui::Response {
        let armed = self.write_armed();
        let (amber, label) = Destructive::ConfirmButton.voice();
        let (fill, ink) = confirm_button_colours(amber, armed);
        let button = egui::Button::new(RichText::new(label).size(size::UI).color(ink))
            .fill(fill)
            .stroke(Stroke::new(metric::HAIRLINE, amber))
            .corner_radius(theme::RADIUS)
            .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0));
        ui.scope(|ui| {
            // The pair above already states "locked" (dark face, ink text); egui's
            // 0.5 opacity on top would dim the label toward the dark face and take
            // it back under 4.5:1, which is the defect being fixed.
            ui.style_mut().visuals.disabled_alpha = 1.0;
            ui.add_enabled(armed, button)
        })
        .inner
    }

    /// The prerequisites of the write, each with its state in words.
    ///
    /// Kept visible while disabled: a greyed button with no explanation is the
    /// interface equivalent of "no".
    fn gate_rows(&self, ui: &mut Ui) {
        let backup = self.image.is_some();
        let patched = self.patched.is_some();
        for (held, text) in [
            (backup, "元のイメージを読み込んである"),
            (
                patched,
                "書き換えたイメージを保存してある（バックアップは残っています）",
            ),
            (
                self.confirmation.trim() == Self::CONFIRMATION,
                "WRITE と入力してある",
            ),
        ] {
            common::status_line(ui, if held { Status::Ok } else { Status::Todo }, text);
        }
    }

    /// The undefined-device state (DESIGN.md §6).
    ///
    /// Amber, because it is the state where the user must not guess: the unit is in
    /// an unknown state and the only safe move is to re-flash a known image. The
    /// recovery command is copyable text, not a button, because it is meant to be
    /// pasted into a terminal.
    fn undefined_panel(&mut self, ui: &mut Ui) {
        let (_border, title) = Destructive::UndefinedState.voice();
        common::panel_with_border(ui, title, Destructive::UndefinedState.voice().0, |ui| {
            ui.label(
                RichText::new("書き込みが途中で止まりました。端末は応答しない可能性があります。")
                    .size(size::UI)
                    .color(theme::INK),
            );
            ui.add_space(space::S);
            let command = recovery_command(&self.source, &self.patched_path);
            ui.label(
                RichText::new("次のコマンドで、吸い出した元のイメージに戻せます。")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&command)
                        .size(size::UI)
                        .color(theme::INK)
                        .monospace(),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("コピー").size(size::CAPTION))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(command.clone());
                }
            });
        });
    }

    /// The sample switch, last on the screen so it never sits where a real action
    /// would be.
    fn sample_toggle(&mut self, ui: &mut Ui) {
        ui.checkbox(
            &mut self.sample,
            RichText::new("見本の値を表示（実機から読んだ値ではありません）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        if !self.sample {
            return;
        }
        common::sample_marker(ui);
        // The sample runs the real planner against a synthesised image, so what is
        // drawn is the real diff table rather than a hand-written one. If the planner
        // changes, the sample changes with it.
        if self.plan.is_none()
            && let Ok(()) = self.load_sample()
        {
            self.step = Step::Patch;
        }
    }

    /// Load a synthetic stock V1.13 image and plan it.
    fn load_sample(&mut self) -> Result<(), String> {
        let bytes = sample_image();
        self.source = String::from("backup/stock.bin（見本）");
        self.dump_path = String::from("backup/stock.bin");
        self.load_image(bytes)
    }

    /// The largest file this screen will read as a firmware image.
    ///
    /// Twice the largest known release: enough for a padded dump, small enough that
    /// a mistyped path cannot exhaust memory.
    const MAX_IMAGE_BYTES: usize = 1 << 20;
}

/// A synthesised stock V1.13 image, for the sample.
///
/// Not a copy of the vendor binary: the header fields, the version marker and the two
/// epoch constants are written at the offsets `firmware::image` publishes, and everything
/// else is erased `0xFF`. That is enough for the real identifier and planner to accept
/// it, which is the point — the sample exercises the shipping code path.
fn sample_image() -> Vec<u8> {
    use m241_core::firmware::image::{
        BUILD_NUMBER_OFFSET, DEVICE_TYPE_OFFSET, FAMILY, FAMILY_OFFSET, FW_SIZE_OFFSET,
    };
    let profile = PROFILE_V1_13;
    let mut data = vec![0xFF_u8; profile.size];
    if let Some(field) = data.get_mut(FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()) {
        field.copy_from_slice(FAMILY.as_bytes());
    }
    // A header field is a C string: the bytes after it are NUL, not 0xFF.
    if let Some(field) = data.get_mut(DEVICE_TYPE_OFFSET..) {
        field.fill(0);
    }
    let device = profile.device_type.as_bytes();
    if let Some(field) = data.get_mut(DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()) {
        field.copy_from_slice(device);
    }
    let marker = profile.name.as_bytes();
    if let Some(field) = data.get_mut(profile.version_marker..profile.version_marker + marker.len())
    {
        field.copy_from_slice(marker);
    }
    let stock = m241_core::firmware::image::GPS_EPOCH_UNIX.to_le_bytes();
    for offset in m241_core::firmware::image::EPOCH_CONSTANT_OFFSETS {
        if let Some(field) = data.get_mut(offset..offset + stock.len()) {
            field.copy_from_slice(&stock);
        }
    }
    let declared = u32::try_from(profile.size).unwrap_or(0).to_le_bytes();
    if let Some(field) = data.get_mut(FW_SIZE_OFFSET..FW_SIZE_OFFSET + declared.len()) {
        field.copy_from_slice(&declared);
    }
    if let Some(field) = data.get_mut(BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2) {
        field.copy_from_slice(&67_u16.to_le_bytes());
    }
    data
}

/// `stock.bin` → `stock.fixed.bin`.
fn default_patched_name(source: &str) -> String {
    let path = Path::new(source.trim());
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("firmware"));
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("bin"));
    let name = format!("{stem}.fixed.{extension}");
    path.parent()
        .map(|parent| parent.join(&name).to_string_lossy().into_owned())
        .unwrap_or(name)
}

/// The file name, without its directory, for a figure row.
fn file_label(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::from("—");
    }
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from(path))
}

/// The command that restores a unit from its own dump (AGENTS.md §12).
fn recovery_command(source: &str, patched: &str) -> String {
    let backup = if source.trim().is_empty() {
        "backup/stock.bin"
    } else {
        source.trim()
    };
    let image = if patched.trim().is_empty() {
        "fixed.bin"
    } else {
        patched.trim()
    };
    format!("m241 fw flash PORT --da DA_v3.8.4.bin --fw {image} --backup {backup} --yes")
}

/// The write button's (surface, label) pair for its two states.
///
/// Amber is the surface and the frame; the label is `ground` on the amber face
/// (7.9:1) and `ink` on the dark face (14.3:1). One function for both states so
/// the pair cannot be half-changed: the review found the two ratios 1.8:1 and
/// 2.2:1, which is what happens when a fill and a label are chosen apart.
#[must_use]
fn confirm_button_colours(amber: Color32, armed: bool) -> (Color32, Color32) {
    if armed {
        (amber, theme::GROUND)
    } else {
        (theme::GROUND, theme::INK)
    }
}

/// A label/value row that takes a whole line, for a long figure.
fn table_row(ui: &mut Ui, label: &str, value: &str, note: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
        ui.add_space(space::S);
        ui.label(
            RichText::new(value)
                .size(size::UI)
                .color(theme::INK)
                .monospace(),
        );
        ui.add_space(space::S);
        ui.label(RichText::new(note).size(size::CAPTION).color(theme::MUTE));
    });
}

/// Version string for the footer figure; kept here so the shell does not have to
/// know the crate version.
#[must_use]
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> FirmwareScreen {
        FirmwareScreen::default()
    }

    /// WCAG relative luminance, so the button's contrast is measured rather than
    /// claimed. Duplicated from `theme`'s test module on purpose: the token tests
    /// assert the palette, this one asserts a pair the palette does not fix.
    fn luminance(colour: Color32) -> f64 {
        let channel = |value: u8| {
            let v = f64::from(value) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(colour.r()) + 0.7152 * channel(colour.g()) + 0.0722 * channel(colour.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn the_wizard_starts_at_the_read_only_step() {
        let screen = screen();
        assert_eq!(screen.step(), Step::Dump);
        assert_eq!(screen.dump_state(), &DumpState::Idle);
    }

    #[test]
    fn the_write_button_label_clears_wcag_aa_in_both_states() {
        // The defect the review found: ink on amber is 1.8:1, and the disabled
        // treatment dimmed both until the label was a lighter patch on a lighter
        // patch. Measured here rather than asserted in a comment, because the two
        // colours are chosen in different branches and only the pair has a ratio.
        let (amber, _label) = Destructive::ConfirmButton.voice();
        for armed in [false, true] {
            let (fill, ink) = confirm_button_colours(amber, armed);
            let ratio = contrast(ink, fill);
            assert!(
                ratio >= 4.5,
                "the {} write button is {ratio:.2}:1, below 4.5:1 (ink {ink:?}, fill {fill:?})",
                if armed { "armed" } else { "locked" }
            );
        }
    }

    #[test]
    fn the_armed_write_button_puts_ground_text_on_amber() {
        // DESIGN.md's answer to "how is amber spent on text": it is not. The face
        // is amber and the word on it is `ground`, which is 7.9:1.
        let (amber, _label) = Destructive::ConfirmButton.voice();
        assert_eq!(confirm_button_colours(amber, true), (amber, theme::GROUND));
    }

    #[test]
    fn amber_has_exactly_three_places_and_each_carries_words() {
        // DESIGN.md §3. The count is the invariant; the words are what keeps amber
        // from being the only signal.
        let voices = [
            Destructive::ConfirmButton.voice(),
            Destructive::WriteStep.voice(),
            Destructive::UndefinedState.voice(),
        ];
        assert_eq!(voices.len(), 3);
        for (colour, words) in voices {
            assert_eq!(colour, theme::DANGER, "a destructive use lost its colour");
            assert!(!words.is_empty(), "an amber use has no label");
        }
    }

    #[test]
    fn the_sample_image_plans_the_three_measured_bytes() {
        // The hardware session changed exactly 0x402F6, 0x402F7 and 0x59C2C
        // (docs/dev/03-hardware-facts.md). The sample runs the shipping planner, so
        // this asserts both the sample and the planner against the measured bytes.
        let mut screen = screen();
        screen
            .load_image(sample_image())
            .expect("the synthetic image must be recognisable");
        let plan = screen.plan().expect("a plan was stored");
        let offsets: Vec<String> = plan
            .edits
            .iter()
            .map(|edit| format!("0x{:05X}", edit.offset))
            .collect();
        assert_eq!(offsets, ["0x402F6", "0x402F7", "0x59C2C"]);
        assert_eq!(plan.edits.len(), 3);
        assert!(plan.changes_anything());
    }

    #[test]
    fn the_patch_changes_the_epoch_constant_to_the_shifted_value() {
        let mut screen = screen();
        screen.load_image(sample_image()).expect("known image");
        let patched = screen
            .plan()
            .and_then(|plan| plan.apply(&sample_image()).ok())
            .expect("the plan applies to the image it was built for");
        let image = FirmwareImage::new(&patched).expect("still an image");
        let effective = image
            .u32_at(m241_core::firmware::image::EFFECTIVE_EPOCH_OFFSET)
            .expect("the constant is inside the image");
        assert_eq!(effective, m241_core::firmware::image::GPS_EPOCH_UNIX_WNRO);
    }

    #[test]
    fn the_sample_image_is_the_size_of_a_known_release() {
        // A sample that is not a real image size would be refused by the planner and
        // the sample toggle would silently do nothing.
        assert_eq!(sample_image().len(), PROFILE_V1_13.size);
    }

    #[test]
    fn write_is_armed_only_when_every_gate_holds() {
        let mut screen = screen();
        assert!(!screen.write_armed(), "a cold screen must not be armed");

        screen.confirmation = String::from("write");
        assert!(!screen.write_armed(), "the word is case sensitive");

        screen.confirmation = String::from("WRITE");
        assert!(
            !screen.write_armed(),
            "the confirmation alone must not arm the write"
        );

        screen.load_image(sample_image()).expect("known image");
        assert!(
            !screen.write_armed(),
            "a plan is not a patched file: nothing has been written yet"
        );
    }

    #[test]
    fn an_undefined_device_state_disarms_the_write() {
        let mut screen = screen();
        screen.confirmation = String::from("WRITE");
        screen.patched = Some(sample_image());
        assert!(screen.write_armed());
        screen.undefined = true;
        assert!(
            !screen.write_armed(),
            "a write must not be offered while the device's state is unknown"
        );
    }

    #[test]
    fn the_file_write_keeps_the_backup_and_names_the_copy() {
        // The source is the only way back; the patched image must land beside it
        // under a different name.
        assert_eq!(
            default_patched_name("backup/stock.bin"),
            "backup/stock.fixed.bin"
        );
        assert_eq!(default_patched_name("stock.bin"), "stock.fixed.bin");
        assert_eq!(default_patched_name("dump"), "dump.fixed.bin");
    }

    #[test]
    fn an_unknown_image_is_refused_with_a_reason() {
        let mut screen = screen();
        let error = screen
            .load_image(vec![0_u8; 1024])
            .expect_err("a kilobyte of zeroes is not a firmware image");
        assert!(!error.is_empty(), "the refusal must say why");
    }

    #[test]
    fn the_recovery_command_names_both_images() {
        let command = recovery_command("backup/stock.bin", "backup/stock.fixed.bin");
        assert!(command.contains("backup/stock.bin"), "got {command}");
        assert!(command.contains("backup/stock.fixed.bin"), "got {command}");
        assert!(command.trim_start().starts_with("m241 "), "got {command}");
    }

    #[test]
    fn the_dump_instruction_is_imperative_and_names_the_wait() {
        // DESIGN.md §6, "Waiting for the user": the instruction is an imperative on
        // the step, not a modal. The two numbers are the ones the hardware session
        // used (pull the battery, wait ten seconds).
        let text = "電池を抜いて 10 秒待ち、入れ直してから続行してください。\
                    2 回目は電源を入れ直した状態から読みます。";
        assert!(text.contains("電池を抜いて"));
        assert!(text.contains("10 秒"));
    }

    #[test]
    fn every_step_has_a_label_and_an_order() {
        assert_eq!(Step::ALL.len(), 3);
        for (index, step) in Step::ALL.iter().enumerate() {
            assert_eq!(step.index(), index);
            assert!(!step.label().is_empty());
        }
    }
}
