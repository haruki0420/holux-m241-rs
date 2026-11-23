//! Render every screen through `egui`'s headless context.
//!
//! A GUI that compiles is not a GUI that works: an immediate-mode layout can
//! panic on an empty rectangle, overflow a fixed-width rail, or paint nothing at
//! all. These tests run the real [`m241_gui::App`] through a `Context` with no
//! display server and assert on the primitives it produced.
//!
//! They are not screenshots. Judging the *look* needs a real capture; judging
//! that every screen draws something and none of them panics does not.
//!
//! The per-screen cases drive the states DESIGN.md §6 requires the build to ship —
//! an empty log, a damaged log, an unknown device, a refused write — because those
//! are the states that only appear after something has gone wrong, and therefore the
//! ones a hand-written test pass never reaches.

// An integration test is its own crate, so the workspace's panic-free rule for
// library code does not apply here: a `expect` in a test is how the failure gets a
// sentence instead of a backtrace.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::{Context, RawInput};
use m241_core::log::ParseReport;
use m241_core::{LogSummary, Track, TrackPoint};
use m241_gui::screens::Screen;
use m241_gui::screens::firmware::{DumpState, Step};
use m241_gui::widgets::rail;
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

/// Drive one pass at a chosen window size, so the narrow layout is exercised too.
fn render_at(ctx: &Context, app: &mut App, width: f32, height: f32) -> usize {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..RawInput::default()
    };
    let output = ctx.run_ui(input, |ui| {
        app.draw(ui);
    });
    let shapes = output.shapes.len();
    output.drop_without_applying_deltas();
    shapes
}

/// Drive two passes so immediate-mode layout settles, returning the second pass's shapes.
fn render_shapes_at(ctx: &Context, app: &mut App, width: f32, height: f32) -> Vec<egui::Shape> {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..RawInput::default()
    };
    let first = ctx.run_ui(input.clone(), |ui| {
        app.draw(ui);
    });
    first.drop_without_applying_deltas();
    let second = ctx.run_ui(input, |ui| {
        app.draw(ui);
    });
    let shapes = second
        .shapes
        .iter()
        .map(|clipped| clipped.shape.clone())
        .collect();
    second.drop_without_applying_deltas();
    shapes
}

/// Flatten nested `Shape::Vec` trees into a single slice of leaf shapes.
fn flatten_shapes<'a>(shapes: &'a [egui::Shape], out: &mut Vec<&'a egui::Shape>) {
    for shape in shapes {
        match shape {
            egui::Shape::Vec(children) => flatten_shapes(children, out),
            other => out.push(other),
        }
    }
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

/// A track with fixes, one waypoint, and one record without a position.
///
/// Hand-built rather than parsed: the render tests are about the layout surviving
/// the shapes of real data, and a dump fixture would make a failure harder to read.
fn mixed_track() -> Track {
    let mut points = Vec::new();
    // A record with a time but no fix: the unit was recording without a lock.
    points.push(TrackPoint {
        utc: Some(1_763_000_000),
        ..TrackPoint::default()
    });
    for step in 0..40_u32 {
        let angle = f64::from(step) * 0.1;
        points.push(TrackPoint {
            utc: Some(1_763_000_100 + step * 30),
            latitude: Some(35.681_236 + angle.sin() * 0.01),
            longitude: Some(139.767_125 + angle.cos() * 0.01),
            height_m: Some(40.0),
            speed_kmh: Some(18.0 + (angle * 2.0).sin() as f32 * 6.0),
            rcr: Some(if step == 7 { 0x0008 } else { 0 }),
        });
    }
    Track {
        points,
        damaged_blocks: vec![3],
    }
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
            theme::WINDOW,
            "{name} theme kept egui's panel fill instead of theme::WINDOW"
        );
        assert!(
            style.visuals.dark_mode,
            "{name} theme is not dark; the product ships dark only"
        );
    }
}

#[test]
fn a_fresh_connect_screen_draws_the_no_port_state() {
    // On a machine with no logger attached this is the state every first run shows,
    // so it is the one that must not be a blank panel.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Connect);
    assert!(render(&ctx, &mut app) > 0);
    let _ports = app.connect_mut();
}

#[test]
fn the_connect_screen_draws_its_sample() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Connect);
    app.connect_mut().show_sample();
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_draws_an_empty_log() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_draws_a_track_and_a_pickable_point() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut()
        .set_track(mixed_track(), ParseReport::default());
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_survives_a_damaged_track() {
    // A dump with failed record checksums is a normal dump, not an error: the
    // records that pass stay in the track, the failed ones are dropped, and the
    // damage is reported. The screen must render the warning rather than panic on
    // the report.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().set_track(
        mixed_track(),
        ParseReport {
            blocks: 4,
            damaged_blocks: 2,
            records: 41,
            bad_checksums: 3,
            truncated_records: 1,
            ..ParseReport::default()
        },
    );
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_survives_a_track_without_fixes_or_speeds() {
    // Every field is `Option` because the block header decides what a record carries.
    // The all-`None` track is the extreme case, and it is the one that produces a NaN
    // in a naïve projection or a division by zero in a speed chart.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().set_track(
        Track {
            points: vec![
                TrackPoint::default(),
                TrackPoint {
                    utc: Some(1_700_000_000),
                    ..TrackPoint::default()
                },
            ],
            damaged_blocks: Vec::new(),
        },
        ParseReport::default(),
    );
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_survives_a_single_fix() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().set_track(
        Track {
            points: vec![TrackPoint {
                utc: Some(1_700_000_000),
                latitude: Some(35.0),
                longitude: Some(139.0),
                height_m: Some(10.0),
                speed_kmh: Some(0.0),
                rcr: Some(0),
            }],
            damaged_blocks: Vec::new(),
        },
        ParseReport::default(),
    );
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_log_screen_collapses_at_a_narrow_width() {
    // DESIGN.md §9 leaves the breakpoint open and names the rule: below
    // `metric::NARROW` (1000pt of content) the drawer floats over the canvas. Both
    // widths are rendered so a layout that only works wide fails here rather than
    // in a screenshot review.
    let ctx = context();
    for (width, height) in [(1280.0, 800.0), (1024.0, 640.0)] {
        let mut app = App::default();
        app.select(Screen::Log);
        app.log_mut().show_sample();
        let shapes = render_at(&ctx, &mut app, width, height);
        assert!(
            shapes > 0,
            "the log screen drew nothing at {width}×{height}"
        );
    }
}

