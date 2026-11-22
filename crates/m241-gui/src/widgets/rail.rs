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

/// The rail's transition when motion is allowed (DESIGN.md §3: 150 ms).
pub const TRANSITION_MOTION_SECS: f32 = 0.15;

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

/// The transition duration the shell hands to [`rail_with_transition`].
///
/// The shell could inline `app.settings().rail_transition_secs()` into its
/// `rail_with_transition` call, and then the only thing connecting the setting
/// to the rail is the argument at that one call site — which is what the Stage 3
/// review found untested (hard-coding 0.15 there left every test green; N5).
/// Naming the derivation here gives the shell and its test the same function.
#[doc(hidden)]
#[must_use]
pub fn rail_transition_secs_for(app: &crate::App) -> f32 {
    app.settings().rail_transition_secs()
}

/// The last `transition_secs` [`rail_with_transition`] was actually called with,
/// for the calling thread.
///
/// Test-side recording of the real argument: a test cannot see the shell's call
/// stack, so the rail writes down what it was handed. It answers "what did the
/// shell pass", which `rail_transition_secs_for` alone cannot — a hard-coded
/// literal at the call site leaves that function correct and unused. Keyed by
/// thread id because the render tests run in parallel and call the rail too.
#[cfg(test)]
pub(crate) fn last_transition_secs() -> Option<f32> {
    let me = format!("{:?}", std::thread::current().id());
    LAST_TRANSITION_SECS
        .lock()
        .ok()
        .and_then(|records| records.get(&me).copied())
}

#[cfg(test)]
pub(crate) fn record_transition_secs(secs: f32) {
    if let Ok(mut records) = LAST_TRANSITION_SECS.lock() {
        let me = format!("{:?}", std::thread::current().id());
        let _previous = records.insert(me, secs);
    }
}

#[cfg(test)]
static LAST_TRANSITION_SECS: std::sync::Mutex<std::collections::BTreeMap<String, f32>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

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
    #[cfg(test)]
    record_transition_secs(transition_secs);
    let mut selected = None;
    ui.add_space(space::XL);
    ui.vertical(|ui| {
        for screen in Screen::ALL {
            // The caller's duration is used here, once; `tab` only paints at the
            // progress that animation reached. The curve is ease-out (§3): most of
            // the 4pt move happens early and the tab settles instead of arriving
            // at a constant speed, which is what a manual's tab would do.
            let animation_id = ui.scope_id().with(("m241-rail-active", screen.index()));
            let progress = ui.ctx().animate_bool_with_time_and_easing(
                animation_id,
                screen == current,
                // egui rejects a negative animation time; the caller passing a
                // negative number of seconds means "none", not "go backwards".
                transition_secs.max(0.0),
                egui::emath::easing::quadratic_out,
            );
            if tab(ui, screen, progress).clicked() && screen != current {
                selected = Some(screen);
            }
        }
    });
    selected
}

