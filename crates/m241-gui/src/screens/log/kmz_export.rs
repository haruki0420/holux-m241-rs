//! KMZ writing for the Log screen (DESIGN.md §5's `[ KMZ ]` button).
//!
//! The archive planner is the core's `GeotagPlan`, so a KMZ bundles the same
//! track/photo pairing the rail shows. This is a simplified `log kmz`: one path
//! (a file or a directory's immediate files, dotfiles skipped), the default
//! five-minute gap, and no `--tz-offset-minutes` (a camera clock correction is
//! the rail's shift instead). The CLI's shift and scope flags are not inherited
//! here: the screen refuses to write a KMZ while a shift is armed, because the
//! KML `<TimeStamp>` would then disagree with the photo's own Exif clock.
//!
//! An empty limit means the bytes go in untouched (the CLI's absent flag); zero
//! is refused, because a zero-pixel long edge is not a limit.
//!
//! Nothing here opens a device or a network socket: the archive is assembled
//! from the track already in memory and the photos on disk. Google Earth and
//! `unzip` have not been run against this output, so the screen marks the writer
//! 未検証 rather than claiming the archive opens there.

use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use m241_core::Track;
use m241_core::log::geotag::{CaptureTime, GeotagOptions, GeotagPlan};
use m241_core::log::kmz::{KmzIconOptions, export_kmz, export_kmz_with_icons};

use crate::theme::size;

/// Long edge of the optional `icon/` thumbnail bundled with each photo.
///
/// Independent of the photo's own 最大長辺: the photo entry keeps its size, the
/// KML marker gets a small square the map can draw quickly.
const ICON_MAX_EDGE: u32 = 320;

/// The 最大長辺 field: `None` for "leave the photo alone", `Some(px)` otherwise.
///
/// # Errors
///
/// A non-numeric text and zero both return the sentence the person needs next to
/// the field, because the field is the control that was wrong.
pub(super) fn parse_max_edge(text: &str) -> Result<Option<u32>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let pixels = trimmed
        .parse::<u32>()
        .map_err(|_| format!("{trimmed:?} はピクセル数ではありません（例: 1600。空欄は原寸）"))?;
    if pixels == 0 {
        return Err(String::from(
            "最大長辺は 1 以上のピクセル数で指定してください（例: 1600。空欄は原寸）",
        ));
    }
    Ok(Some(pixels))
}

/// Draw the 最大長辺 input beside the KMZ action.
pub(super) fn max_edge_field(ui: &mut Ui, value: &mut String) {
    ui.label(
        RichText::new("最大長辺")
            .size(size::CAPTION)
            .color(crate::theme::MUTE),
    );
    let _response = ui.add(
        egui::TextEdit::singleline(value)
            .hint_text("原寸")
            .desired_width(64.0)
            .font(egui::TextStyle::Monospace),
    );
    ui.label(
        RichText::new("px")
            .size(size::CAPTION)
            .color(crate::theme::MUTE),
    );
}

/// The regular files under `path`, sorted, or `path` itself when it is a file.
///
/// A directory contributes its immediate files and skips dotfiles, the same rule
/// the photo loader uses. The planner rejects a non-JPEG with its own reason, so
/// this does not pre-filter by extension.
///
/// # Errors
///
/// The Japanese sentence for an empty path, a missing target, an unreadable
/// directory, or a directory with no files. Each names the path it failed on.
pub(super) fn collect_photos(path: &Path) -> Result<Vec<PathBuf>, String> {
    let path_text = path.to_string_lossy();
    if path_text.trim().is_empty() {
        return Err(String::from(
            "写真フォルダまたはファイルのパスを入力してください",
        ));
    }
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        return Err(format!("{} が見つかりません", path.display()));
    }
    let entries =
        std::fs::read_dir(path).map_err(|err| format!("{} を開けません: {err}", path.display()))?;
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path();
        let is_hidden = candidate
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'));
        if candidate.is_file() && !is_hidden {
            paths.push(candidate);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{} に写真ファイルがありません", path.display()));
    }
    Ok(paths)
}