#[test]
fn the_firmware_screen_draws_every_dump_state() {
    // The two states that matter most are the ones a passing test would never reach:
    // the power-cycle prompt and the two-hash mismatch.
    let ctx = context();
    for state in [
        DumpState::Idle,
        DumpState::FirstPass {
            read: 214_880,
            total: 406_620,
        },
        DumpState::WaitingForPowerCycle {
            total: 406_620,
            digest: String::from(
                "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
            ),
        },
        DumpState::Verified {
            bytes: 406_620,
            digest: String::from(
                "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
            ),
        },
        DumpState::Mismatch {
            first: String::from("32fe5269"),
            second: String::from("32fe5270"),
        },
        DumpState::Failed(String::from("the device did not answer")),
    ] {
        let mut app = App::default();
        app.select(Screen::Firmware);
        app.firmware_mut().set_step(Step::Dump);
        app.firmware_mut().set_dump_state(state.clone());
        let shapes = render(&ctx, &mut app);
        assert!(shapes > 0, "{state:?} drew nothing");
    }
}

#[test]
fn waiting_for_the_power_cycle_holds_the_progress_bar_at_one_hundred_percent() {
    // DESIGN.md §6, "Waiting for the user": "Progress held at 100%, the
    // instruction given as an imperative". The shipped screen drew the state line
    // and the instruction but no bar, and the old test only inspected a local
    // string — this reads the bar that reached the frame.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    app.firmware_mut().set_step(Step::Dump);
    app.firmware_mut()
        .set_dump_state(DumpState::WaitingForPowerCycle {
            total: 406_620,
            digest: String::from(
                "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
            ),
        });

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);

    // A progress bar is a track rect in the extreme background colour with a
    // same-origin fill rect on top in another colour; the fill's width is the
    // progress. Finding the pair geometrically keeps a single-line text field,
    // which also uses the extreme background, out of the match.
    let (track, fill) = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Rect(rect) if rect.fill == theme::SUNKEN && rect.rect.width() > 200.0 => {
                Some(rect.rect)
            }
            _ => None,
        })
        .find_map(|track| {
            flat.iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill != theme::SUNKEN
                            && (rect.rect.min - track.min).length() < 0.5
                            && (rect.rect.height() - track.height()).abs() < 0.5 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .next()
                .map(|fill| (track, fill))
        })
        .expect("the waiting state drew no progress bar");

    assert!(
        (fill.width() - track.width()).abs() <= 1.0,
        "the waiting bar is {:.1}% full ({:.1} of {:.1}pt); §6 holds it at 100%",
        100.0 * fill.width() / track.width(),
        fill.width(),
        track.width()
    );

    // The instruction is part of the same state, and must be on the frame rather
    // than only in the source: an imperative that the layout dropped is not shown.
    assert!(
        flat.iter().any(|shape| matches!(
            shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("電池を抜いて")
        )),
        "the waiting state printed no power-cycle instruction"
    );
}

#[test]
fn an_empty_log_says_there_is_no_log_yet() {
    // DESIGN.md §6 names the sentence. The old render test only asserted that the
    // screen drew something, so a blank panel would have passed.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    assert!(
        flat.iter().any(|shape| matches!(
            shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("まだログがありません")
        )),
        "the empty log did not say まだログがありません"
    );
}

#[test]
fn a_sample_map_does_not_credit_tiles_it_does_not_show() {
    // The OSM licence requires the attribution where OSM content is shown. A
    // sample frame draws no tile, so crediting OSM there would be a false
    // statement about where the pixels came from. The two tests this replaces
    // asserted the credit and "shows no tiles" on the same frame, which made the
    // contradiction itself the passing contract.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let texts: Vec<&str> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("© OpenStreetMap contributors")),
        "a sample frame credited OSM tiles it did not draw: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("見本：地図タイルは表示しません")),
        "the sample map did not say it shows no tiles: {texts:?}"
    );
}

#[test]
fn a_sample_map_says_it_shows_no_tiles() {
    // §3 makes the map the one light surface; §10 records that the screenshot is
    // produced offline. The note is how a reader knows the dark canvas is the
    // sample frame rather than a broken tile fetch.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    assert!(
        flat.iter().any(|shape| matches!(
            shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("見本：地図タイルは表示しません")
        )),
        "the sample map did not say it shows no tiles"
    );
}

