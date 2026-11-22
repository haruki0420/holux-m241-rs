//! KMZ writing for the Log screen (DESIGN.md §5's `[ KMZ ]` button).
//!
//! The CLI's `log kmz` semantics are reused rather than re-invented: photos are
//! matched to the track by capture time through the same `GeotagPlan`, and
//! `--max-photo-edge` is this screen's 最大長辺. An empty limit means the bytes go
//! in untouched (the CLI's absent flag); zero is refused, because a zero-pixel
//! long edge is not a limit.
//!
//! Nothing here opens a device or a network socket: the archive is assembled
//! from the track already in memory and the photos on disk.

use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use m241_core::Track;
use m241_core::log::geotag::GeotagPlan;
use m241_core::log::kmz::export_kmz;

use crate::theme::size;

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
        .map_err(|_| format!("{trimmed:?} はピクセル数ではありません"))?;
    if pixels == 0 {
        return Err(String::from(
            "最大長辺は 1 以上のピクセル数で指定してください（空欄は上限なし）",
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
/// the photo loader and the CLI use. The planner rejects a non-JPEG with its own
/// reason, so this does not pre-filter by extension.
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

/// Build the archive from the plans, reading each accepted photo's bytes.
///
/// Returns the archive with the number of embedded photos and the number the
/// planner left out. The reasons are on each plan (`GeotagPlan::skipped`) and the
/// caller reports the count rather than guessing why.
///
/// # Errors
///
/// A photo that cannot be read, or an `export_kmz` refusal (a non-finite
/// coordinate, an undecodable photo over the limit, or a ZIP32 overflow).
pub(super) fn build_archive(
    track: &Track,
    plans: &[GeotagPlan],
    max_edge: Option<u32>,
) -> Result<(Vec<u8>, usize, usize), String> {
    let mut photos = Vec::with_capacity(plans.len());
    let mut skipped = 0_usize;
    for plan in plans {
        let Some(mut photo) = plan.to_kmz_photo(max_edge) else {
            skipped += 1;
            continue;
        };
        photo.jpeg_bytes = std::fs::read(&plan.photo)
            .map_err(|err| format!("{} を読めません: {err}", plan.photo.display()))?;
        photos.push(photo);
    }
    let archive = export_kmz(track, &photos)
        .map_err(|err| format!("KMZ を組み立てられませんでした: {err}"))?;
    Ok((archive, photos.len(), skipped))
}
