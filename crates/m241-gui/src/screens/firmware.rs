//! ファーム — the Firmware screen (DESIGN.md §5).
//!
//! This is the screen the whole product exists for, and the only one that can end a
//! device. Three steps, in order: read the firmware off the unit (read-only),
//! rewrite the epoch constant in a file on the PC, and write the patched image
//! back (the one destructive act).
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
//! # What actually happens now
//!
//! The screen drives the same session the CLI does: [`m241_flash::flow`] reads the
//! firmware, writes the patched range through `DownloadAgent`, restarts the unit,
//! reads the range back and compares it. The GUI does not have a second write
//! path: `tools/check_safety.py` requires `m241-gui` to depend on `m241-flash`, and
//! requires the token [`Confirmation`] to be built only in the screens that check
//! a typed word — this screen for `WRITE`, `screens/connect.rs` for `PROBE` (that
//! action loads the DA into RAM, which can program flash itself). The token is
//! built after the user types [`FirmwareScreen::CONFIRMATION`] in the field below
//! — there is no `--yes` equivalent, and no agent may type it.
//!
//! The token is not proof: [`Confirmation::granted`] takes no argument and cannot
//! know who typed. What refuses an untyped write is the re-check at the top of
//! [`FirmwareScreen::start_write`] (and `ConnectScreen::start_probe_da`), and a
//! unit test pins that the button's disabled state is not the only gate. The
//! checker proves the token exists in the right file, not that a person produced
//! it; `docs/dev/10-review-log.md` records the limit.
//!
//! Nothing on this screen claims the write path has run on hardware. The write
//! step carries the word 未検証, and the success panel says the read-back matched
//! while the hardware verification is still outstanding. That is a fact about the
//! hardware (docs/dev/05-verification.md), not about whether this code compiles.
//!
//! # Why the old gate is gone
//!
//! `m241-gui` used to be forbidden from linking `m241-flash`: the dependency
//! graph was the gate, and no edit inside this crate could bypass it. The user
//! asked for the CLI's write capability in the GUI (2026-11-23, AGENTS.md §0), so
//! the rule was removed deliberately. What replaces it is a textual check in
//! `tools/check_safety.py` — the `flow::flash(` call, the typed-word constants,
//! the absence of a destructive opcode, and where the token/job may be built.
//! That is **weaker** than a missing dependency edge: an edit can weaken a text
//! check. The invariants that still hold structurally are the ones inside
//! `m241-flash`: the fixed range, `check_layout`, `check_image`, `verify_device`
//! and the read-back. `device.rs`'s module doc records the same trade.

use egui::{Color32, RichText, Sense, Stroke, Ui, Vec2};
use m241_core::firmware::{self, FirmwareImage, PROFILE_V1_13};
use m241_core::{PatchPlan, VERSION};
use m241_flash::da::{
    Confirmation, FLASH_WRITE_END, FLASH_WRITE_LENGTH, FLASH_WRITE_START,
    VENDOR_PER_UNIT_DATA_BYTES,
};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

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

/// How one step's marker is drawn: a shape first, a colour second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepMarker {
    /// A finished step: a filled dot.
    Done,
    /// The step the wizard is on: the same dot, larger.
    Current,
    /// A step not started: an open ring.
    Ahead,
}

impl StepMarker {
    /// Which marker a step gets, given the step the wizard is on.
    #[must_use]
    fn of(step: Step, current: Step) -> Self {
        if step == current {
            Self::Current
        } else if step.index() < current.index() {
            Self::Done
        } else {
            Self::Ahead
        }
    }

    /// Radius of the dot or ring.
    #[must_use]
    fn radius(self) -> f32 {
        match self {
            Self::Current => metric::POINT_DOT + 1.0,
            Self::Done | Self::Ahead => metric::POINT_DOT,
        }
    }

    /// Whether the marker is a filled dot (`true`) or an open ring (`false`).
    #[must_use]
    fn is_filled(self) -> bool {
        match self {
            Self::Done | Self::Current => true,
            Self::Ahead => false,
        }
    }

    /// The colour of the marker.
    #[must_use]
    fn colour(self) -> Color32 {
        match self {
            Self::Current => theme::INK,
            Self::Done => theme::DATA,
            Self::Ahead => theme::MUTE,
        }
    }

    /// The label is never the divider colour.
    #[must_use]
    fn label_colour(self) -> Color32 {
        match self {
            Self::Current => theme::INK,
            Self::Done | Self::Ahead => theme::MUTE,
        }
    }
}

/// The three places amber may appear (DESIGN.md §3).
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
/// A dump is two reads from two boots, the same shape the CLI uses. The first
/// pass leaves the screen in [`DumpState::WaitingForPowerCycle`]; the user
/// power-cycles the unit and starts the second pass, and only two matching
/// digests produce [`DumpState::Verified`] and a file. A single read that
/// truncated still parses as an image, and the backup is the only way back, so a
/// one-read "verified" would be a claim the evidence does not support.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DumpState {
    /// Nothing has been read.
    #[default]
    Idle,
    /// A read is running: `read` of `total` bytes.
    FirstPass { read: usize, total: usize },
    /// The first read finished; the unit must be power-cycled before the second.
    WaitingForPowerCycle {
        /// Bytes the first read returned.
        total: usize,
        /// SHA-256 of the first read, compared with the second.
        digest: String,
    },
    /// Both reads finished and agreed; the image is saved.
    Verified { bytes: usize, digest: String },
    /// The two reads differed. Nothing was saved.
    Mismatch { first: String, second: String },
    /// The read was refused or failed. The device's words are kept verbatim.
    Failed(String),
}

/// What the last write or recovery produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FlashState {
    /// Nothing has been written this session.
    #[default]
    Idle,
    /// The session is running; `stage` is the step it is in.
    Running {
        /// The stage's own words.
        stage: String,
        /// Packets the DA acknowledged.
        packets: usize,
        /// Bytes those packets cover.
        bytes: usize,
        /// Bytes the range covers.
        total: usize,
    },
    /// The read-back matched.
    Succeeded {
        /// Packets acknowledged.
        packets: usize,
        /// Bytes read back and compared.
        verified: usize,
    },
    /// The write stopped; the message is the device's own words where it has them.
    Failed(String),
}

/// Which background job the screen is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Dump,
    Write,
    Recover,
    Restart,
    BootRom,
}

impl JobKind {
    /// The word the failure sentence starts with.
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            Self::Dump => "吸い出し",
            Self::Write => "書き込み",
            Self::Recover => "復旧",
            Self::Restart => "再起動",
            Self::BootRom => "BootROM 再入",
        }
    }
}

/// The progress fraction the dump bar shows, for the states that have one.
fn dump_progress(state: &DumpState) -> Option<f32> {
    match state {
        DumpState::Idle => Some(0.0),
        DumpState::FirstPass { read, total } => Some(dump_fraction(*read, *total)),
        DumpState::WaitingForPowerCycle { .. } => Some(1.0),
        DumpState::Verified { .. } | DumpState::Mismatch { .. } | DumpState::Failed(_) => None,
    }
}