#[test]
fn a_headless_render_says_why_it_shows_no_tiles() {
    // The screenshot renderer and every test use a headless context. The map
    // must name that reason rather than sit on "loading" forever, and it must
    // not credit tiles it cannot draw. The fetch count itself is asserted in
    // `App`'s unit test, where a fake fetcher is injected: this frame-level test
    // alone cannot tell a fetch from no fetch.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut()
        .set_track(mixed_track(), ParseReport::default());
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let texts: Vec<&str> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("地図タイルは表示しません（ヘッドレス）")),
        "the headless map did not explain its state: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("© OpenStreetMap contributors")),
        "the headless map credited tiles it did not draw: {texts:?}"
    );
}

#[test]
fn the_settings_screen_names_the_cache_location_and_the_patch_offset() {
    // DESIGN.md §5's settings row list is size *and* location; §5 also requires
    // the colophon's figures to be readable. The offset must be the one
    // `m241-core` measures, not a second literal.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Settings);
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let texts: Vec<&str> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"地図キャッシュの場所"),
        "the settings table has no cache location row: {texts:?}"
    );
    let offset = format!(
        "0x{:05X}",
        m241_core::firmware::patch::EFFECTIVE_EPOCH_OFFSET
    );
    assert!(
        texts.contains(&"修正する位置"),
        "the colophon has no patch-offset row: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains(&offset)),
        "the colophon does not name the core patch offset {offset}: {texts:?}"
    );
}

#[test]
fn the_firmware_screen_draws_the_plan_and_the_write_step() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    app.firmware_mut()
        .show_sample()
        .expect("the sample image must be recognised");
    assert!(render(&ctx, &mut app) > 0);

    app.firmware_mut().set_step(Step::Flash);
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn the_firmware_screen_draws_the_undefined_device_state() {
    // The amber recovery panel (DESIGN.md §6). Its whole purpose is to appear when
    // the device stopped answering, so nothing else on the screen may depend on a
    // working device for it to render.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    app.firmware_mut().set_undefined(true);
    assert!(render(&ctx, &mut app) > 0);
}

#[test]
fn no_screen_panics_with_every_sample_loaded() {
    // The three screens at once, each carrying its largest state, then repainted:
    // layout that only survives one pass is layout that will jitter on the second.
    let ctx = context();
    let mut app = App::default();
    app.connect_mut().show_sample();
    app.log_mut().show_sample();
    app.firmware_mut()
        .show_sample()
        .expect("the sample image must be recognised");
    app.firmware_mut().set_step(Step::Flash);
    for screen in Screen::ALL {
        app.select(screen);
        let first = render(&ctx, &mut app);
        let second = render(&ctx, &mut app);
        assert!(
            first > 0 && second > 0,
            "{} stopped drawing",
            screen.label()
        );
    }
}

#[test]
fn a_log_summary_matches_the_track_it_describes() {
    // The screen prints the summary; if the two ever disagree the Log screen would
    // report a figure the table below it contradicts.
    let track = mixed_track();
    let summary = LogSummary::of(&track);
    assert_eq!(summary.records, track.points.len());
    assert_eq!(summary.fixes, track.fix_count());
    assert_eq!(summary.damaged_block_count, track.damaged_blocks.len());
}

#[test]
fn amber_appears_in_the_write_step_and_nowhere_else() {
    // The design invariant in pixels rather than in source. DESIGN.md §3 allows amber
    // on exactly three things — the destructive button, the write step and the
    // undefined state — all of which are on the firmware screen's third step, so a
    // rendered frame of that step must show it and the other screens must not.
    //
    // This lives here rather than in `src/screenshot.rs` because the source gate
    // (`tools/check_design.py`) is file-based and correctly refuses to have
    // `theme::DANGER` named anywhere but the destructive module. Naming the colour is
    // what a test does; the gate cannot tell a test from a paint call, so the test
    // moves to the crate that the gate does not scan.
    let mut app = App::default();
    app.firmware_mut()
        .show_sample()
        .expect("the sample image must be recognised");
    app.firmware_mut().set_step(Step::Flash);
    let firmware =
        m241_gui::screenshot::render(&mut app, Screen::Firmware, egui::vec2(640.0, 400.0));
    assert!(
        firmware.contains_colour(theme::DANGER, 12),
        "the destructive control is not amber in the rendered frame"
    );

    for screen in [Screen::Connect, Screen::Log, Screen::Settings] {
        let mut app = App::default();
        match screen {
            Screen::Connect => app.connect_mut().show_sample(),
            Screen::Log => app.log_mut().show_sample(),
            Screen::Firmware => {}
            Screen::Settings => {}
        }
        let image = m241_gui::screenshot::render(&mut app, screen, egui::vec2(640.0, 400.0));
        assert!(
            !image.contains_colour(theme::DANGER, 6),
            "{} has amber on it; amber means danger and lives on the firmware screen",
            screen.label()
        );
    }
}

