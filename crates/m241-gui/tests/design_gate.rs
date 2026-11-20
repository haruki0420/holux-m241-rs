//! Prove that `tools/check_design.py` actually detects the three invariants it
//! claims to enforce.
//!
//! A gate that has silently stopped matching is worse than no gate: it stays
//! green while the invariant it guards rots, and the failure only shows up in a
//! review that nobody has time for. The Python checker is one of the five gates
//! in `tools/check.sh`, so a mistake in its regular expressions is invisible
//! from the Rust side.
//!
//! These tests run the real script, against a throwaway repository laid out the
//! way the script expects, and assert:
//!
//! * each of the three invariants fails on a minimal violating file, and the
//!   report names that invariant (a check that stops firing is caught);
//! * a compliant tree passes (a checker that flags everything is caught);
//! * `tools/check.sh` still runs the checker (removing the gate is caught).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root; `crates/m241-gui` sits two levels below it.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/m241-gui is not inside a repository checkout")
        .to_path_buf()
}

/// A Python 3 interpreter. The design gate itself is Python, so a machine that
/// cannot run the checker cannot run the gate either.
fn python() -> String {
    for candidate in ["python3", "python"] {
        let reported = match Command::new(candidate).arg("--version").output() {
            Ok(output) => output,
            Err(_) => continue,
        };
        let mut version = String::from_utf8_lossy(&reported.stdout).into_owned();
        version.push_str(&String::from_utf8_lossy(&reported.stderr));
        if version.contains("Python 3") {
            return candidate.to_owned();
        }
    }
    panic!("no Python 3 interpreter on PATH; the design gate cannot be exercised");
}

/// A temporary checkout holding the real checker plus the GUI sources a test
/// wants it to inspect. Removed when the test ends.
struct FakeRepo {
    root: PathBuf,
}

impl FakeRepo {
    fn new(label: &str, sources: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("m241-design-gate-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::copy(
            repo_root().join("tools").join("check_design.py"),
            root.join("tools").join("check_design.py"),
        )
        .expect("tools/check_design.py is missing; the design gate cannot be verified");

        for (relative, source) in sources {
            let path = root
                .join("crates")
                .join("m241-gui")
                .join("src")
                .join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, source).unwrap();
        }

        Self { root }
    }

    /// Run the gate. Returns whether it passed and everything it printed.
    fn run(&self) -> (bool, String) {
        let output: Output = Command::new(python())
            .arg(self.root.join("tools").join("check_design.py"))
            .current_dir(&self.root)
            .output()
            .unwrap();
        let mut report = String::from_utf8_lossy(&output.stdout).into_owned();
        report.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.success(), report)
    }

    /// Assert the gate fails, names `invariant`, and does not fail for some
    /// unrelated reason such as a Python traceback.
    fn assert_reports(&self, invariant: &str) {
        let (passed, report) = self.run();
        assert!(
            !passed,
            "the design gate accepted a {invariant} violation:\n{report}"
        );
        assert!(
            report.contains("design invariant violation"),
            "the gate failed without reporting a violation (did it crash?):\n{report}"
        );
        assert!(
            report.contains(invariant),
            "the gate did not name the {invariant} invariant:\n{report}"
        );
    }
}

impl Drop for FakeRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn amber_outside_the_destructive_module_is_reported() {
    let repo = FakeRepo::new(
        "amber",
        &[("screens/log.rs", "use crate::theme::DANGER;\n")],
    );
    repo.assert_reports("amber (DANGER) used outside");
}

#[test]
fn amber_written_as_a_literal_is_reported() {
    let repo = FakeRepo::new(
        "amber-literal",
        &[(
            "screens/log.rs",
            "fn amber() -> Color32 {\n    Color32::from_rgb(0xE3, 0xA0, 0x08)\n}\n",
        )],
    );
    repo.assert_reports("amber written as a literal");
}

#[test]
fn a_literal_corner_radius_is_reported() {
    let repo = FakeRepo::new(
        "radius",
        &[(
            "widgets/panel.rs",
            "fn radius() -> CornerRadius {\n    CornerRadius::same(2)\n}\n",
        )],
    );
    repo.assert_reports("literal corner radius");
}

#[test]
fn a_status_colour_without_a_label_is_reported() {
    let repo = FakeRepo::new(
        "status",
        &[(
            "screens/connect.rs",
            "fn state(ui: &mut Ui, rect: Rect) {\n    ui.painter().rect_filled(rect, theme::RADIUS, theme::ERR);\n}\n",
        )],
    );
    repo.assert_reports("without a text label");
}

#[test]
fn compliant_sources_pass() {
    // One use of each token shape the checks allow: amber in the destructive
    // module next to its label, the radius token, and a status colour on a line
    // that carries text. If any check starts flagging correct code, this fails.
    let repo = FakeRepo::new(
        "clean",
        &[
            (
                "theme.rs",
                "pub const DANGER: Color32 = Color32::from_rgb(0xE3, 0xA0, 0x08);\n",
            ),
            (
                "screens/firmware.rs",
                "pub fn confirm(ui: &mut Ui) {\n    ui.label(RichText::new(\"書き込み\").color(theme::DANGER));\n}\n",
            ),
            (
                "widgets/panel.rs",
                "pub fn radius() -> CornerRadius {\n    theme::RADIUS\n}\n",
            ),
            (
                "screens/connect.rs",
                "pub fn state(ui: &mut Ui) {\n    ui.label(RichText::new(\"未接続\").color(theme::ERR));\n}\n",
            ),
        ],
    );
    let (passed, report) = repo.run();
    assert!(
        passed,
        "the design gate rejected compliant sources:\n{report}"
    );
    assert!(
        report.contains("all design invariants hold"),
        "the gate passed without saying what it verified:\n{report}"
    );
}

#[test]
fn the_design_gate_is_still_wired_into_check_sh() {
    // The self-tests above exercise the checker, but only `tools/check.sh`
    // makes it a gate. Dropping the line would leave every check green.
    let script = std::fs::read_to_string(repo_root().join("tools").join("check.sh"))
        .expect("tools/check.sh is missing");
    assert!(
        script.contains("tools/check_design.py"),
        "tools/check.sh no longer runs the design gate:\n{script}"
    );
}