/// Bytes read over the bytes the device announced, clamped to `0.0..=1.0`.
fn dump_fraction(read: usize, total: usize) -> f32 {
    if total == 0 {
        0.0
    } else {
        (read as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// Draw a progress bar with its caption inside it.
fn progress_bar(ui: &mut Ui, fraction: f32, caption: String) {
    let _bar = ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(ui.available_width())
            .corner_radius(theme::RADIUS)
            // The fraction is a measurement of bytes acknowledged, not motion to
            // be animated: animating it would draw a position the device never
            // reported.
            .animate(false)
            .text(RichText::new(caption).size(size::CAPTION)),
    );
}

/// The Firmware screen's own state.
#[derive(Debug)]
pub struct FirmwareScreen {
    step: Step,
    dump: DumpState,
    /// Path the dump is written to.
    dump_path: String,
    /// Serial port every device action on this screen opens.
    port: String,
    /// The pinned DA file.
    da_path: String,
    /// The official vendor image, when `--allow-vendor` applies.
    vendor_path: String,
    /// Whether the vendor gate is requested.
    allow_vendor: bool,
    /// Take the `--assume-brom` entry route.
    assume_brom: bool,
    /// The image currently loaded from disk (or injected), if any; the backup.
    image: Option<Vec<u8>>,
    /// Path the backup was read from, for the figure rows.
    source: String,
    /// The plan for [`Self::image`], or the reason there is none.
    plan: Option<PatchPlan>,
    /// The patched bytes, once the plan has been applied.
    patched: Option<Vec<u8>>,
    /// Where the patched image is (or would be) written.
    patched_path: String,
    /// The typed confirmation. Compared exactly to [`Self::CONFIRMATION`].
    confirmation: String,
    /// The typed confirmation for the no-reply `0xD9` restart.
    restart_confirmation: String,
    /// Whether the device's state is unknown.
    undefined: bool,
    /// The last thing the screen said, and how it should be read.
    message: Option<(Status, String)>,
    /// Show sample data, so the layout can be reviewed without a device.
    sample: bool,
    /// The write's state.
    flash: FlashState,
    /// The running job and its channel, when one is running.
    job: Option<(JobKind, mpsc::Receiver<crate::device::JobEvent>)>,
}

impl Default for FirmwareScreen {
    fn default() -> Self {
        Self {
            step: Step::Dump,
            dump: DumpState::Idle,
            dump_path: String::from("backup/stock.bin"),
            port: String::new(),
            da_path: String::from("DA_v3.8.4.bin"),
            vendor_path: String::new(),
            allow_vendor: false,
            assume_brom: false,
            image: None,
            source: String::from("backup/stock.bin"),
            plan: None,
            patched: None,
            patched_path: String::new(),
            confirmation: String::new(),
            restart_confirmation: String::new(),
            undefined: false,
            message: None,
            sample: false,
            flash: FlashState::Idle,
            job: None,
        }
    }
}

impl FirmwareScreen {
    /// The word the user must type before the write button unlocks.
    pub const CONFIRMATION: &'static str = "WRITE";

    /// The word the user must type before a `0xD9` restart is sent.
    ///
    /// `0xD9` has no reply, so the screen cannot show a result; the typed word is
    /// the user's acknowledgement that the unit will be restarted.
    pub const RESTART_CONFIRMATION: &'static str = "RESTART";

    /// The step the wizard is on.
    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    /// Move to a step.
    pub fn set_step(&mut self, step: Step) {
        self.step = step;
    }

    /// The dump's state.
    #[must_use]
    pub fn dump_state(&self) -> &DumpState {
        &self.dump
    }

    /// Put the screen in a specific dump state (render tests walk every state).
    pub fn set_dump_state(&mut self, state: DumpState) {
        self.dump = state;
    }

    /// The write's state.
    #[must_use]
    pub fn flash_state(&self) -> &FlashState {
        &self.flash
    }

    /// Put the screen in a specific write state.
    ///
    /// The headless screenshot run uses this to show the running and finished
    /// layouts, which a real device would otherwise be needed to produce.
    pub fn set_flash_state(&mut self, state: FlashState) {
        self.flash = state;
    }

    /// Declare the device's state unknown, which arms the amber recovery panel.
    pub fn set_undefined(&mut self, undefined: bool) {
        self.undefined = undefined;
    }

    /// Whether a background job is running.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Fill the screen with the sample image and its real patch plan.
    ///
    /// # Errors
    ///
    /// The planner's refusal, if the synthesised image is ever no longer recognised.
    pub fn show_sample(&mut self) -> Result<(), String> {
        self.sample = true;
        self.load_sample()
    }

    /// Fill the sample at the write step, with the patched image in memory and
    /// the confirmation field filled.
    ///
    /// Screenshot-side state: it never writes the patched file to disk, never
    /// opens a port and never starts a job — every figure on the screen stays
    /// labelled 見本. Filling the field is only how the armed button becomes
    /// renderable; the sample has no port, so [`Self::write_armed`] is still
    /// false and the real gate (the action's re-check) is untouched.
    pub(crate) fn show_sample_write(&mut self) -> Result<(), String> {
        self.show_sample()?;
        let patched = {
            let (Some(plan), Some(image)) = (self.plan.as_ref(), self.image.as_ref()) else {
                return Err(String::from("見本の差分を用意できません"));
            };
            plan.apply(image).map_err(|err| err.to_string())?
        };
        self.patched_path = default_patched_name(&self.source);
        self.patched = Some(patched);
        self.confirmation = String::from(Self::CONFIRMATION);
        self.step = Step::Flash;
        Ok(())
    }

    /// Report dump progress to the screen.
    pub fn set_dump_progress(&mut self, read: usize, total: usize) {
        self.dump = DumpState::FirstPass { read, total };
    }

    /// The plan the screen is showing, if a load succeeded.
    #[must_use]
    pub fn plan(&self) -> Option<&PatchPlan> {
        self.plan.as_ref()
    }

    /// Whether the write button would be enabled, i.e. every gate is satisfied.
    #[must_use]
    pub fn write_armed(&self) -> bool {
        let image_ready = self.patched.is_some() || self.allow_vendor;
        self.confirmation.trim() == Self::CONFIRMATION
            && image_ready
            && !self.port.trim().is_empty()
            && !self.undefined
            && self.job.is_none()
    }

    /// Whether the recovery button would be enabled: the backup and the word.
    ///
    /// Recovery deliberately ignores [`Self::undefined`]: that flag is set by a
    /// failed write, and re-writing the backup is exactly the action the unknown
    /// state calls for. The typed word is still required.
    #[must_use]
    pub fn recover_armed(&self) -> bool {
        self.confirmation.trim() == Self::CONFIRMATION
            && self.image.is_some()
            && !self.port.trim().is_empty()
            && self.job.is_none()
    }

    /// Load an image that is already in memory, plan the patch, and show step 2.
    ///
    /// # Errors
    ///
    /// The reasons `m241_core::firmware::plan` refuses.
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
    /// The size cap is a guard against pointing the text field at a disk image
    /// or a video by mistake: reading it would take a long time and parse as
    /// garbage. The largest release is under half a megabyte.
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
    /// The source file is never modified: the dump is the only way back, and a
    /// patch that overwrote it would destroy the recovery image at the moment
    /// the user is preparing to write.
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

    /// Start the read-only dump job.
    ///
    /// # Errors
    ///
    /// A missing port, a missing path, or a job already running.
    pub fn start_dump(&mut self) -> Result<(), String> {
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        if self.port.trim().is_empty() {
            return Err(String::from("ポートを入力してください"));
        }
        if self.dump_path.trim().is_empty() {
            return Err(String::from("保存先を入力してください"));
        }
        let job = crate::device::Job::Dump {
            port: self.port.trim().to_owned(),
            output: self.dump_path.trim().to_owned(),
            assume_brom: self.assume_brom,
        };
        self.dump = DumpState::FirstPass {
            read: 0,
            total: m241_flash::flow::FIRMWARE_DUMP_LENGTH,
        };
        self.message = Some((Status::Info, String::from("吸い出しています…")));
        self.start_job(JobKind::Dump, job);
        Ok(())
    }

    /// Start the second read of the dump, after the user power-cycled the unit.
    ///
    /// The first pass's digest is the comparison target. This is a separate job
    /// rather than a loop inside one job because the physical step between the
    /// reads is the user's: a background thread cannot ask them to pull the
    /// battery.
    ///
    /// # Errors
    ///
    /// No first pass to compare against, a missing port or path, or a job
    /// already running.
    pub fn start_dump_second(&mut self) -> Result<(), String> {
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        if self.port.trim().is_empty() {
            return Err(String::from("ポートを入力してください"));
        }
        if self.dump_path.trim().is_empty() {
            return Err(String::from("保存先を入力してください"));
        }
        let DumpState::WaitingForPowerCycle { digest, .. } = &self.dump else {
            return Err(String::from(
                "先に 1 回目の読み取りを終わらせてください（2 回目はそのハッシュと比較します）",
            ));
        };
        let first_digest = digest.clone();
        let job = crate::device::Job::DumpSecond {
            port: self.port.trim().to_owned(),
            output: self.dump_path.trim().to_owned(),
            assume_brom: self.assume_brom,
            first_digest,
        };
        self.dump = DumpState::FirstPass {
            read: 0,
            total: m241_flash::flow::FIRMWARE_DUMP_LENGTH,
        };
        self.message = Some((Status::Info, String::from("2 回目を読み取っています…")));
        self.start_job(JobKind::Dump, job);
        Ok(())
    }

    /// Start the write (`recover == false`) or the recovery write.
    ///
    /// The typed word is checked here as well as by the disabled button: a gate
    /// that exists only in a widget closure is a gate that a new call path skips.
    ///
    /// # Errors
    ///
    /// The typed word missing, no backup, another job running, a missing port or
    /// DA, or a patched image that cannot be written to disk.
    pub fn start_write(&mut self, recover: bool) -> Result<(), String> {
        if self.confirmation.trim() != Self::CONFIRMATION {
            return Err(format!(
                "端末に書き込むには {} と入力してください",
                Self::CONFIRMATION
            ));
        }
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        if self.port.trim().is_empty() {
            return Err(String::from("ポートを入力してください"));
        }
        if self.da_path.trim().is_empty() {
            return Err(String::from("DA のファイルを指定してください"));
        }
        if self.image.is_none() {
            return Err(String::from("先にバックアップを吸い出してください"));
        }
        if self.allow_vendor && !recover {
            if self.vendor_path.trim().is_empty() {
                return Err(String::from("ベンダーイメージのパスを入力してください"));
            }
        } else if !recover {
            if self.patched.is_none() {
                return Err(String::from(
                    "先に書き換えたイメージを作ってください（書き換えのステップ）",
                ));
            }
            self.ensure_patched_on_disk()?;
        }
        let backup = String::from(self.source.trim());
        let image = if recover {
            backup.clone()
        } else if self.allow_vendor {
            String::from(self.vendor_path.trim())
        } else {
            String::from(self.patched_path.trim())
        };
        let job = crate::device::FlashJob {
            port: self.port.trim().to_owned(),
            backup,
            image,
            da: self.da_path.trim().to_owned(),
            confirmation: Confirmation::granted(),
            recover,
            vendor: self.allow_vendor && !recover,
            assume_brom: self.assume_brom,
        };
        self.flash = FlashState::Running {
            stage: String::from("準備"),
            packets: 0,
            bytes: 0,
            total: FLASH_WRITE_LENGTH,
        };
        self.undefined = false;
        self.message = Some((
            Status::Info,
            String::from(if recover {
                "復旧イメージを書き込んでいます…"
            } else {
                "書き込んでいます…"
            }),
        ));
        self.start_job(
            if recover {
                JobKind::Recover
            } else {
                JobKind::Write
            },
            crate::device::Job::Flash(job),
        );
        Ok(())
    }

    /// Save the patched bytes if the flash job has not seen them on disk yet.
    fn ensure_patched_on_disk(&mut self) -> Result<(), String> {
        let Some(patched) = self.patched.as_ref() else {
            return Ok(());
        };
        if self.patched_path.trim().is_empty() {
            self.patched_path = default_patched_name(&self.source);
        }
        let path = self.patched_path.trim();
        if std::fs::read(path).is_ok_and(|existing| existing == *patched) {
            return Ok(());
        }
        std::fs::write(path, patched).map_err(|err| format!("{path} に書けません: {err}"))
    }

    /// Send the one-byte `0xD9` restart (`fw da-restart`).
    ///
    /// # Errors
    ///
    /// The typed word missing, a missing port, or a job already running.
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
        if self.port.trim().is_empty() {
            return Err(String::from("ポートを入力してください"));
        }
        self.start_job(
            JobKind::Restart,
            crate::device::Job::Restart {
                port: self.port.trim().to_owned(),
            },
        );
        Ok(())
    }

    /// Enter the BootROM and sync, without writing anything.
    ///
    /// # Errors
    ///
    /// A missing port, or a job already running.
    pub fn start_bootrom(&mut self) -> Result<(), String> {
        if self.job.is_some() {
            return Err(String::from("別の処理が動いています"));
        }
        if self.port.trim().is_empty() {
            return Err(String::from("ポートを入力してください"));
        }
        self.start_job(
            JobKind::BootRom,
            crate::device::Job::EnterBootRom {
                port: self.port.trim().to_owned(),
                assume_brom: self.assume_brom,
            },
        );
        Ok(())
    }

    /// Spawn a job and keep its channel.
    fn start_job(&mut self, kind: JobKind, job: crate::device::Job) {
        self.restart_confirmation.clear();
        self.job = Some((kind, crate::device::start(job)));
    }

    /// Poll the job's channel. Returns `true` while it is still running.
    pub fn poll_job(&mut self) -> bool {
        let Some(kind) = self.job.as_ref().map(|(kind, _)| *kind) else {
            return false;
        };
        let (events, disconnected) = match self.job.as_ref() {
            Some((_, rx)) => crate::device::poll_channel(rx),
            None => return false,
        };
        let mut finished = false;
        for event in events {
            if self.apply_event(kind, event) {
                finished = true;
                break;
            }
        }
        if !finished && disconnected {
            // The worker sends its own failure before returning; a disconnect
            // without one is a thread that died. A write that died mid-packet
            // leaves the unit in an unknown state, so the message must not
            // suggest pressing the same button again.
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

    /// Apply one queued job event. Returns `true` when the job ended.
    fn apply_event(&mut self, kind: JobKind, event: crate::device::JobEvent) -> bool {
        match event {
            crate::device::JobEvent::Flow(event) => {
                self.apply_flow_event(&event);
                false
            }
            crate::device::JobEvent::DumpFirst { digest, bytes } => {
                self.dump = DumpState::WaitingForPowerCycle {
                    total: bytes,
                    digest,
                };
                self.message = Some((
                    Status::Waiting,
                    String::from(
                        "1 回目を読みました。電池を抜いて 10 秒待ち、入れ直して電源を入れてから\
                         2 回目を読み取ってください。2 回のハッシュが一致したときだけ保存します",
                    ),
                ));
                true
            }
            crate::device::JobEvent::DumpMismatch { first, second } => {
                self.dump = DumpState::Mismatch { first, second };
                self.message = Some((
                    Status::Err,
                    String::from(
                        "2 回の読み取りが一致しません。保存していません。ケーブルと USB ポートを\
                         変えて、吸い出しをやり直してください",
                    ),
                ));
                true
            }
            crate::device::JobEvent::Dumped(bytes) => {
                let digest = firmware::sha256_hex(&bytes);
                let length = bytes.len();
                if let Some((status, text)) = self.dump_saved(&bytes) {
                    self.message = Some((status, text));
                }
                self.dump = DumpState::Verified {
                    bytes: length,
                    digest,
                };
                self.source = String::from(self.dump_path.trim());
                true
            }
            crate::device::JobEvent::WriteDone(report)
            | crate::device::JobEvent::RecoverDone(report) => {
                self.flash = FlashState::Succeeded {
                    packets: report.packets,
                    verified: report.verified,
                };
                self.undefined = false;
                self.message = Some((
                    Status::Ok,
                    format!(
                        "書き込みと読み戻しが一致しました（{} バイト。実機での書き込み検証は別途）",
                        common::grouped(report.verified)
                    ),
                ));
                true
            }
            crate::device::JobEvent::Restarted => {
                self.message = Some((
                    Status::Info,
                    String::from(
                        "0xD9 を送りました。この命令に応答はないため、再起動したかは確認できません。\
                         吸い出しを実行して端末の応答を確かめてください",
                    ),
                ));
                true
            }
            crate::device::JobEvent::BootRomAnswered => {
                self.undefined = false;
                self.message = Some((
                    Status::Ok,
                    String::from(
                        "BootROM の同期に応答しました。吸い出し・書き込みを再試行できます",
                    ),
                ));
                true
            }
            crate::device::JobEvent::Failed(message) => {
                self.fail(kind, message);
                true
            }
            // The Connect screen's jobs; the Firmware screen never starts them,
            // but a shared channel type lists them and this match stays total.
            crate::device::JobEvent::ProbeDone(_) | crate::device::JobEvent::DoctorDone(_) => true,
        }
    }

    /// Remember a saved dump and compute its plan.
    fn dump_saved(&mut self, bytes: &[u8]) -> Option<(Status, String)> {
        let loaded = self.load_image(bytes.to_vec());
        match loaded {
            Ok(()) => {
                self.step = Step::Dump;
                Some((
                    Status::Ok,
                    format!(
                        "{} に保存しました。次は書き換えのステップへ",
                        self.dump_path.trim()
                    ),
                ))
            }
            Err(reason) => Some((
                Status::Err,
                format!("保存しましたが、イメージとして認識できません: {reason}"),
            )),
        }
    }

    /// Fold one flow event into the visible progress.
    fn apply_flow_event(&mut self, event: &m241_flash::flow::Event) {
        use m241_flash::flow::Event;
        match event {
            Event::Stage(stage) => {
                let label = match stage {
                    m241_flash::flow::Stage::Entering => "BootROM に入っています",
                    m241_flash::flow::Stage::Reading => "吸い出しています",
                    m241_flash::flow::Stage::Comparing => "端末とバックアップを比較しています",
                    m241_flash::flow::Stage::LoadingDa => "DA を RAM に読み込んでいます",
                    m241_flash::flow::Stage::Writing => "書き込んでいます",
                    m241_flash::flow::Stage::ReadingBack => "読み戻して比較しています",
                };
                if self.job.as_ref().map(|(kind, _)| *kind) == Some(JobKind::Dump) {
                    if *stage == m241_flash::flow::Stage::Reading {
                        self.dump = DumpState::FirstPass {
                            read: 0,
                            total: m241_flash::flow::FIRMWARE_DUMP_LENGTH,
                        };
                    }
                } else {
                    let (packets, bytes, total) = match &self.flash {
                        FlashState::Running {
                            packets,
                            bytes,
                            total,
                            ..
                        } => (*packets, *bytes, *total),
                        _ => (0, 0, FLASH_WRITE_LENGTH),
                    };
                    self.flash = FlashState::Running {
                        stage: String::from(label),
                        packets,
                        bytes,
                        total,
                    };
                }
            }
            Event::Dump { read, total } => {
                self.dump = DumpState::FirstPass {
                    read: *read,
                    total: *total,
                };
            }
            Event::DaLoaded { version, region } => {
                let stage = format!(
                    "DA v{}.{}（region 0x{region:02X}）を読み込みました",
                    version >> 8,
                    version & 0xFF
                );
                if let FlashState::Running {
                    packets,
                    bytes,
                    total,
                    ..
                } = &self.flash
                {
                    self.flash = FlashState::Running {
                        stage,
                        packets: *packets,
                        bytes: *bytes,
                        total: *total,
                    };
                }
            }
            Event::Write {
                packets,
                bytes,
                total,
            } => {
                self.flash = FlashState::Running {
                    stage: String::from("書き込んでいます"),
                    packets: *packets,
                    bytes: *bytes,
                    total: *total,
                };
            }
        }
    }

    /// Record a failed job.
    fn fail(&mut self, kind: JobKind, message: String) {
        match kind {
            JobKind::Dump => self.dump = DumpState::Failed(message.clone()),
            JobKind::Write | JobKind::Recover => {
                self.flash = FlashState::Failed(message.clone());
                // A write that did not report a verified read-back may have left
                // the unit in an unknown state; the amber panel is the safe
                // default until a dump shows otherwise.
                self.undefined = true;
            }
            JobKind::Restart | JobKind::BootRom => {}
        }
        self.message = Some((Status::Err, format!("{}: {message}", kind.label())));
    }

    /// Draw the screen body. The rail is drawn by the shell, not here.
    pub fn ui(&mut self, ui: &mut Ui) {
        if self.poll_job() {
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }

        common::footer(ui, "m241-firmware-actions", |ui| self.action_bar(ui));

        common::scrolling(ui, "m241-firmware-body", |ui| {
            common::heading(
                ui,
                "ファームを書き換える",
                "本体のファームを吸い出し、日付定数を書き換えて書き戻します。",
            );
            if self.sample {
                ui.add_space(space::S);
                common::sample_marker(ui);
            }
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
                Step::Flash => {
                    // The running / result block sits above the panel, not under
                    // it: at 1280x800 the panel alone fills the page, and a
                    // progress bar below the fold is not a progress bar. It is
                    // drawn here once — `flash_step` does not repeat it (the
                    // duplicate was a review finding).
                    self.write_state_panel(ui);
                    ui.add_space(space::M);
                    self.flash_step(ui);
                }
            }

            ui.add_space(space::L);
            self.sample_toggle(ui);
        });
    }

    /// 中止 / 続行 on the first two steps, 中止 / 復旧 / 書き込む on the third.
    fn action_bar(&mut self, ui: &mut Ui) {
        if self.step == Step::Flash {
            self.gate_rows(ui);
            ui.add_space(space::XS);
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "ステップ {} / 3　{}",
                    self.step.index() + 1,
                    self.step.label()
                ))
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.step == Step::Flash {
                    let response = self.write_button(ui);
                    if response.clicked() {
                        self.message = Some(match self.start_write(false) {
                            Ok(()) => (Status::Info, String::from("書き込みを開始しました")),
                            Err(reason) => (Status::Err, reason),
                        });
                    }
                    if common::secondary_button(ui, "復旧", self.recover_armed()).clicked()
                        && let Err(reason) = self.start_write(true)
                    {
                        self.message = Some((Status::Err, reason));
                    }
                } else if let Some(next) = self.step.next()
                    && common::primary_button(ui, "続行", true).clicked()
                {
                    self.step = next;
                }
                if common::secondary_button(ui, "中止", true).clicked() {
                    self.step = Step::Dump;
                    self.confirmation.clear();
                    self.restart_confirmation.clear();
                    self.undefined = false;
                }
            });
        });
        if let Some((status, text)) = &self.message {
            ui.add_space(space::S);
            common::status_line(ui, *status, text);
        }
    }

    /// 1 吸い出し 2 書き換え 3 書き戻し, drawn as a rail of its own.
    fn step_row(&self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, _response) =
            ui.allocate_exact_size(Vec2::new(width, metric::ROW * 2.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let padding = space::L;
        let count = Step::ALL.len();
        let span = rect.width() - padding * 2.0;
        let line_y = rect.top() + metric::ROW;
        let step_width = span / (count - 1) as f32;

        let positions: Vec<egui::Pos2> = (0..count)
            .map(|index| egui::pos2(rect.left() + padding + step_width * index as f32, line_y))
            .collect();
        if let (Some(first), Some(last)) = (positions.first(), positions.last()) {
            painter.line_segment([*first, *last], Stroke::new(metric::HAIRLINE, theme::LINE));
            if let Some(current) = positions.get(self.step.index()) {
                painter.line_segment(
                    [*first, *current],
                    Stroke::new(metric::TRACK_LINE, theme::INK),
                );
            }
        }

        for step in Step::ALL {
            let Some(center) = positions.get(step.index()).copied() else {
                continue;
            };
            let marker = StepMarker::of(step, self.step);
            if marker.is_filled() {
                painter.circle_filled(center, marker.radius(), marker.colour());
            } else {
                painter.circle_stroke(
                    center,
                    marker.radius(),
                    Stroke::new(metric::HAIRLINE, marker.colour()),
                );
            }
            let anchor = match step.index() {
                0 => egui::Align2::LEFT_TOP,
                index if index + 1 == count => egui::Align2::RIGHT_TOP,
                _ => egui::Align2::CENTER_TOP,
            };
            painter.text(
                center + Vec2::new(0.0, space::M),
                anchor,
                format!("{} {}", step.index() + 1, step.label()),
                egui::FontId::proportional(size::CAPTION),
                marker.label_colour(),
            );
        }
    }

    /// Step 1: read the image off the device. Read-only.
    fn dump_step(&mut self, ui: &mut Ui) {
        common::panel(ui, "吸い出し", |ui| {
            ui.label(
                RichText::new("読み取りのみ。本体は書き換えません。")
                    .size(size::UI)
                    .color(theme::MUTE),
            );
            ui.add_space(space::M);
            self.port_row(ui);

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
                // carry; the path is typed, and the job refuses an existing file so
                // the backup cannot be overwritten by accident.
            });
            ui.add_space(space::S);
            ui.checkbox(
                &mut self.assume_brom,
                RichText::new("端末はすでに BootROM にいる（--assume-brom 相当）")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );

            ui.add_space(space::M);
            if let Some(fraction) = dump_progress(&self.dump) {
                let caption = match &self.dump {
                    DumpState::Idle => String::from("まだ読み取っていません"),
                    DumpState::FirstPass { read, total } => format!(
                        "{} / {} バイト",
                        common::grouped(*read),
                        common::grouped(*total)
                    ),
                    DumpState::WaitingForPowerCycle { total, .. } => format!(
                        "{} / {} バイト（読み取り完了）",
                        common::grouped(*total),
                        common::grouped(*total)
                    ),
                    DumpState::Verified { .. }
                    | DumpState::Mismatch { .. }
                    | DumpState::Failed(_) => String::new(),
                };
                progress_bar(ui, fraction, caption);
            }

            match &self.dump {
                DumpState::WaitingForPowerCycle { .. } => {
                    common::status_line(ui, Status::Waiting, "端末の操作待ちです");
                    ui.add_space(space::S);
                    ui.label(
                        RichText::new(
                            "電池を抜いて 10 秒待ち、入れ直して電源を入れてから、\
                             2 回目の読み取りを始めてください。2 回のハッシュが一致したときだけ\
                             保存します。",
                        )
                        .size(size::UI)
                        .color(theme::INK),
                    );
                    ui.add_space(space::S);
                    let ready = !self.busy();
                    if common::primary_button(ui, "2 回目を読み取る", ready).clicked()
                        && let Err(reason) = self.start_dump_second()
                    {
                        self.message = Some((Status::Err, reason));
                    }
                }
                DumpState::Verified { bytes, digest } => {
                    common::status_line(
                        ui,
                        Status::Ok,
                        "本体から読み取りました（2 回のハッシュが一致）",
                    );
                    ui.add_space(space::S);
                    common::figure_row(ui, "サイズ", &common::bytes(*bytes));
                    common::figure_row(ui, "SHA-256", digest);
                    common::figure_row(ui, "保存先", self.dump_path.trim());
                }
                DumpState::Mismatch { first, second } => {
                    common::status_line(ui, Status::Err, "2 回の読み取りが一致しません（未保存）");
                    ui.add_space(space::S);
                    common::figure_row(ui, "1 回目", first);
                    common::figure_row(ui, "2 回目", second);
                }
                DumpState::Failed(reason) => {
                    common::status_line(ui, Status::Err, reason);
                }
                DumpState::Idle | DumpState::FirstPass { .. } => {}
            }

            ui.add_space(space::M);
            ui.horizontal(|ui| {
                let running = self.busy();
                if common::secondary_button(ui, "吸い出す", !running).clicked()
                    && let Err(reason) = self.start_dump()
                {
                    self.message = Some((Status::Err, reason));
                }
                ui.label(
                    RichText::new(cli_note("m241 fw dump"))
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
            ui.add_space(space::S);
            ui.label(
                RichText::new(
                    "2 回読み（1 回目のあと電池を抜き差し）でハッシュを比較します。\
                     一致したイメージだけを保存し、これが復旧用のバックアップになります。",
                )
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
        });
    }

    /// The port field shared by every device action on this screen.
    fn port_row(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ポート")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let _response = ui.add(
                egui::TextEdit::singleline(&mut self.port)
                    .hint_text("/dev/ttyUSB0")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width() * 0.5),
            );
            ui.label(
                RichText::new("38400 bps・8N1")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
    }

    /// Step 2: rewrite the constant in a file. Real work, no device.
    fn patch_step(&mut self, ui: &mut Ui) {
        let sample = self.sample;
        common::sample_panel(ui, "書き換え", sample, |ui| {
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
                if common::secondary_button(ui, "開く", true).clicked() {
                    let source = self.source.clone();
                    self.message = Some(match self.open(&source) {
                        Ok(()) => (Status::Ok, format!("{} を読み込みました", self.source)),
                        Err(reason) => (Status::Err, reason),
                    });
                }
            });

            ui.add_space(space::M);
            self.plan_panel(ui);
            ui.add_space(space::S);
            ui.label(
                RichText::new(cli_note("m241 fw patch"))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
    }

    /// The 差分 block: what the plan would change, byte by byte.
    fn plan_panel(&mut self, ui: &mut Ui) {
        let Some(plan) = self.plan.clone() else {
            common::status_line(ui, Status::Info, "イメージを読み込むと差分が出ます");
            return;
        };

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

        common::table_header(
            ui,
            &[
                ("オフセット", 0.4, egui::Align::Min),
                ("変更前", 0.3, egui::Align::Min),
                ("変更後", 0.3, egui::Align::Min),
            ],
        );
        for edit in &plan.edits {
            common::table_row(
                ui,
                &[
                    (&format!("0x{:05X}", edit.offset), 0.4),
                    (&format!("0x{:02X}", edit.before), 0.3),
                    (&format!("0x{:02X}", edit.after), 0.3),
                ],
            );
        }

        ui.add_space(space::S);
        if let Some(profile) = plan.profile.as_ref() {
            common::figure_row(ui, "プロファイル", &profile.to_string());
        }
        common::figure_row(
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
            if common::primary_button(ui, "書き換えたイメージを保存", true).clicked() {
                self.message = Some(match self.write_patched() {
                    Ok(()) => (Status::Ok, format!("{} に保存しました", self.patched_path)),
                    Err(reason) => (Status::Err, reason),
                });
            }
        }
    }

    /// Step 3: write to the device. The only destructive act in the product.
    ///
    /// Kept as one function because the panel is read top to bottom before the
    /// typed word (range, image, diff, vendor gate, confirmation); splitting it
    /// would hide that order. The rows all borrow different fields of `self`,
    /// which is why they are drawn inline rather than through row helpers.
    fn flash_step(&mut self, ui: &mut Ui) {
        let (border, voice_title) = Destructive::WriteStep.voice();
        let title = if self.sample {
            format!("{voice_title}／{}", common::SAMPLE_WORD)
        } else {
            String::from(voice_title)
        };
        common::panel_with_border(ui, &title, border, |ui| {
            ui.label(
                RichText::new(
                    "本体のファーム全体を上書きします。失敗すると使えなくなる場合があります。\
                     書き込み後に全長を読み戻して照合します。",
                )
                .size(size::UI)
                .color(theme::INK),
            );
            // The same 未検証 word the Connect screen shows for this row: the
            // write sequence has run on a unit through the CLI, but this GUI
            // route has not (docs/dev/05-verification.md). A read-back match is
            // not hardware verification, so the screen must not imply it is.
            common::status_line(
                ui,
                Status::Unverified,
                "この書き込み経路は実機で未検証です（CLI の書き込みは実機で実行済み）",
            );
            ui.add_space(space::M);
            self.port_row(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new("DA").size(size::CAPTION).color(theme::MUTE));
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.da_path)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - metric::ACTION_WIDTH - space::L),
                );
            });

            ui.add_space(space::M);
            // The review a person does before typing WRITE: which range, which
            // image, which digest, which bytes differ.
            common::figure_row(
                ui,
                "書き込み範囲",
                &format!(
                    "0x{FLASH_WRITE_START:08X}〜0x{FLASH_WRITE_END:08X}（{} バイト）",
                    common::grouped(FLASH_WRITE_LENGTH)
                ),
            );
            let image_path = if self.allow_vendor {
                self.vendor_path.clone()
            } else {
                self.patched_path.clone()
            };
            let image_bytes = if self.allow_vendor {
                None
            } else {
                self.patched.as_ref()
            };
            let size = image_bytes
                .map(|bytes| common::bytes(bytes.len()))
                .unwrap_or_else(|| String::from("—"));
            let digest = image_bytes
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
            common::figure_row(
                ui,
                "書き込むイメージ",
                &format!("{}  {size}  {digest}", file_label(&image_path)),
            );
            common::figure_row(
                ui,
                "上書きされる内容",
                "本体ファーム全体（復旧用バックアップは PC 上に残ります）",
            );
            let diff = self.plan.as_ref().map_or_else(
                || String::from("—"),
                |plan| {
                    let offsets: Vec<String> = plan
                        .edits
                        .iter()
                        .map(|edit| format!("0x{:05X}", edit.offset))
                        .collect();
                    format!("{} バイト（{}）", plan.edits.len(), offsets.join(", "))
                },
            );
            common::figure_row(ui, "差分", &diff);

            ui.add_space(space::S);
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut self.allow_vendor,
                    RichText::new("ベンダーイメージを書き込む（--allow-vendor 相当）")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
            if self.allow_vendor {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ベンダー")
                            .size(size::CAPTION)
                            .color(theme::MUTE),
                    );
                    let _response = ui.add(
                        egui::TextEdit::singleline(&mut self.vendor_path)
                            .hint_text("V1.13 の公式イメージ")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(ui.available_width() - space::L),
                    );
                });
                ui.add_space(space::S);
                ui.label(
                    RichText::new(format!(
                        "警告: 公式ベンダーイメージは端末の個体データ {} バイトを上書きし、\
                         日付修正も入っていません（V1.13 のまま）。固定 SHA-256 の V1.13 のみ許可されます。",
                        VENDOR_PER_UNIT_DATA_BYTES
                    ))
                    .size(size::CAPTION)
                    .color(theme::INK),
                );
            }
            ui.checkbox(
                &mut self.assume_brom,
                RichText::new("端末はすでに BootROM にいる（--assume-brom 相当）")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );

            ui.add_space(space::M);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("確認のため {} と入力", Self::CONFIRMATION))
                        .size(size::UI)
                        .color(theme::INK),
                );
                let _response = ui.add_sized(
                    Vec2::new(metric::ACTION_WIDTH * 2.0, metric::ROW),
                    egui::TextEdit::singleline(&mut self.confirmation)
                        .hint_text(Self::CONFIRMATION)
                        .font(egui::TextStyle::Monospace),
                );
            });
            ui.add_space(space::S);
            ui.label(
                RichText::new(cli_note("m241 fw flash"))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
    }

    /// Progress, result, or the failure's next steps.
    fn write_state_panel(&mut self, ui: &mut Ui) {
        match self.flash.clone() {
            FlashState::Idle => {
                common::status_line(ui, Status::Info, "まだ書き込んでいません");
            }
            FlashState::Running {
                stage,
                packets,
                bytes,
                total,
            } => {
                common::status_line(ui, Status::Info, &stage);
                ui.add_space(space::S);
                let fraction = dump_fraction(bytes, total);
                progress_bar(
                    ui,
                    fraction,
                    format!(
                        "パケット {}・{} / {} バイト",
                        common::grouped(packets),
                        common::grouped(bytes),
                        common::grouped(total)
                    ),
                );
            }
            FlashState::Succeeded { packets, verified } => {
                common::status_line(
                    ui,
                    Status::Ok,
                    "読み戻しが一致しました（実機での書き込み検証は未実施）",
                );
                ui.add_space(space::S);
                common::figure_row(ui, "パケット", &common::grouped(packets));
                common::figure_row(
                    ui,
                    "読み戻したバイト",
                    &format!("{} バイト（イメージと一致）", common::grouped(verified)),
                );
            }
            FlashState::Failed(reason) => {
                // The recovery actions live in the amber panel above, which is
                // the one that says the device's state is unknown.
                common::status_line(ui, Status::Err, &reason);
            }
        }
    }

    /// The write button, the only amber-filled control in the product.
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
            ui.style_mut().visuals.disabled_alpha = 1.0;
            let response = ui.add_enabled(armed, button);
            common::focus_ring(ui, &response);
            response
        })
        .inner
    }

    /// The prerequisites of the write, each with its state in words.
    fn gate_rows(&self, ui: &mut Ui) {
        let backup = self.image.is_some();
        let image_ready = self.patched.is_some() || self.allow_vendor;
        for (held, text) in [
            (backup, "元のイメージを吸い出してある"),
            (
                image_ready,
                if self.allow_vendor {
                    "ベンダーイメージを指定してある（--allow-vendor 相当）"
                } else {
                    "書き換えたイメージを保存してある（バックアップは残っています）"
                },
            ),
            (
                self.confirmation.trim() == Self::CONFIRMATION,
                "WRITE と入力してある",
            ),
            (!self.port.trim().is_empty(), "ポートを指定してある"),
        ] {
            common::status_line(ui, if held { Status::Ok } else { Status::Todo }, text);
        }
    }

    /// The undefined-device state (DESIGN.md §6) with the GUI's recovery actions.
    ///
    /// Amber, because it is the state where the user must not guess. The actions
    /// are the CLI's recovery steps without a terminal: `0xD9` restart (typed
    /// word), BootROM re-entry, and a recovery write of the backup.
    fn undefined_panel(&mut self, ui: &mut Ui) {
        let (border, title) = Destructive::UndefinedState.voice();
        common::panel_with_border(ui, title, border, |ui| {
            ui.label(
                RichText::new(
                    "書き込みが途中で止まったか、読み戻しが一致しませんでした。端末は応答しない\
                     可能性があります。電池やケーブルを抜く必要はありません。",
                )
                .size(size::UI)
                .color(theme::INK),
            );
            ui.add_space(space::S);
            if let FlashState::Failed(reason) = &self.flash {
                common::status_line(ui, Status::Err, reason);
                ui.add_space(space::S);
            }
            self.port_row(ui);
            ui.add_space(space::S);
            self.recovery_actions(ui);
        });
    }

    /// The three software ways back, in the order the CLI tries them.
    fn recovery_actions(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("復旧の手順（上から試してください。物理的な抜き差しは不要です）")
                .size(size::CAPTION)
                .color(theme::MUTE),
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
                && self.job.is_none();
            if common::secondary_button(ui, "0xD9 再起動", ready).clicked()
                && let Err(reason) = self.start_restart()
            {
                self.message = Some((Status::Err, reason));
            }
            ui.label(
                RichText::new(cli_note("m241 fw da-restart"))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
        ui.add_space(space::S);

        ui.horizontal(|ui| {
            if common::secondary_button(ui, "BootROM 再入を確認", self.job.is_none()).clicked()
                && let Err(reason) = self.start_bootrom()
            {
                self.message = Some((Status::Err, reason));
            }
            ui.label(
                RichText::new(cli_note("--assume-brom"))
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });
        ui.add_space(space::S);
        ui.label(
            RichText::new(
                "応答が戻ったら「復旧」を押し、吸い出したバックアップを書き戻してください。\
                 復旧はベクタとファミリの確認だけを行い、全長比較は省きます（--recover 相当）。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
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
        if self.plan.is_none()
            && let Ok(()) = self.load_sample()
        {
            self.step = Step::Patch;
        }
    }

    /// Load a synthetic stock V1.13 image and plan it.
    fn load_sample(&mut self) -> Result<(), String> {
        let bytes = sample_image();
        self.source = String::from("backup/stock.bin");
        self.dump_path = String::from("backup/stock.bin");
        self.load_image(bytes)
    }

    /// The largest file this screen will read as a firmware image.
    const MAX_IMAGE_BYTES: usize = 1 << 20;
}

/// A short note saying which CLI operation the control is the same as.
fn cli_note(command: &str) -> String {
    format!("CLI の {command} と同じ処理です")
}

/// A synthesised stock V1.13 image, for the sample.
fn sample_image() -> Vec<u8> {
    use m241_core::firmware::image::{
        BUILD_NUMBER_OFFSET, DEVICE_TYPE_OFFSET, FAMILY, FAMILY_OFFSET, FW_SIZE_OFFSET,
    };
    let profile = PROFILE_V1_13;
    let mut data = vec![0xFF_u8; profile.size];
    if let Some(field) = data.get_mut(FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()) {
        field.copy_from_slice(FAMILY.as_bytes());
    }
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

/// The write button's (surface, label) pair for its two states.
#[must_use]
fn confirm_button_colours(amber: Color32, armed: bool) -> (Color32, Color32) {
    if armed {
        (amber, theme::WINDOW)
    } else {
        (theme::WINDOW, theme::INK)
    }
}

/// Version string for the footer figure.
#[must_use]
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use m241_flash::da::{PACKET_SIZE, WriteReport};

    fn screen() -> FirmwareScreen {
        FirmwareScreen::default()
    }

    /// WCAG relative luminance, so the button's contrast is measured rather than
    /// claimed.
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
    fn the_step_markers_are_told_apart_by_shape_not_only_by_colour() {
        let markers = [
            StepMarker::of(Step::Dump, Step::Patch),
            StepMarker::of(Step::Patch, Step::Patch),
            StepMarker::of(Step::Flash, Step::Patch),
        ];
        assert_eq!(
            markers,
            [StepMarker::Done, StepMarker::Current, StepMarker::Ahead]
        );
        assert!(markers[0].is_filled(), "Done must be a filled dot");
        assert!(markers[1].is_filled(), "Current must be a filled dot");
        assert!(!markers[2].is_filled(), "Ahead must be an open ring");
        assert_eq!(markers[0].radius(), markers[2].radius());
        assert!(markers[1].radius() > markers[0].radius());
    }

    #[test]
    fn a_step_label_is_never_a_line_colour() {
        for step in Step::ALL {
            for current in Step::ALL {
                assert_ne!(StepMarker::of(step, current).label_colour(), theme::LINE);
            }
        }
    }

    #[test]
    fn the_write_button_label_clears_wcag_aa_in_both_states() {
        let (amber, _label) = Destructive::ConfirmButton.voice();
        for armed in [false, true] {
            let (fill, ink) = confirm_button_colours(amber, armed);
            let ratio = contrast(ink, fill);
            assert!(
                ratio >= 4.5,
                "the {} write button is {ratio:.2}:1 (ink {ink:?}, fill {fill:?})",
                if armed { "armed" } else { "locked" }
            );
        }
    }

    #[test]
    fn amber_has_exactly_three_places_and_each_carries_words() {
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
    fn the_typed_word_is_checked_by_the_action_and_not_only_by_the_button() {
        // A gate that lives only in a widget closure is skipped by any new call
        // path. With every other input present but no typed word, the action must
        // refuse before a job exists.
        let mut screen = screen();
        screen.image = Some(sample_image());
        screen.patched = Some(sample_image());
        screen.patched_path = String::from("backup/stock.fixed.bin");
        screen.port = String::from("/dev/ttyUSB0");
        screen.da_path = String::from("DA_v3.8.4.bin");
        let error = screen
            .start_write(false)
            .expect_err("the write must refuse without the typed confirmation");
        assert!(
            error.contains("WRITE"),
            "the refusal does not ask for WRITE: {error}"
        );
        assert!(!screen.busy(), "a refused write started a job");
    }

    #[test]
    fn a_vendor_write_is_armed_without_a_patched_image() {
        let mut screen = screen();
        screen.confirmation = String::from("WRITE");
        screen.port = String::from("/dev/ttyUSB0");
        screen.allow_vendor = true;
        screen.vendor_path = String::from("vendor/V1.13.bin");
        assert!(
            screen.write_armed(),
            "the vendor path writes the vendor image, so a patched copy is not needed"
        );
        screen.allow_vendor = false;
        assert!(
            !screen.write_armed(),
            "without the vendor gate a write still needs a patched image"
        );
    }

    #[test]
    fn an_undefined_device_state_disarms_the_write() {
        let mut screen = screen();
        screen.confirmation = String::from("WRITE");
        screen.patched = Some(sample_image());
        screen.port = String::from("/dev/ttyUSB0");
        assert!(screen.write_armed());
        screen.undefined = true;
        assert!(
            !screen.write_armed(),
            "a write must not be offered while the device's state is unknown"
        );
    }

    #[test]
    fn a_failed_write_arms_the_recovery_panel_and_keeps_the_devices_words() {
        let mut screen = screen();
        screen.confirmation = String::from("WRITE");
        screen.image = Some(sample_image());
        screen.apply_event(
            JobKind::Write,
            crate::device::JobEvent::Failed(String::from("0xD5 packet 3 was refused")),
        );
        assert!(
            screen.undefined,
            "a failed write must arm the recovery panel"
        );
        assert!(matches!(screen.flash, FlashState::Failed(_)));
        let (status, text) = screen.message.clone().expect("a message was set");
        assert_eq!(status, Status::Err);
        assert!(
            text.contains("0xD5 packet 3"),
            "the device's words were lost: {text}"
        );
    }

    #[test]
    fn write_progress_and_the_read_back_reach_the_screen() {
        let mut screen = screen();
        screen.undefined = true;
        screen.apply_event(
            JobKind::Write,
            crate::device::JobEvent::Flow(m241_flash::flow::Event::Write {
                packets: 400,
                bytes: 102_400,
                total: FLASH_WRITE_LENGTH,
            }),
        );
        assert_eq!(
            screen.flash,
            FlashState::Running {
                stage: String::from("書き込んでいます"),
                packets: 400,
                bytes: 102_400,
                total: FLASH_WRITE_LENGTH,
            }
        );
        screen.apply_event(
            JobKind::Write,
            crate::device::JobEvent::WriteDone(WriteReport {
                start: FLASH_WRITE_START,
                length: FLASH_WRITE_LENGTH,
                packets: FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE),
                verified: FLASH_WRITE_LENGTH,
            }),
        );
        assert_eq!(
            screen.flash,
            FlashState::Succeeded {
                packets: FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE),
                verified: FLASH_WRITE_LENGTH,
            }
        );
        assert!(
            !screen.undefined,
            "a verified read-back must clear the amber panel"
        );
    }

    #[test]
    fn a_dump_job_failure_is_shown_on_the_dump_step() {
        let mut screen = screen();
        screen.apply_event(
            JobKind::Dump,
            crate::device::JobEvent::Failed(String::from("no reply from the device")),
        );
        assert!(matches!(screen.dump, DumpState::Failed(_)));
        assert!(
            !screen.undefined,
            "a read-only failure must not claim the device's state is unknown"
        );
    }

    #[test]
    fn a_dump_is_two_reads_and_the_second_pass_needs_the_first() {
        // The GUI's dump is the CLI's two reads: a first pass that reports its
        // digest, a physical power cycle, then a second pass compared against it.
        // A one-read dump would mark a truncated read as verified, and the
        // backup is the only way back.
        let mut screen = screen();
        screen.port = String::from("/dev/ttyUSB0");
        screen.dump_path = String::from("backup/stock.bin");
        screen.dump = DumpState::Idle;
        let error = screen
            .start_dump_second()
            .expect_err("the second pass must not start before the first");
        assert!(
            error.contains("1 回目"),
            "the refusal does not say why: {error}"
        );
        assert!(!screen.busy(), "a refused second pass started a job");

        // The first pass's event is what arms the second one, and the digest it
        // reports is what the comparison will use.
        screen.apply_event(
            JobKind::Dump,
            crate::device::JobEvent::DumpFirst {
                digest: String::from("aaaa"),
                bytes: 406_620,
            },
        );
        assert_eq!(
            screen.dump,
            DumpState::WaitingForPowerCycle {
                total: 406_620,
                digest: String::from("aaaa"),
            }
        );
        assert!(
            screen.start_dump_second().is_ok(),
            "the second pass must start once the first finished"
        );
        assert!(screen.busy(), "the second pass did not start a job");
    }

    #[test]
    fn two_reads_that_disagree_are_shown_and_nothing_is_verified() {
        let mut screen = screen();
        screen.apply_event(
            JobKind::Dump,
            crate::device::JobEvent::DumpMismatch {
                first: String::from("aaaa"),
                second: String::from("bbbb"),
            },
        );
        assert_eq!(
            screen.dump,
            DumpState::Mismatch {
                first: String::from("aaaa"),
                second: String::from("bbbb"),
            }
        );
        assert!(
            !matches!(screen.dump, DumpState::Verified { .. }),
            "a mismatched pair must never look verified"
        );
        assert!(
            screen.image.is_none(),
            "a mismatched pair must not become the loaded backup image"
        );
    }

    #[test]
    fn two_reads_that_agree_become_the_loaded_backup() {
        let mut screen = screen();
        screen.dump_path = String::from("backup/stock.bin");
        screen.apply_event(
            JobKind::Dump,
            crate::device::JobEvent::Dumped(sample_image()),
        );
        let DumpState::Verified { bytes, digest } = &screen.dump else {
            panic!("an agreeing pair must be verified, got {:?}", screen.dump);
        };
        assert_eq!(*bytes, PROFILE_V1_13.size);
        assert!(!digest.is_empty(), "the saved digest is empty");
        assert!(
            screen.image.is_some(),
            "the verified dump must become the backup image"
        );
    }

    #[test]
    fn the_write_success_text_does_not_claim_hardware_verification() {
        // `検証済み` is the repository's word for a hardware measurement, so a
        // read-back match must not use it (readability Must 1). The screen says
        // 未検証 in the write step and qualifies the success line instead.
        let mut screen = screen();
        screen.apply_event(
            JobKind::Write,
            crate::device::JobEvent::WriteDone(WriteReport {
                start: FLASH_WRITE_START,
                length: FLASH_WRITE_LENGTH,
                packets: FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE),
                verified: FLASH_WRITE_LENGTH,
            }),
        );
        let (status, text) = screen.message.clone().expect("a result message was set");
        assert_eq!(status, Status::Ok);
        assert!(
            text.contains("未実施") || text.contains("検証は別途"),
            "the success line claims more than a read-back match: {text}"
        );
        assert!(
            !text.contains("検証済み"),
            "the success line uses the hardware word 検証済み: {text}"
        );
    }

    #[test]
    fn the_file_write_keeps_the_backup_and_names_the_copy() {
        assert_eq!(
            Path::new(&default_patched_name("backup/stock.bin")),
            Path::new("backup/stock.fixed.bin")
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
    fn the_waiting_state_holds_the_dump_progress_at_one_hundred_percent() {
        assert_eq!(
            dump_progress(&DumpState::WaitingForPowerCycle {
                total: 406_620,
                digest: String::from("first"),
            }),
            Some(1.0)
        );
        assert_eq!(dump_progress(&DumpState::Idle), Some(0.0));
        assert_eq!(
            dump_progress(&DumpState::FirstPass {
                read: 203_310,
                total: 406_620
            }),
            Some(0.5)
        );
        assert_eq!(
            dump_progress(&DumpState::FirstPass {
                read: 500_000,
                total: 406_620
            }),
            Some(1.0)
        );
        assert_eq!(
            dump_progress(&DumpState::Verified {
                bytes: 406_620,
                digest: String::new()
            }),
            None
        );
        assert_eq!(dump_progress(&DumpState::Failed(String::new())), None);
    }

    #[test]
    fn the_restart_needs_its_own_typed_word() {
        let mut screen = screen();
        screen.port = String::from("/dev/ttyUSB0");
        let error = screen
            .start_restart()
            .expect_err("0xD9 must not be sent without its typed word");
        assert!(
            error.contains("RESTART"),
            "the refusal does not ask for RESTART: {error}"
        );
        assert!(!screen.busy());
    }

    #[test]
    fn recovery_stays_available_after_a_failed_write() {
        let mut screen = screen();
        screen.confirmation = String::from("WRITE");
        screen.image = Some(sample_image());
        screen.port = String::from("/dev/ttyUSB0");
        screen.apply_event(
            JobKind::Write,
            crate::device::JobEvent::Failed(String::from("read-back differs")),
        );
        assert!(
            screen.undefined,
            "the fixture did not reach the unknown state"
        );
        assert!(
            !screen.write_armed(),
            "a normal write must not be offered while the state is unknown"
        );
        assert!(
            screen.recover_armed(),
            "recovery is the action the unknown state exists for"
        );
    }

    #[test]
    fn a_second_job_is_refused_while_one_runs() {
        let mut screen = screen();
        let (_tx, rx) = mpsc::channel();
        screen.job = Some((JobKind::Dump, rx));
        screen.port = String::from("/dev/ttyUSB0");
        screen.dump_path = String::from("backup/stock.bin");
        assert!(screen.start_dump().is_err());
        assert!(screen.start_bootrom().is_err());
        assert!(screen.start_restart().is_err());
    }
}