#[test]
fn the_rail_keeps_its_fixed_width_across_every_screen() {
    // Regression test for pinning `egui::Panel::left("rail")` to `rail::WIDTH`:
    // reverting `.exact_size(rail::WIDTH)` to `.resizable(false)` lets the rail
    // drift to egui's default / content-sized width and shifts the heading on
    // navigation (DESIGN.md §4).
    for (width, height) in [(1280.0, 800.0), (1024.0, 640.0)] {
        for screen in Screen::ALL {
            let ctx = context();
            let mut app = App::default();
            app.select(screen);
            let shapes = render_shapes_at(&ctx, &mut app, width, height);
            let mut flat = Vec::new();
            flatten_shapes(&shapes, &mut flat);

            let rail_rect = flat
                .iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::WINDOW
                            && rect.rect.left().abs() < 0.5
                            && rect.rect.height() > 100.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .next()
                .unwrap_or_else(|| {
                    panic!(
                        "could not find the rail panel rect on {} at {width}×{height}",
                        screen.label()
                    )
                });

            assert!(
                (rail_rect.width() - rail::WIDTH).abs() < 0.5,
                "{} at {width}×{height}: rail width was {:.2}pt, expected {:.2}pt",
                screen.label(),
                rail_rect.width(),
                rail::WIDTH
            );
        }
    }
}

#[test]
fn the_export_action_lives_in_the_drawer_not_on_the_empty_screen() {
    // DESIGN.md §5 puts the map on the page and the export bar in the drawer, so
    // the export no longer sits in a pinned footer. An empty log offers nothing
    // to export; a populated one reaches it through the 書き出し tab.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);

    let draws_export = |app: &mut App| -> bool {
        let shapes = render_shapes_at(&ctx, app, 1280.0, 800.0);
        let mut flat = Vec::new();
        flatten_shapes(&shapes, &mut flat);
        flat.iter().any(|shape| match shape {
            egui::Shape::Text(text) => text.galley.job.text.contains("書き出す"),
            _ => false,
        })
    };

    assert!(
        !draws_export(&mut app),
        "an empty log screen must not render the export action"
    );

    app.log_mut().show_sample();
    assert!(
        !draws_export(&mut app),
        "the closed drawer must not draw the export action over the map"
    );

    app.log_mut()
        .open_drawer(m241_gui::screens::log::LogTab::Export);
    assert!(
        draws_export(&mut app),
        "the 書き出し tab must render the export action"
    );
}

#[test]
fn the_firmware_sample_renders_one_sample_banner_and_captions_its_panels() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    app.firmware_mut()
        .show_sample()
        .expect("the sample image must be recognised");

    for (step, expected_caption) in [
        (Step::Patch, "書き換え／見本"),
        (Step::Flash, "書き込み／見本"),
    ] {
        app.firmware_mut().set_step(step);
        let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
        let mut flat = Vec::new();
        flatten_shapes(&shapes, &mut flat);

        let texts: Vec<&str> = flat
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();

        let banner_count = texts
            .iter()
            .filter(|text| **text == "見本（実機から読んだ値ではありません）")
            .count();
        assert_eq!(
            banner_count, 1,
            "{step:?} rendered {banner_count} sample banners instead of 1"
        );
        assert!(
            texts.contains(&expected_caption),
            "{step:?} is missing sample panel caption {expected_caption}"
        );
    }
}

#[test]
fn the_firmware_step_row_paints_distinct_circle_primitives() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Firmware);
    app.firmware_mut().set_step(Step::Patch);

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);

    let circles: Vec<&egui::epaint::CircleShape> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Circle(circle) => Some(circle),
            _ => None,
        })
        .collect();

    assert_eq!(
        circles.len(),
        3,
        "expected three step-row markers, found {}",
        circles.len()
    );
    let done = circles
        .iter()
        .find(|circle| circle.fill == theme::DATA)
        .expect("missing filled Done marker");
    let current = circles
        .iter()
        .find(|circle| circle.fill == theme::INK)
        .expect("missing filled Current marker");
    let ahead = circles
        .iter()
        .find(|circle| circle.fill == egui::Color32::TRANSPARENT && circle.stroke.width > 0.0)
        .expect("missing open-ring Ahead marker");

    assert!((done.radius - theme::metric::POINT_DOT).abs() < 0.01);
    assert!((ahead.radius - theme::metric::POINT_DOT).abs() < 0.01);
    assert!(
        current.radius > done.radius,
        "Current marker radius {} must exceed Done radius {}",
        current.radius,
        done.radius
    );
}

#[test]
fn the_connect_panels_align_their_figures_to_the_right_edge() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Connect);
    app.connect_mut().show_sample();

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);

    let find_right_edge = |needle: &str| -> f32 {
        flat.iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.job.text == needle => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()).right())
                }
                _ => None,
            })
            .next()
            .unwrap_or_else(|| panic!("missing sample figure {needle:?}"))
    };

    // Device panel values of different widths must share the same right edge
    // (figure_line), not a left-aligned Grid column.
    let device_edges = [
        find_right_edge("B-core_1.20"),
        find_right_edge("0043"),
        find_right_edge("01017-00F"),
        find_right_edge("512 KiB"),
    ];
    for edge in &device_edges[1..] {
        assert!(
            (edge - device_edges[0]).abs() < 1.0,
            "device panel figures are not right-aligned: {device_edges:?}"
        );
    }

    // Log panel values beside it must also share one right edge.
    let log_edges = [
        find_right_edge("212,860 B"),
        find_right_edge("10399"),
        find_right_edge("11/14 → 11/15"),
        find_right_edge("有効"),
    ];
    for edge in &log_edges[1..] {
        assert!(
            (edge - log_edges[0]).abs() < 1.0,
            "log panel figures are not right-aligned: {log_edges:?}"
        );
    }
}

