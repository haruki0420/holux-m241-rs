//! The rail, the title bar, and the ruled section rhythm.
//!
//! The rail is the product's one signature move (DESIGN.md §4): each tab is
//! indented 8 points further than the one above, so the outer edge is a
//! staircase, and the active tab takes the panel fill and reaches the content
//! edge. This is custom painted rather than assembled from widgets because egui
//! has no stepped tab index, and because the move must appear in exactly one
//! place.

use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::screens::Screen;
use crate::theme::{self, INK, MUTE, PANEL, space};

/// How far each successive tab steps to the right.
pub const STEP: f32 = 8.0;

/// The height of one tab.
pub const TAB_HEIGHT: f32 = 40.0;

/// Left inset of the shallowest tab.
pub const INSET: f32 = space::M;

/// Draw the rail. Returns `Some(screen)` only when the selection changed this
/// frame, so the caller can cancel in-flight work for the screen it left.
#[must_use]
pub fn rail(ui: &mut Ui, current: Screen) -> Option<Screen> {
    let mut selected = None;
    ui.add_space(space::XL);
    ui.vertical(|ui| {
        for screen in Screen::ALL {
            if tab(ui, screen, screen == current).clicked() && screen != current {
                selected = Some(screen);
            }
        }
    });
    selected
}

fn tab(ui: &mut Ui, screen: Screen, active: bool) -> Response {
    let indent = INSET + STEP * screen.index() as f32;
    let width = (ui.available_width() - indent).max(0.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, TAB_HEIGHT), Sense::click());

    let fill = if active {
        PANEL
    } else if response.hovered() {
        theme::PANEL.gamma_multiply(0.6)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, theme::RADIUS, fill);

    ui.painter().text(
        rect.left_top() + Vec2::new(space::L, space::M),
        egui::Align2::LEFT_TOP,
        screen.label(),
        egui::FontId::proportional(13.0),
        if active { INK } else { MUTE },
    );

    // Focus ring: 2pt ink, never amber. Amber means danger, not "here".
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            theme::RADIUS,
            Stroke::new(2.0, INK),
            StrokeKind::Inside,
        );
    }
    response
}

/// A ruled section: caption-sized heading, hairline, body.
///
/// The manual's structure, transferred: a rule under a heading, figures
/// right-aligned. Used by every screen so the rhythm is identical.
pub fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(space::M);
    ui.label(egui::RichText::new(title).size(11.0).color(MUTE));
    ui.add_space(space::XS);
    let rule = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 1.0));
    ui.painter().rect_filled(rule, 0.0, theme::RULE);
    ui.add_space(space::M);
    body(ui);
}

/// A label/value row, the manual's figure line: label left, value right-aligned.
pub fn figure(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).color(MUTE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(13.0).color(INK).monospace());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_staircase_stays_inside_the_rail() {
        // The deepest tab plus its inset must leave room for content: the rail
        // panel is sized by its children, so a tab that overflows would widen it.
        let deepest = INSET + STEP * (Screen::ALL.len() - 1) as f32;
        assert!(
            deepest < 96.0,
            "deepest indent {deepest} overflows a 96pt rail"
        );
    }

    #[test]
    fn screens_are_in_rail_order() {
        // Written as the expected order rather than derived from `index()`: the
        // previous form re-ran the implementation and could not fail when `ALL`
        // was reordered. This is the requirement, spelled out.
        assert_eq!(
            Screen::ALL,
            [
                Screen::Connect,
                Screen::Log,
                Screen::Firmware,
                Screen::Settings
            ]
        );
    }

    #[test]
    fn each_screen_knows_its_rail_position() {
        // The staircase is drawn from `index()`; a variant that lands on the
        // wrong step draws on top of its neighbour. Literals, not a search.
        assert_eq!(Screen::Connect.index(), 0);
        assert_eq!(Screen::Log.index(), 1);
        assert_eq!(Screen::Firmware.index(), 2);
        assert_eq!(Screen::Settings.index(), 3);
    }

    #[test]
    fn every_screen_has_a_label() {
        for screen in Screen::ALL {
            assert!(!screen.label().is_empty());
        }
    }
}
