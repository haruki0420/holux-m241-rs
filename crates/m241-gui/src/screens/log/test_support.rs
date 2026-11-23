//! Test-only fixtures shared by the `log` submodules.
//!
//! The core's dump builders are `#[cfg(test)]` inside `m241-core`, so a GUI test
//! cannot reuse them. [`synthetic_dump`] builds the smallest valid 64 KiB block
//! `m241_core::log::parse_dump` reads; the points are arithmetic, never a
//! recording.
//!
//! [`screen_texts`] renders one frame of a [`super::LogScreen`] and returns the
//! text it laid out, so a test can check that a control a sentence names is
//! actually drawn — a property an image colour count cannot answer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use egui::Vec2;

use super::{LogScreen, LogTab};
use crate::screens::settings::AppSettings;

/// A 64 KiB dump holding `points` as `(utc, latitude, longitude, speed)`.
pub fn synthetic_dump(points: &[(u32, f32, f32, f32)]) -> Vec<u8> {
    use m241_core::log::block::{
        BLOCK_HEADER_SIZE, BLOCK_SIZE, HOLUX_EXTRA_BIT, POLICY_LOG_ENABLED, bit,
    };
    let mask = HOLUX_EXTRA_BIT
        | (1u32 << u32::from(bit::UTC))
        | (1u32 << u32::from(bit::LATITUDE))
        | (1u32 << u32::from(bit::LONGITUDE))
        | (1u32 << u32::from(bit::HEIGHT))
        | (1u32 << u32::from(bit::SPEED));
    let mut block = vec![0xFFu8; BLOCK_SIZE];
    let count = points.len() as u16;
    block[0..2].copy_from_slice(&count.to_le_bytes());
    block[2..6].copy_from_slice(&mask.to_le_bytes());
    block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());

    let mut offset = BLOCK_HEADER_SIZE;
    for (utc, lat, lon, speed) in points {
        let mut record = Vec::with_capacity(20);
        record.extend_from_slice(&utc.to_le_bytes());
        record.extend_from_slice(&lat.to_le_bytes());
        record.extend_from_slice(&lon.to_le_bytes());
        record.extend_from_slice(&[0x00, 0x28, 0x42]); // height 42.0 m
        record.extend_from_slice(&speed.to_le_bytes());
        let checksum = record.iter().fold(0u8, |acc, byte| acc ^ byte);
        record.push(checksum);
        block[offset..offset + record.len()].copy_from_slice(&record);
        offset += record.len();
    }
    block
}

/// A unique temporary directory, removed when it falls out of scope.
#[derive(Debug)]
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a scratch directory labelled `tag`.
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "m241-gui-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self(path)
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every text run the screen tessellated at 1280×800, with its position.
///
/// Two passes settle the immediate-mode layout; the collection ignores clip
/// rectangles, so a line the body would let a person scroll to still counts
/// as drawn. That is the right question for "is the refusal on screen rather
/// than only in a field nobody sees".
pub fn screen_text_runs(screen: &mut LogScreen) -> Vec<(String, egui::Pos2)> {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::new(1280.0, 800.0),
        )),
        ..egui::RawInput::default()
    };
    let mut runs = Vec::new();
    for _pass in 0..2 {
        let output = ctx.run_ui(input.clone(), |ui| {
            screen.ui_with_settings(ui, &AppSettings::default());
        });
        runs.clear();
        collect_text_runs(&output.shapes, &mut runs);
        output.drop_without_applying_deltas();
    }
    runs
}

/// Every text run the screen laid out, without positions.
pub fn screen_texts(screen: &mut LogScreen) -> Vec<String> {
    screen_text_runs(screen)
        .into_iter()
        .map(|(text, _pos)| text)
        .collect()
}

/// Every text run the screen tessellated with the drawer open on `tab`.
///
/// The panels live in the drawer (DESIGN.md §5), so a test that asks "is this
/// control drawn" has to say which tab draws it. Rendering with the drawer
/// closed answers a different question — whether the canvas is uncluttered —
/// and would make every panel assertion fail for the wrong reason.
pub fn screen_text_runs_on(screen: &mut LogScreen, tab: LogTab) -> Vec<(String, egui::Pos2)> {
    screen.open_drawer(tab);
    screen_text_runs(screen)
}

/// The same, without positions.
pub fn screen_texts_on(screen: &mut LogScreen, tab: LogTab) -> Vec<String> {
    screen_text_runs_on(screen, tab)
        .into_iter()
        .map(|(text, _pos)| text)
        .collect()
}

fn collect_text_runs(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<(String, egui::Pos2)>) {
    for clipped in shapes {
        collect_shape_texts(&clipped.shape, out);
    }
}

fn collect_shape_texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(text) => out.push((text.galley.job.text.clone(), text.pos)),
        egui::Shape::Vec(children) => {
            for child in children {
                collect_shape_texts(child, out);
            }
        }
        _ => {}
    }
}