#[test]
fn connect_screen_renders_connected_and_failed_probes_without_blocking() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Connect);

    app.connect_mut()
        .start_probe_with(String::from("/dev/ttyFake0"), |_port| {
            Ok(m241_gui::screens::connect::DeviceProbe {
                info: m241_core::DeviceInfo {
                    release: String::from("B-core_1.20"),
                    build: String::from("0043"),
                    model: String::from("01017-00F"),
                    sdk: None,
                },
                log_used_bytes: 212_860,
                log_enabled: Some(true),
            })
        });
    for _ in 0..50 {
        if !app.connect_mut().poll_probe() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    assert!(
        flat.iter()
            .any(|s| matches!(s, egui::Shape::Text(t) if t.galley.job.text == "212,860 B")),
        "connected probe log usage did not render"
    );

    app.connect_mut()
        .start_probe_with(String::from("/dev/ttyFake1"), |_port| {
            Err(m241_gui::screens::connect::ProbeError {
                message: String::from("serial port: permission denied"),
                hint: String::from("ポート権限を確認してください"),
            })
        });
    for _ in 0..50 {
        if !app.connect_mut().poll_probe() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    flat.clear();
    flatten_shapes(&shapes, &mut flat);
    assert!(
        flat.iter().any(
            |s| matches!(s, egui::Shape::Text(t) if t.galley.job.text.contains("permission denied"))
        ),
        "failed probe device error did not render"
    );
}

#[test]
fn dragging_the_playback_slider_moves_the_selected_point() {
    // The timeline is the only continuous control that moves the selection, so
    // it is driven for real: press on its track, drag to the right, release. A
    // control rebuilt from `self.selected` every frame passes any state test and
    // fails here unless the drag itself writes the selection back.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();
    assert_eq!(app.log_mut().selected_index(), Some(0));

    let width = 1280.0;
    let height = 2400.0;
    let _ = render_at(&ctx, &mut app, width, height);
    let shapes = render_shapes_at(&ctx, &mut app, width, height);

    // The counter and the point's figures are on the timeline row (DESIGN.md
    // §5), so the readout names the row; the timeline is the long horizontal rule
    // within one line height of it. (The schematic grid also draws long
    // horizontals, so the y has to come from the readout rather than from length
    // alone.)
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let readout_y = flat
        .iter()
        .find_map(|shape| match shape {
            egui::Shape::Text(text) if text.galley.job.text.contains("/240") => Some(text.pos.y),
            _ => None,
        })
        .expect("the n/N readout did not render");
    let track = flat
        .iter()
        .find_map(|shape| match shape {
            egui::Shape::LineSegment { points, .. }
                if (points[0].y - points[1].y).abs() < f32::EPSILON
                    && (points[0].y - readout_y).abs() < 20.0
                    && (points[1].x - points[0].x).abs() > 300.0 =>
            {
                Some(*points)
            }
            _ => None,
        })
        .expect("the playback slider track did not render");

    let y = track[0].y;
    let from = egui::pos2(track[0].x + 4.0, y);
    let to = egui::pos2(track[0].x + (track[1].x - track[0].x) * 0.75, y);
    let input = |events: Vec<egui::Event>| RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        events,
        ..RawInput::default()
    };
    for events in [
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        vec![egui::Event::PointerMoved(egui::pos2(
            from.x + (to.x - from.x) / 2.0,
            y,
        ))],
        vec![
            egui::Event::PointerMoved(to),
            egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        Vec::new(),
    ] {
        let output = ctx.run_ui(input(events), |ui| app.draw(ui));
        output.drop_without_applying_deltas();
    }

    let selected = app.log_mut().selected_index();
    let len = app.log_mut().track().points.len();
    assert!(
        selected.is_some_and(|index| index > len / 2),
        "dragging the slider three quarters along left the selection at {selected:?} of {len}"
    );
}

#[test]
fn the_map_draws_a_poi_ring_at_the_waypoint() {
    // The POI bit is the device's own button. It is drawn as a ring at twice a
    // record's radius; the ring is `ink` over a `ground` disc so it reads on the
    // light OSM surface (design review 2026-11-23, M1). The count is asserted so
    // a ring that stops being painted (or one painted for every record) fails.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);

    let rings: Vec<&egui::epaint::CircleShape> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Circle(circle)
                if circle.fill == egui::Color32::TRANSPARENT
                    && circle.stroke.width > 0.0
                    && circle.stroke.color == theme::INK
                    && (circle.radius - (theme::metric::POINT_DOT * 2.0)).abs() < 0.01 =>
            {
                Some(circle)
            }
            _ => None,
        })
        .collect();

    // One POI ring, plus the start (1.5×) and end (2.5×) markers which are not
    // this radius. The selection marker is drawn at 3×, also not this radius.
    assert_eq!(
        rings.len(),
        1,
        "expected exactly one POI ring at {}pt, found {}",
        theme::metric::POINT_DOT * 2.0,
        rings.len()
    );
    let ring = rings.first().expect("the POI ring");
    assert_eq!(ring.stroke.color, theme::INK);
}

