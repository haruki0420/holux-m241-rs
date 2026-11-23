//! The three design invariants and the two token rules, asserted against the real
//! sources.
//!
//! `tools/check_design.py` is the CI gate, and `design_gate.rs` proves that the
//! script actually detects a planted violation. Neither of them says anything about
//! *this* tree on its own: the script could pass because it stopped matching, and the
//! gate test only ever writes its own fixture files.
//!
//! These tests read `crates/m241-gui/src/` directly and count. They are the ones that
//! fail when a screen grows a fourth amber, a hand-written radius, or a font size
//! that is not one of the five steps in DESIGN.md §3 — the mistakes that are easy to
//! make in an immediate-mode UI and hard to see in a diff.
//!
//! Deliberately independent of the Python script: the two are allowed to disagree,
//! and a disagreement is a bug in one of them rather than a passing build.

// An integration test is its own crate; a failing assertion here is the test's job.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

/// `crates/m241-gui/src`.
fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `crates/m241-gui/src`, sorted.
fn sources() -> Vec<PathBuf> {
    fn walk(root: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            panic!(
                "cannot read {}; the test cannot inspect the sources",
                root.display()
            );
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&src(), &mut files);
    files
}

/// The file's text, or a panic that names the path.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

/// The file's production code: everything before its `#[cfg(test)]` module.
///
/// The invariants are about what ships. A test that asserts "the amber use carries
/// words" must be allowed to name the colour, so scanning past `#[cfg(test)]` would
/// turn the guard into a rule against checking the guard.
fn production(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => text,
    }
}

/// The source lines that are code, not documentation, with their real line numbers.
///
/// A doc comment that mentions `theme::DANGER` is not a use, and the invariant is
/// about uses. Lines are filtered rather than parsed: the files are small and a
/// comment-only line is unambiguous at the start of the trimmed text.
///
/// The number is kept from the unfiltered file: a report that points at line 37 of a
/// 120-line file when the offending line is 64 wastes the reader's time, and a check
/// that cannot be located is a check somebody deletes.
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    production(text)
        .lines()
        .enumerate()
        .filter(|(_number, line)| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//") || trimmed.starts_with("*"))
        })
        .map(|(number, line)| (number + 1, line))
        .collect()
}

#[test]
fn amber_appears_exactly_three_times_and_only_in_the_firmware_screen() {
    // DESIGN.md §3: the destructive button, the write step, the undefined-device
    // state. Three, not "a few": the count is what keeps amber meaningful, and it can
    // only be asserted by looking at every file.
    let mut total = 0;
    for path in sources() {
        let text = read(&path);
        for (number, line) in code_lines(&text) {
            if !line.contains("theme::DANGER") {
                continue;
            }
            // The pairing form is the only allowed shape: the colour is returned with
            // the words that go beside it, and both are consumed at the call site.
            let is_pair = line.contains("(theme::DANGER, \"");
            assert!(
                is_pair,
                "{}:{number}: amber outside the `(colour, words)` pairing: {line}",
                path.display()
            );
            assert!(
                path.ends_with("screens/firmware.rs"),
                "{}:{number}: amber outside the destructive module: {line}",
                path.display()
            );
            total += 1;
        }
    }
    assert_eq!(
        total, 3,
        "amber is allowed in exactly three places (DESIGN.md §3); found {total}"
    );
}

#[test]
fn the_coordinates_of_the_fifteen_tokens_come_from_the_theme_module_only() {
    // A colour written by hand is a second palette. `theme.rs` is the only file
    // that may spell one out; every other file names a token — and that includes
    // the derived forms (`from_black_alpha`, `from_gray`, `from_rgba`), because a
    // shadow or a dimmed tone is still a colour decision.
    for path in sources() {
        if path.ends_with("theme.rs") {
            continue;
        }
        let text = read(&path);
        for (number, line) in production(&text).lines().enumerate() {
            assert!(
                !line.contains("Color32::from_rgb")
                    && !line.contains("Color32::from_rgba")
                    && !line.contains("Color32::from_black_alpha")
                    && !line.contains("Color32::from_white_alpha")
                    && !line.contains("Color32::from_gray"),
                "{}:{}: a colour literal outside theme.rs: {line}",
                path.display(),
                number + 1
            );
        }
    }
}

