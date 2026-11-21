//! The rail, the product's one signature move.
//!
//! The rail is the product's one signature move (DESIGN.md §4): each tab is
//! indented 8 points further than the one above, so the outer edge is a
//! staircase, and the active tab takes the panel fill and reaches the content
//! edge. This is custom painted rather than assembled from widgets because egui
//! has no stepped tab index, and because the move must appear in exactly one
//! place.

use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2};

use crate::screens::Screen;
use crate::theme::{self, INK, MUTE, PANEL, metric, size, space};

/// How far each successive tab steps to the right.
pub const STEP: f32 = 8.0;

/// The height of one tab.
pub const TAB_HEIGHT: f32 = 40.0;

/// Left inset of the shallowest tab.
pub const INSET: f32 = space::M;

/// The rail's fixed width, outer margin included (DESIGN.md §4).
///
/// Fixed rather than content-sized: the rail's width used to be whatever its
/// widest tab happened to be, which made it 164pt on 接続 and 176pt on the other
/// three screens, so the heading started at two different x positions.
pub const WIDTH: f32 = 176.0;

/// The left edge of a tab, relative to the rail.
///
/// The step is applied to the *left* edge and the right edge is pinned to the
/// rail's width. Indenting by allocating a narrower widget instead would leave
/// the left edge at zero and shrink the right edge, which reads as tabs getting
/// narrower rather than deeper — the opposite of a thumb index (DESIGN.md §4).
#[must_use]
pub fn tab_left(screen: Screen) -> f32 {
    INSET + STEP * screen.index() as f32
}

/// Draw the rail. Returns `Some(screen)` only when the selection changed this
/// frame, so the caller can cancel in-flight work for the screen it left.
#[must_use]
pub fn rail(ui: &mut Ui, current: Screen) -> Option<Screen> {
    rail_with_transition(ui, current, 0.15)
}

/// Draw the rail with an explicit tab transition duration in seconds.
///
/// Pass `0.0` when reduced motion is enabled so the active tab steps out
/// immediately without animation (DESIGN.md §3).
#[must_use]
pub fn rail_with_transition(ui: &mut Ui, current: Screen, transition_secs: f32) -> Option<Screen> {
    let mut selected = None;
    ui.add_space(space::XL);
    ui.vertical(|ui| {
        for screen in Screen::ALL {
            if tab(ui, screen, screen == current, transition_secs).clicked() && screen != current {
                selected = Some(screen);
            }
        }
    });
    selected
}

fn tab(ui: &mut Ui, screen: Screen, active: bool, transition_secs: f32) -> Response {
    // The row claims the rail's full width so every tab ends on the same right
    // edge; the tab is painted and hit-tested from its stepped left edge to that
    // right edge inside the row.
    let (row, _row) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), TAB_HEIGHT), Sense::hover());
    let progress = ui.ctx().animate_bool_with_time(
        ui.scope_id().with(("m241-rail-active", screen.index())),
        active,
        transition_secs.max(0.0),
    );
    let step_out = (space::XS * progress).round();
    let rect = Rect::from_min_max(
        pos2(row.left() + tab_left(screen) - step_out, row.top()),
        row.right_bottom(),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("m241-rail-tab", screen.index())),
        Sense::click(),
    );

    let fill = if active && progress >= 0.99 {
        PANEL
    } else if progress > 0.0 {
        PANEL.gamma_multiply(progress)
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
        egui::FontId::proportional(size::UI),
        if active { INK } else { MUTE },
    );

    // Focus ring: 2pt ink, never amber. Amber means danger, not "here".
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(metric::HAIRLINE),
            theme::RADIUS,
            Stroke::new(metric::FOCUS_RING, INK),
            StrokeKind::Inside,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_staircase_stays_inside_the_rail() {
        // The deepest tab must still start left of the rail's right edge: the tab
        // keeps its right edge pinned, so only its left edge can run out of room.
        let deepest = tab_left(Screen::ALL[Screen::ALL.len() - 1]);
        assert!(
            deepest + STEP < WIDTH,
            "deepest indent {deepest} leaves no width for a tab in a {WIDTH}pt rail"
        );
    }

    #[test]
    fn the_stairs_step_right_by_eight_points() {
        // DESIGN.md §4: each tab is indented 8pt more than the one above it, so the
        // left edges form the staircase and the right edges do not move.
        let lefts: Vec<f32> = Screen::ALL.iter().copied().map(tab_left).collect();
        assert_eq!(lefts.len(), 4, "the rail has four tabs");
        for pair in lefts.windows(2) {
            let (Some(first), Some(second)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            assert!(
                (second - first - STEP).abs() < f32::EPSILON,
                "tabs at {first} and {second} are not one step apart"
            );
        }
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