#[test]
fn the_selected_point_is_a_ground_halo_an_ink_disc_and_a_ring() {
    // DESIGN.md §5 / design review 2026-11-23, M1: the selection is a `ground`
    // halo, a filled `ink` disc and an `ink` ring one radius out, drawn last so a
    // record cannot cover it. The POI test above counts ring primitives; this
    // pins the three radii and that they share a centre, which is what makes the
    // marker read as one object rather than three coincidences.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let circles: Vec<&egui::epaint::CircleShape> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Circle(circle) => Some(circle),
            _ => None,
        })
        .collect();

    let find = |predicate: &dyn Fn(&egui::epaint::CircleShape) -> bool| {
        circles
            .iter()
            .enumerate()
            .find(|(_index, circle)| predicate(circle))
            .map(|(index, circle)| (index, *circle))
    };

    const HALO: f32 = 3.5;
    const DISC: f32 = 2.0;
    const RING: f32 = 3.0;
    let dot = theme::metric::POINT_DOT;
    let (halo_index, halo) =
        find(&|circle| circle.fill == theme::WINDOW && (circle.radius - dot * HALO).abs() < 0.01)
            .expect("the selection's window halo did not render");
    let (disc_index, disc) =
        find(&|circle| circle.fill == theme::INK && (circle.radius - dot * DISC).abs() < 0.01)
            .expect("the selection's ink disc did not render");
    let (ring_index, ring) = find(&|circle| {
        circle.fill == egui::Color32::TRANSPARENT
            && circle.stroke.color == theme::INK
            && (circle.radius - dot * RING).abs() < 0.01
            && (circle.stroke.width - theme::metric::HAIRLINE).abs() < 0.01
    })
    .expect("the selection's 3x ink ring did not render");

    assert!(
        (halo.center - disc.center).length() < 0.5,
        "the halo {halo:?} and the disc {disc:?} are not on the same point"
    );
    assert!(
        (disc.center - ring.center).length() < 0.5,
        "the disc {disc:?} and the ring {ring:?} are not on the same point"
    );
    assert!(
        halo_index < disc_index && disc_index < ring_index,
        "the selection was painted out of order: halo {halo_index}, disc {disc_index}, ring {ring_index}"
    );
}

#[test]
fn the_map_plates_are_overlay_faces_with_the_one_shadow() {
    // DESIGN.md §3/§4: a plate over the map is a floating layer, so it is
    // `OVERLAY` — opaque, so a bright tile cannot raise the background behind
    // `INK` — with a 1px `LINE` edge, the one radius and the one `POPUP_SHADOW`.
    // Read from the painted rects rather than from the constants: a plate that
    // left the surface, or one that lost its shadow, fails here.
    let ctx = context();
    // With the drawer open the toggle and the three map plates are four plates at
    // 1280; below `NARROW` the floating drawer adds a fifth.
    for (width, height, minimum) in [(1280.0_f32, 800.0_f32, 4_usize), (1024.0, 640.0, 5)] {
        let mut app = App::default();
        app.select(Screen::Log);
        app.log_mut().show_sample();
        app.log_mut()
            .open_drawer(m241_gui::screens::log::LogTab::Summary);
        let shapes = render_shapes_at(&ctx, &mut app, width, height);
        let mut flat = Vec::new();
        flatten_shapes(&shapes, &mut flat);
        let plates: Vec<&egui::epaint::RectShape> = flat
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == theme::OVERLAY && rect.corner_radius == theme::RADIUS =>
                {
                    Some(rect)
                }
                _ => None,
            })
            .collect();
        assert!(
            plates.len() >= minimum,
            "at {width}x{height} only {} plate(s) painted the OVERLAY surface, expected at least {minimum}",
            plates.len()
        );
        // Every plate carries the one shadow: `Frame::shadow` paints a blurred
        // rect in `POPUP_SHADOW.color` behind the face. A plate without one reads
        // as part of the map instead of as a layer over it.
        let shadowed = flat
            .iter()
            .filter(|shape| match shape {
                egui::Shape::Rect(rect) => {
                    rect.blur_width > 0.0 && rect.fill == theme::POPUP_SHADOW.color
                }
                _ => false,
            })
            .count();
        assert!(
            shadowed >= minimum,
            "at {width}x{height} {shadowed} plate(s) carried the one shadow, expected at least {minimum}",
        );
        if width < 1280.0 {
            assert!(
                plates
                    .iter()
                    .any(|rect| (rect.rect.width() - theme::metric::DRAWER).abs() < 1.0),
                "the floating drawer was not painted as an OVERLAY plate: {plates:?}"
            );
        }
    }
}