#[test]
fn the_one_shadow_lives_in_the_theme_module_only() {
    // DESIGN.md §3: elevation is a fill plus a 1px edge; exactly one shadow exists
    // (`theme::POPUP_SHADOW`) and it means "this layer floats". A second `Shadow`
    // literal is a second elevation language, the same failure the corner-radius
    // gate exists to catch.
    for path in sources() {
        if path.ends_with("theme.rs") {
            continue;
        }
        let text = read(&path);
        assert!(
            !production(&text).contains("egui::epaint::Shadow {")
                && !production(&text).contains("epaint::Shadow {")
                && !production(&text).contains("Shadow {"),
            "{}: a shadow literal outside theme.rs; use theme::POPUP_SHADOW instead",
            path.display()
        );
    }
}

#[test]
fn the_previous_generation_s_token_names_are_gone() {
    // `GROUND` / `PANEL` / `RAIL` / `RULE` were the printed-form palette. They
    // were removed with the direction, and a screen that reintroduces one has
    // reintroduced a second vocabulary for the same surface.
    for path in sources() {
        let text = read(&path);
        for (number, line) in production(&text).lines().enumerate() {
            for old in [
                "theme::GROUND",
                "theme::PANEL",
                "theme::RAIL",
                "theme::RULE",
            ] {
                assert!(
                    !line.contains(old),
                    "{}:{}: the old token name {old} is back: {line}",
                    path.display(),
                    number + 1
                );
            }
        }
    }
}

#[test]
fn the_only_corner_radius_is_the_token() {
    // DESIGN.md §3: 6px, one token. A literal that happens to equal the token is
    // still a violation: the next person changes the token and this one keeps the old
    // shape.
    for path in sources() {
        if path.ends_with("theme.rs") {
            continue;
        }
        let text = read(&path);
        assert!(
            !production(&text).contains("CornerRadius::same"),
            "{}: a literal corner radius; use theme::RADIUS instead",
            path.display()
        );
    }
}

#[test]
fn type_sizes_and_spacing_come_from_the_scale() {
    // DESIGN.md §3 fixes five type steps (12/14/16/20/28) and a 4px spacing scale.
    // A raw `13.0` in a `RichText` is how a sixth step appears, and the mistake is
    // invisible in review because 13 is a legal-looking number.
    const ALLOWED: [&str; 5] = [
        "size::CAPTION",
        "size::UI",
        "size::BODY",
        "size::TITLE",
        "size::DISPLAY",
    ];
    for path in sources() {
        if path.ends_with("theme.rs") {
            continue;
        }
        let text = read(&path);
        for (number, line) in production(&text).lines().enumerate() {
            for needle in [
                ".size(",
                "FontId::proportional(",
                "FontId::monospace(",
                "add_space(",
            ] {
                let Some(at) = line.find(needle) else {
                    continue;
                };
                let argument = &line[at + needle.len()..];
                let literal = argument.starts_with(|character: char| character.is_ascii_digit());
                assert!(
                    !literal,
                    "{}:{}: a raw number where a token belongs: {line}",
                    path.display(),
                    number + 1
                );
                // `size::` arguments must be one of the five steps, not a new one.
                if needle == ".size(" {
                    assert!(
                        ALLOWED.iter().any(|token| argument.starts_with(token))
                            || argument.starts_with("Self::")
                            || argument.starts_with("font_size"),
                        "{}:{}: a type size outside the five steps: {line}",
                        path.display(),
                        number + 1
                    );
                }
            }
        }
    }
}

#[test]
fn a_status_colour_is_never_alone_on_a_line() {
    // DESIGN.md §3: status is never encoded by colour alone. The screen code must
    // therefore never paint `theme::OK`/`ERR`/`DANGER` without a label on the same
    // line — which in practice means going through `common::status_line`, whose
    // signature forces a `Status` and a `&str` together.
    for path in sources() {
        if path.ends_with("theme.rs") {
            continue;
        }
        let text = read(&path);
        for (number, line) in production(&text).lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("use ") || trimmed.starts_with("pub const") {
                continue;
            }
            for token in ["theme::OK", "theme::ERR", "theme::DANGER"] {
                if !line.contains(token) {
                    continue;
                }
                assert!(
                    line.contains('"'),
                    "{}:{}: {token} without a text label on the same line: {line}",
                    path.display(),
                    number + 1
                );
            }
        }
    }
}

#[test]
fn the_screens_do_not_paint_raw_shapes_without_the_tokens() {
    // A weak but useful floor: a screen that paints with a bare `Stroke::new(1.0, …)`
    // or a bare `Shape::line` is drawing furniture that belongs in `common` or the
    // theme. This asserts the *canvas* work that does exist uses the token widths.
    let log = read(&src().join("screens").join("log.rs"));
    for (number, line) in log.lines().enumerate() {
        if line.contains("Stroke::new(") {
            assert!(
                line.contains("metric::") || line.contains("theme::"),
                "screens/log.rs:{}: a stroke with no token: {line}",
                number + 1
            );
        }
    }
}
