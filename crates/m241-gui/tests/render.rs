//! Render every screen through `egui`'s headless context.
//!
//! A GUI that compiles is not a GUI that works: an immediate-mode layout can
//! panic on an empty rectangle, overflow a fixed-width rail, or paint nothing at
//! all. These tests run the real [`m241_gui::App`] through a `Context` with no
//! display server and assert on the primitives it produced.
//!
//! They are not screenshots. Judging the *look* needs a real capture; judging
//! that every screen draws something and none of them panics does not.

use egui::{Context, RawInput};
use m241_gui::screens::Screen;
use m241_gui::{App, theme};

/// Drive one full pass and return the number of tessellated shapes.
fn render(ctx: &Context, app: &mut App) -> usize {
    let output = ctx.run_ui(RawInput::default(), |ui| {
        app.draw(ui);
    });
    let shapes = output.shapes.len();
    output.drop_without_applying_deltas();
    shapes
}

fn context() -> Context {
    let ctx = Context::default();
    theme::install(&ctx);
    ctx
}

/// The style egui would use for the dark and the light theme right now.
///
/// `theme::install` writes both, because the product is dark-only and a light
/// OS theme must not produce a light widget set.
fn styles(ctx: &Context) -> (egui::Style, egui::Style) {
    (
        (*ctx.style_of(egui::Theme::Dark)).clone(),
        (*ctx.style_of(egui::Theme::Light)).clone(),
    )
}

#[test]
fn the_shell_draws_a_frame() {
    let ctx = context();
    let mut app = App::default();
    assert!(render(&ctx, &mut app) > 0, "the shell drew nothing");
}

#[test]
fn every_screen_draws_something() {
    let ctx = context();
    for screen in Screen::ALL {
        let mut app = App::default();
        app.select(screen);
        let shapes = render(&ctx, &mut app);
        assert!(shapes > 0, "{} drew nothing", screen.label());
    }
}

#[test]
fn rendering_the_same_screen_twice_is_stable() {
    let ctx = context();
    let mut app = App::default();
    let first = render(&ctx, &mut app);
    let second = render(&ctx, &mut app);
    // Immediate mode is allowed to differ slightly between passes as layout
    // settles, but a second pass that draws nothing means the first was an
    // artefact rather than a screen.
    assert!(second > 0, "second pass drew nothing (first drew {first})");
}

#[test]
fn the_selected_screen_survives_a_repaint() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    let _ = render(&ctx, &mut app);
    assert_eq!(app.current(), Screen::Firmware);
}

#[test]
fn installing_the_theme_twice_is_idempotent() {
    let ctx = Context::default();
    let before = styles(&ctx);

    theme::install(&ctx);
    let once = styles(&ctx);
    assert_ne!(
        before, once,
        "install changed nothing; the idempotency check would pass vacuously"
    );

    theme::install(&ctx);
    let twice = styles(&ctx);
    assert_eq!(
        once, twice,
        "a second install changed the style; the tokens accumulate instead of being set"
    );

    // Both theme slots carry the token set: egui stores one style per theme, and
    // a light OS theme must not hand back egui's light widget set.
    let expected = theme::text_styles().len();
    for (name, style) in [("dark", &once.0), ("light", &once.1)] {
        assert_eq!(
            style.text_styles.len(),
            expected,
            "{name} theme has {} text styles, the token set has {expected}",
            style.text_styles.len()
        );
        assert_eq!(
            style.visuals.panel_fill,
            theme::GROUND,
            "{name} theme kept egui's panel fill instead of theme::GROUND"
        );
        assert!(
            style.visuals.dark_mode,
            "{name} theme is not dark; the product ships dark only"
        );
    }
}