#[test]
fn the_track_is_one_solid_line_without_record_dots() {
    // DESIGN.md §10: the map's track is one line. A record is not a visible dot:
    // the old build painted a palette disc at every plotted record, which turned
    // a 240-point track into beads and its line into a dotted outline. Read off
    // the painted shapes rather than the constants, so "the line stopped being
    // painted" fails here too.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);

    let tracks: Vec<&egui::epaint::PathShape> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Path(path) if path.points.len() > 100 => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(
        tracks.len(),
        2,
        "the track is one line plus its casing; found {} path(s) over 100 points",
        tracks.len()
    );
    let casing = tracks
        .iter()
        .find(|path| (path.stroke.width - 7.0).abs() < f32::EPSILON)
        .expect("the track casing did not render");
    assert_eq!(
        casing.stroke.color,
        egui::epaint::ColorMode::Solid(theme::WINDOW),
        "the casing under the track is not the window colour"
    );
    let line = tracks
        .iter()
        .find(|path| (path.stroke.width - 5.0).abs() < f32::EPSILON)
        .expect("the track polyline did not render");
    // The headless sample draws no tiles, so the line is the dark-chrome token.
    assert_eq!(
        line.stroke.color,
        egui::epaint::ColorMode::Solid(theme::DATA),
        "the offline track is not the data colour at full opacity"
    );

    // No record is a dot: a filled circle at `POINT_DOT` would be one. The
    // playback thumb is also 3pt but it is `ink` — a control, not a record — so
    // the colour separates them. The selection, the two ends, the waypoint ring
    // and the photo markers are the only other points on the canvas.
    let record_dots = flat
        .iter()
        .filter(|shape| match shape {
            egui::Shape::Circle(circle) => {
                (circle.radius - theme::metric::POINT_DOT).abs() < 0.01
                    && circle.fill != egui::Color32::TRANSPARENT
                    && circle.fill != theme::INK
            }
            _ => false,
        })
        .count();
    assert_eq!(
        record_dots, 0,
        "a record was painted as a dot; the track is one line (DESIGN.md §10)"
    );
}

#[test]
fn log_screen_renders_playback_follow_mode_and_imperial_units() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.settings_mut().units = m241_gui::screens::settings::UnitSystem::Imperial;
    app.settings_mut().reduced_motion = true;
    app.log_mut()
        .set_track(mixed_track(), ParseReport::default());
    app.log_mut().set_zoom(5.0);
    app.log_mut().set_follow(true);
    let _ = app.log_mut().step_next();

    assert!(app.log_mut().map_zoom() >= 12.0);

    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let has_mph = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("mph"),
        _ => false,
    });
    assert!(has_mph, "imperial unit mph did not render on Log screen");
    let has_follow_zoom = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("追従 zoom 12"),
        _ => false,
    });
    assert!(
        has_follow_zoom,
        "follow mode zoom >= 12 status did not render on Log screen"
    );
}

#[test]
fn settings_screen_renders_live_controls_and_applies_reduced_motion_to_rail() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Settings);
    app.settings_mut().reduced_motion = true;
    app.settings_mut().units = m241_gui::screens::settings::UnitSystem::Imperial;

    assert!((app.settings().rail_transition_secs() - 0.0).abs() < f32::EPSILON);
    // Taller than the documented 800pt window on purpose: the map cache rows,
    // the map-tiles switch and the clear-cache action lengthened the page, so at
    // 800pt the scroll area clips the last rows (the note is still reachable by
    // scrolling). This test is about the note reflecting the setting, not about
    // everything fitting without a scroll.
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 1200.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let has_zero_ms = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("0 ms"),
        _ => false,
    });
    let texts: Vec<&str> = flat
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        has_zero_ms,
        "reduced motion 0 ms note did not render: {texts:?}"
    );
}

/// A minimal JPEG whose bytes carry no position, timestamped for the sample track.
fn write_jpeg(dir: &std::path::Path, name: &str, utc: u32) -> std::path::PathBuf {
    let mut bytes = vec![0xFF, 0xD8];
    // JFIF APP0
    bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    bytes.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    // EOI
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    let path = dir.join(name);
    std::fs::write(&path, &bytes).expect("write jpeg");
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open jpeg for mtime");
    let times = std::fs::FileTimes::new()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(utc)));
    file.set_times(times).expect("set jpeg mtime");
    path
}

/// A scratch directory the test removes on drop.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "m241-gui-render-{}-{label}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The first text shape whose label is exactly `label`, as its painted rect.
fn text_rect(shapes: &[egui::Shape], label: &str) -> Option<egui::Rect> {
    let mut flat = Vec::new();
    flatten_shapes(shapes, &mut flat);
    flat.iter().find_map(|shape| match shape {
        egui::Shape::Text(text) if text.galley.job.text == label => {
            Some(text.galley.rect.translate(text.pos.to_vec2()))
        }
        _ => None,
    })
}

/// Draw one pass with the events given, at a fixed window size.
fn draw_with(ctx: &Context, app: &mut App, events: Vec<egui::Event>, width: f32, height: f32) {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        events,
        ..RawInput::default()
    };
    let output = ctx.run_ui(input, |ui| app.draw(ui));
    output.drop_without_applying_deltas();
}

/// Click the centre of the first shape labelled `label`, the way a person does.
fn click_label(ctx: &Context, app: &mut App, label: &str) {
    let shapes = render_shapes_at(ctx, app, 1280.0, 900.0);
    let rect =
        text_rect(&shapes, label).unwrap_or_else(|| panic!("the label {label:?} did not render"));
    let at = rect.center();
    let press = egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };
    draw_with(
        ctx,
        app,
        vec![egui::Event::PointerMoved(at), press],
        1280.0,
        900.0,
    );
    draw_with(ctx, app, vec![release], 1280.0, 900.0);
    // One more pass so the immediate-mode screen settles on the new state.
    draw_with(ctx, app, Vec::new(), 1280.0, 900.0);
}

