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
            theme::GROUND,
            "{name} theme kept egui's panel fill instead of theme::GROUND"
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
    // A dump with a failed block checksum is a normal dump, not an error: the points
    // are kept and the damage is reported. The screen must render the warning rather
    // than panic on the report.
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
    // DESIGN.md §9 leaves the breakpoint open and names the rule: below 1100pt the
    // three-zone layout must collapse. Both widths are rendered so a layout that
    // only works wide fails here rather than in a screenshot review.
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
        DumpState::WaitingForPowerCycle,
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
                        if rect.fill == theme::RAIL
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
fn an_empty_log_omits_the_export_footer_until_a_track_exists() {
    let ctx = context();
    let mut app = App::default();
    app.select(Screen::Log);

    let empty_shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&empty_shapes, &mut flat);
    let has_export_when_empty = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("書き出す"),
        _ => false,
    });
    assert!(
        !has_export_when_empty,
        "an empty log screen must not render the export footer"
    );

    app.log_mut().show_sample();
    let loaded_shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    flat.clear();
    flatten_shapes(&loaded_shapes, &mut flat);
    let has_export_when_loaded = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("書き出す"),
        _ => false,
    });
    assert!(
        has_export_when_loaded,
        "a populated log screen must render the export footer"
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
            .filter(|text| **text == "見本 — 実機から読んだ値ではありません")
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
    let shapes = render_shapes_at(&ctx, &mut app, 1280.0, 800.0);
    let mut flat = Vec::new();
    flatten_shapes(&shapes, &mut flat);
    let has_zero_ms = flat.iter().any(|shape| match shape {
        egui::Shape::Text(text) => text.galley.job.text.contains("0 ms"),
        _ => false,
    });
    assert!(has_zero_ms, "reduced motion 0 ms note did not render");
}