/// One tab, painted at the selection progress the caller's animation reached.
///
/// Split from [`rail_with_transition`] so the drawing can be measured in a test
/// while keeping the animation in one place: the progress is an argument, and
/// nothing here can re-create the animation with a different duration.
fn tab(ui: &mut Ui, screen: Screen, progress: f32) -> Response {
    // The row claims the rail's full width so every tab ends on the same right
    // edge; the tab is painted and hit-tested from its stepped left edge to that
    // right edge inside the row.
    let (row, _row) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), TAB_HEIGHT), Sense::hover());
    let rect = Rect::from_min_max(
        pos2(row.left() + tab_left_at(screen, progress), row.top()),
        row.right_bottom(),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("m241-rail-tab", screen.index())),
        Sense::click(),
    );

    let fill = if progress >= 0.99 {
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
        if progress >= 0.99 { INK } else { MUTE },
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

    /// Run eight rail passes, moving the selection from `from` to `to` after two
    /// frames, and return the x where `label`'s tab was painted in `read_pass`.
    ///
    /// Measured from the primitives, not from `tab_left_at`: the point of the
    /// test is the duration and the curve the *caller* passed, so re-reading the
    /// pure function would prove nothing about the wiring. `read_pass` is an
    /// argument because a position read late enough cannot tell easing curves
    /// apart once the 4pt offset has been rounded.
    fn measure_labelled_tab(
        label: &str,
        seconds: f32,
        from: Screen,
        to: Option<Screen>,
        read_pass: usize,
    ) -> f32 {
        let ctx = egui::Context::default();
        let mut time = 0.0_f64;
        let mut left = f32::NAN;
        for pass in 0..8 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(WIDTH, 400.0),
                )),
                time: Some(time),
                // A real frame clock: egui only advances an animation when the
                // time between frames is a positive fraction of a second, and
                // moving back onto the same target frame by frame keeps the
                // animation running the way a selection change does.
                predicted_dt: 1.0 / 60.0,
                ..egui::RawInput::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let current = if pass < 2 { from } else { to.unwrap_or(from) };
                let _selected = rail_with_transition(ui, current, seconds);
            });
            if pass == read_pass {
                for clipped in &output.shapes {
                    if let egui::Shape::Text(text) = &clipped.shape
                        && text.galley.job.text == label
                    {
                        left = text.pos.x;
                    }
                }
            }
            output.drop_without_applying_deltas();
            time += 1.0 / 60.0;
        }
        left
    }

    #[test]
    fn the_tab_slides_at_the_transition_time_the_caller_passed() {
        // DESIGN.md §3: 150 ms by default, and reduced motion drops the duration
        // to 0 without dropping the 4pt offset. Frames are driven at the real
        // rate and 「ログ」 is measured where it was painted four frames in
        // (~67 ms, a third of 150 ms). Ease-out is already past halfway there, so
        // the tab is still between the two ends; at 0 s it is home at once.
        // (Seven frames was the old probe; ease-out is nearly home by then, so
        // the curve would decide the result instead of the duration.)
        let moving = measure_labelled_tab(
            Screen::Log.label(),
            0.15,
            Screen::Connect,
            Some(Screen::Log),
            4,
        );
        let at_once = measure_labelled_tab(
            Screen::Log.label(),
            0.0,
            Screen::Connect,
            Some(Screen::Log),
            4,
        );
        // Text is drawn at the tab's left edge plus `space::L`.
        let home = tab_left(Screen::Log) - ACTIVE_STEP_OUT + space::L;
        let seated = tab_left(Screen::Log) + space::L;
        assert!(
            (at_once - home).abs() < 0.5,
            "with a 0 s transition the selected tab painted at {at_once}, expected {home}"
        );
        assert!(
            (moving - home).abs() > 0.05 && (moving - seated).abs() > 0.05,
            "a 0.15 s transition finished within four frames: it painted at {moving}, \
             home is {home} and the staircase is {seated}"
        );
    }

    #[test]
    fn the_tab_eases_out_rather_than_moving_linearly() {
        // §3: the transition is ease-out. A third of the way through, linear
        // would have moved round(4 × 1/3) = 1pt; quadratic-out has moved
        // round(4 × 0.56) = 2pt. Those integers are far enough apart to tell the
        // curves apart, which a read later in the animation cannot.
        let moving = measure_labelled_tab(
            Screen::Log.label(),
            0.15,
            Screen::Connect,
            Some(Screen::Log),
            4,
        );
        let seated = tab_left(Screen::Log) + space::L;
        let offset = seated - moving;
        assert!(
            offset > 1.5,
            "at a third of the transition the tab had moved {offset}pt; a linear \
             animation moves 1pt and an ease-out more than 2pt"
        );
        assert!(
            offset < ACTIVE_STEP_OUT,
            "the tab was already home at a third of the transition ({offset}pt)"
        );
    }

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