#[test]
fn pressing_geotag_writes_nothing_until_the_write_button_is_pressed() {
    // M2 from the Stage 3 review: adding `apply_geotags(GeotagConfirmation(()))`
    // right after 「写真に位置」 opened the prompt wrote the files with no
    // confirmation, and the old tests — which called the screen's methods
    // directly — still passed. This test only uses the UI: a real click on
    // 「写真に位置」 must open the prompt and leave the bytes alone; only a click
    // on 「書き込む」 may change them. The token type is not nameable from outside
    // its module, so this is the route a person actually takes.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);

    let dir = Scratch::new("geotag-ui");
    let photo_a = write_jpeg(&dir.0, "a.jpg", 1_763_000_000);
    let photo_b = write_jpeg(&dir.0, "b.jpg", 1_763_000_900);
    let orig_a = std::fs::read(&photo_a).expect("orig a");
    let orig_b = std::fs::read(&photo_b).expect("orig b");

    // Load the sample track and plan the photos through the screen's own loader.
    app.log_mut().show_sample();
    // 写真に位置 lives in the 書き出し drawer tab (DESIGN.md §5), so the frame the
    // click is measured against is the one with that tab open.
    app.log_mut()
        .open_drawer(m241_gui::screens::log::LogTab::Export);
    assert_eq!(
        app.log_mut().load_photos_from_path(&dir.0).expect("plan"),
        2
    );
    assert!(!app.log_mut().geotag_confirmation_open());

    // The drawer stays open with the prompt inside it, so a click needs no scroll.
    click_label(&ctx, &mut app, "写真に位置 (2 枚)");
    assert!(
        app.log_mut().geotag_confirmation_open(),
        "clicking 写真に位置 did not open the confirmation prompt"
    );
    assert_eq!(
        std::fs::read(&photo_a).expect("after asking a"),
        orig_a,
        "clicking 写真に位置 wrote to a.jpg before the prompt was answered"
    );
    assert_eq!(
        std::fs::read(&photo_b).expect("after asking b"),
        orig_b,
        "clicking 写真に位置 wrote to b.jpg before the prompt was answered"
    );

    // 「キャンセル」 answers the prompt without writing.
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 900.0);
    assert!(
        text_rect(&shapes, "書き込む").is_some(),
        "the prompt did not offer 書き込む"
    );
    click_label(&ctx, &mut app, "キャンセル");
    assert!(
        !app.log_mut().geotag_confirmation_open(),
        "キャンセル did not close the prompt"
    );
    assert_eq!(std::fs::read(&photo_a).expect("after cancel a"), orig_a);
    assert_eq!(std::fs::read(&photo_b).expect("after cancel b"), orig_b);

    // Ask again and answer with 「書き込む」: now the bytes change.
    click_label(&ctx, &mut app, "写真に位置 (2 枚)");
    assert!(app.log_mut().geotag_confirmation_open());
    click_label(&ctx, &mut app, "書き込む");
    assert!(
        !app.log_mut().geotag_confirmation_open(),
        "the prompt stayed open after 書き込む"
    );
    assert_ne!(
        std::fs::read(&photo_a).expect("after write a"),
        orig_a,
        "書き込む did not write a position into a.jpg"
    );
    assert_ne!(
        std::fs::read(&photo_b).expect("after write b"),
        orig_b,
        "書き込む did not write a position into b.jpg"
    );
}

#[test]
fn the_map_marks_the_start_and_the_end_of_the_track_in_ink() {
    // ezTour §5 draws a `line_start` and a `line_end` icon; DESIGN.md §5's map
    // has no icon set, so this screen separates them by ring radius and keeps
    // both in ink. Nothing tested that until the Stage 3 review (N6): deleting
    // the rings, or painting them amber, left the other tests green.
    //
    // The sample track selects point 0, which suppresses the start ring on that
    // frame; `select_point(len - 1)` moves the suppression to the end, and the
    // start ring is then visible. Both frames are measured so neither ring can
    // be missing.
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);
    app.log_mut().show_sample();

    let ring_radii = |app: &mut App| -> Vec<(f32, egui::Color32)> {
        let shapes = render_shapes_at(&ctx, app, 1280.0, 2400.0);
        let mut flat = Vec::new();
        flatten_shapes(&shapes, &mut flat);
        flat.iter()
            .filter_map(|shape| match shape {
                egui::Shape::Circle(circle)
                    if circle.fill == egui::Color32::TRANSPARENT && circle.stroke.width > 0.0 =>
                {
                    Some((circle.radius, circle.stroke.color))
                }
                _ => None,
            })
            .collect()
    };

    let dot = theme::metric::POINT_DOT;
    let start_radius = dot * 1.5;
    let end_radius = dot * 2.5;
    let find = |rings: &[(f32, egui::Color32)], radius: f32| {
        rings
            .iter()
            .find(|(r, _)| (r - radius).abs() < 0.01)
            .map(|(_, colour)| *colour)
    };

    // The last point selected: the end ring is suppressed, the start is drawn.
    let last = app.log_mut().track().points.len() - 1;
    app.log_mut().select_point(Some(last));
    let rings = ring_radii(&mut app);
    assert_eq!(
        find(&rings, start_radius),
        Some(theme::INK),
        "the start ring at {start_radius}pt is not drawn in ink; rings were {rings:?}"
    );

    // The start selected instead: the start is suppressed, the end is drawn.
    app.log_mut().select_point(Some(0));
    let rings = ring_radii(&mut app);
    assert_eq!(
        find(&rings, end_radius),
        Some(theme::INK),
        "the end ring at {end_radius}pt is not drawn in ink; rings were {rings:?}"
    );
}
