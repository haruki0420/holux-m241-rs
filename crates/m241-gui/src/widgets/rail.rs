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

/// The left edge of a tab once its selection offset is applied, relative to the rail.
///
/// `progress` is the tab's animated selection value (0 = off the staircase,
/// 1 = fully selected). Kept as a pure function so the 4px move can be asserted
/// without painting.
#[must_use]
pub fn tab_left_at(screen: Screen, progress: f32) -> f32 {
    tab_left(screen) - active_step_out(progress)
}

/// How far the active tab steps out of the staircase while it is selected.
///
/// It is this product's one authored motion (DESIGN.md §3): 4 px, on the left
/// edge only, because the right edge stays pinned to the content.
pub const ACTIVE_STEP_OUT: f32 = space::XS;

/// Draw the rail. Returns `Some(screen)` only when the selection changed this
/// frame, so the caller can cancel in-flight work for the screen it left.
#[must_use]
pub fn rail(ui: &mut Ui, current: Screen) -> Option<Screen> {
    rail_with_transition(
        ui,
        current,
        crate::screens::settings::AppSettings::DEFAULT_RAIL_TRANSITION_SECS,
    )
}

/// The horizontal movement a tab makes when its selection changes.
///
/// `progress` is 0 for an unselected tab and 1 for the selected one; it is the
/// *selected* tab that moves, from [`tab_left`] to [`tab_left`] −
/// [`ACTIVE_STEP_OUT`]. A tab that has just been left moves the other way, from
/// the protruded edge back onto the staircase.
#[must_use]
pub fn active_step_out(progress: f32) -> f32 {
    (ACTIVE_STEP_OUT * progress).round()
}

/// Draw the rail with an explicit tab transition duration in seconds.
///
/// `transition_secs` is how long a tab takes to slide between the staircase and
/// the protruded edge — the *outgoing* and *incoming* tab both use it, so a
/// selection swap is one movement rather than a jump.
///
/// Pass `0.0` when reduced motion is enabled: DESIGN.md §3 says the tab keeps
/// its 4px offset as a static state and only the transition goes away, so the
/// offset is applied at once instead of being animated.
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
        // egui rejects a negative animation time; the caller passing a negative
        // number of seconds means "none", not "go backwards".
        transition_secs.max(0.0),
    );
    let rect = Rect::from_min_max(
        pos2(row.left() + tab_left_at(screen, progress), row.top()),
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

    #[test]
    fn the_selected_tab_steps_out_by_four_points_and_the_others_do_not() {
        // DESIGN.md §3: the one authored motion is the active tab stepping out by
        // 4px. Written as a literal, so a 0 (no protrusion) or a 2 (two XS steps)
        // fails here rather than in a screenshot review.
        let unselected = tab_left_at(Screen::Log, 0.0);
        let selected = tab_left_at(Screen::Log, 1.0);
        assert!(
            (unselected - selected - 4.0).abs() < f32::EPSILON,
            "a selected tab moved {:.2}pt, expected 4pt",
            unselected - selected
        );
        // Reduced motion keeps the same static offset: it removes the transition,
        // not the protrusion (DESIGN.md §3).
        assert!((tab_left_at(Screen::Log, 0.0) - tab_left(Screen::Log)).abs() < f32::EPSILON);
        assert_eq!(active_step_out(1.0), 4.0);
        assert_eq!(active_step_out(0.0), 0.0);
    }
}