/// Plan the archive's photos in the camera's clock, the way `log kmz` does.
///
/// `tz_offset_minutes` is the camera's offset from UTC: `DateTimeOriginal` has
/// no zone, so a camera set to UTC+9 would otherwise look nine hours away from
/// every point. Matching runs on a shifted copy of the track; the returned
/// plans' capture times are put back on UTC, because the archive's `<TimeStamp>`
/// must agree with the UTC track it is drawn on.
///
/// # Errors
///
/// The core planner's refusal, re-worded for the rail.
pub(super) fn plan_with_tz(
    photos: &[PathBuf],
    track: &Track,
    options: &GeotagOptions,
    tz_offset_minutes: i32,
) -> Result<Vec<GeotagPlan>, String> {
    let delta_seconds = i64::from(tz_offset_minutes) * 60;
    let mut matching_track = track.clone();
    if delta_seconds != 0 {
        for point in &mut matching_track.points {
            point.utc = point.utc.map(|utc| shift_utc(utc, delta_seconds));
        }
    }
    let mut plans = m241_core::log::geotag::plan(photos, &matching_track, options)
        .map_err(|err| format!("写真を計画できません: {err}"))?;
    if delta_seconds != 0 {
        for plan in &mut plans {
            plan.capture_time = plan
                .capture_time
                .map(|capture| shift_capture(capture, -delta_seconds));
        }
    }
    Ok(plans)
}

/// `capture` moved by `delta_seconds`, clamped like every other time shift here.
fn shift_capture(capture: CaptureTime, delta_seconds: i64) -> CaptureTime {
    match capture {
        CaptureTime::Exif(utc) => CaptureTime::Exif(shift_utc(utc, delta_seconds)),
        CaptureTime::FileModified(utc) => CaptureTime::FileModified(shift_utc(utc, delta_seconds)),
    }
}

/// A timestamp shifted by seconds, clamped to the `u32` range.
fn shift_utc(utc: u32, delta_seconds: i64) -> u32 {
    let shifted = i64::from(utc) + delta_seconds;
    u32::try_from(shifted.clamp(0, i64::from(u32::MAX))).unwrap_or(utc)
}

/// Build the archive from the plans, reading each accepted photo's bytes.
///
/// # Errors
///
/// A photo that cannot be read, or an `export_kmz` refusal (a non-finite
/// coordinate, an undecodable photo over the limit, or a ZIP32 overflow).
pub(super) fn build_archive(
    track: &Track,
    plans: &[GeotagPlan],
    max_edge: Option<u32>,
    bundle_icons: bool,
) -> Result<Vec<u8>, String> {
    let mut photos = Vec::with_capacity(plans.len());
    for plan in plans {
        let Some(mut photo) = plan.to_kmz_photo(max_edge) else {
            continue;
        };
        photo.jpeg_bytes = std::fs::read(&plan.photo)
            .map_err(|err| format!("{} を読めません: {err}", plan.photo.display()))?;
        photos.push(photo);
    }
    let archive = if bundle_icons {
        export_kmz_with_icons(
            track,
            &photos,
            &KmzIconOptions {
                max_edge_pixels: ICON_MAX_EDGE,
            },
        )
    } else {
        export_kmz(track, &photos)
    };
    archive.map_err(|err| format!("KMZ を組み立てられませんでした: {err}"))
}

/// Why photos in `plans` will not be bundled, up to three, for the status line.
///
/// The core keeps the precise reason in English on each plan; the screen's
/// voice is Japanese, so this names the two facts the person can act on — which
/// file, and whether its clock or its position was the problem — rather than
/// repeating the core sentence. A count alone (the previous message) did not say
/// what to fix.
pub(super) fn skipped_reasons(plans: &[GeotagPlan]) -> Vec<String> {
    plans
        .iter()
        .filter(|plan| !plan.will_write())
        .take(3)
        .map(|plan| {
            let name = plan.photo.file_name().map_or_else(
                || plan.photo.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let reason = if plan.capture_time.is_none() {
                String::from("撮影時刻を読めません")
            } else if let Some(gap) = plan.gap_seconds {
                format!("最も近い点と {gap} 秒差で一致しません")
            } else {
                String::from("位置のある点がありません")
            };
            format!("{name}（{reason}）")
        })
        .collect()
}
