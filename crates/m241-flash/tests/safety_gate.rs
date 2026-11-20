//! Prove that `tools/check_safety.py` is wired into `tools/check.sh` and
//! actually detects every safety invariant it claims to enforce.
//!
//! A gate that silently stops matching is worse than no gate: it stays green
//! while the read/write boundary rots. These tests run the real Python script
//! against throwaway workspaces laid out the way the script expects (never
//! touching the real repository), asserting:
//!
//! * each forbidden command, escape form, write-shaped identifier, misplaced
//!   `set_control_lines` reference, and reversed dependency fails and names
//!   the violated rule;
//! * valid exclusions (comments, trailing inline comments, `erased`, SHA-256
//!   constants like `0xd5a7_9147`, and `set_control_lines` inside its exact
//!   allowlisted files) pass;
//! * `tools/check.sh` still invokes `tools/check_safety.py`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root; `crates/m241-flash` sits two levels below it.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/m241-flash is not inside a repository checkout")
        .to_path_buf()
}

/// A Python 3 interpreter. The safety gate itself is Python, so a machine that
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
    panic!("no Python 3 interpreter on PATH; the safety gate cannot be exercised");
}

const WORKSPACE_TOML: &str = "\
[workspace]
resolver = \"3\"
members = [\"crates/m241-core\", \"crates/m241-flash\", \"crates/m241-gui\"]
";

const CLEAN_CORE_TOML: &str = "\
[package]
name = \"m241-core\"
version = \"0.1.0\"
edition = \"2024\"
";

const CLEAN_FLASH_TOML: &str = "\
[package]
name = \"m241-flash\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
m241-core = { path = \"../m241-core\" }
";

const CLEAN_GUI_TOML: &str = "\
[package]
name = \"m241-gui\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
m241-core = { path = \"../m241-core\" }
";

/// A temporary workspace holding the real `tools/check_safety.py` plus the
/// `m241-core`, `m241-flash`, and `m241-gui` layouts a test wants to inspect.
/// Uses zero external dependencies so `cargo tree` runs offline and fast.
/// Removed when the test ends.
struct FakeRepo {
    root: PathBuf,
}

impl FakeRepo {
    fn new(label: &str, core_files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("m241-safety-gate-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::copy(
            repo_root().join("tools").join("check_safety.py"),
            root.join("tools").join("check_safety.py"),
        )
        .expect("tools/check_safety.py is missing; the safety gate cannot be verified");

        std::fs::write(root.join("Cargo.toml"), WORKSPACE_TOML).unwrap();

        let core = root.join("crates").join("m241-core");
        std::fs::create_dir_all(core.join("src")).unwrap();
        std::fs::write(core.join("Cargo.toml"), CLEAN_CORE_TOML).unwrap();
        std::fs::write(core.join("src").join("lib.rs"), "pub fn read_only() {}\n").unwrap();

        let flash = root.join("crates").join("m241-flash");
        std::fs::create_dir_all(flash.join("src")).unwrap();
        std::fs::write(flash.join("Cargo.toml"), CLEAN_FLASH_TOML).unwrap();
        std::fs::write(flash.join("src").join("lib.rs"), "pub fn write_path() {}\n").unwrap();

        let gui = root.join("crates").join("m241-gui");
        std::fs::create_dir_all(gui.join("src")).unwrap();
        std::fs::write(gui.join("Cargo.toml"), CLEAN_GUI_TOML).unwrap();
        std::fs::write(gui.join("src").join("lib.rs"), "pub fn ui() {}\n").unwrap();

        let repo = Self { root };
        for (relative, source) in core_files {
            repo.write_core_file(relative, source);
        }
        repo
    }

    /// Write a file under `crates/m241-core/` (e.g. `src/foo.rs` or `tests/bar.rs`).
    fn write_core_file(&self, relative: &str, content: &str) {
        let path = self.root.join("crates").join("m241-core").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Replace a crate manifest under `crates/<crate_name>/Cargo.toml`.
    fn write_manifest(&self, crate_name: &str, content: &str) {
        let path = self.root.join("crates").join(crate_name).join("Cargo.toml");
        std::fs::write(path, content).unwrap();
    }

    /// Run the gate. Returns whether it passed and everything it printed.
    fn run(&self) -> (bool, String) {
        let output: Output = Command::new(python())
            .arg(self.root.join("tools").join("check_safety.py"))
            .current_dir(&self.root)
            .output()
            .unwrap();
        let mut report = String::from_utf8_lossy(&output.stdout).into_owned();
        report.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.success(), report)
    }

    /// Assert the gate fails, names `expected`, and did not crash with a traceback.
    fn assert_reports(&self, expected: &str) {
        let (passed, report) = self.run();
        assert!(
            !passed,
            "the safety gate accepted a violation expected to report `{expected}`:\n{report}"
        );
        assert!(
            report.contains("safety violation"),
            "the gate failed without reporting a safety violation (did it crash?):\n{report}"
        );
        assert!(
            report.contains(expected),
            "the gate did not report `{expected}`:\n{report}"
        );
    }
}

impl Drop for FakeRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn the_safety_gate_is_still_wired_into_check_sh() {
    // The self-tests below exercise the checker, but only `tools/check.sh`
    // makes it a gate. Dropping the line would leave the remaining gates green.
    let script = std::fs::read_to_string(repo_root().join("tools").join("check.sh"))
        .expect("tools/check.sh is missing");
    assert!(
        script.contains("tools/check_safety.py"),
        "tools/check.sh no longer runs the safety gate:\n{script}"
    );
}

#[test]
fn compliant_sources_and_valid_exclusions_pass() {
    let repo = FakeRepo::new(
        "clean",
        &[
            (
                "src/lib.rs",
                "// Comments may mention $PMTK182,6, 0xD3, 0xD4, 0xD5, 0xD9, erase, program.\n\
                 /// Doc comments may mention set_control_lines and flash_write.\n\
                 pub const SHA256_K: u32 = 0xd5a7_9147;\n\
                 pub const P: u8 = 0; // trailing comment mentioning erase and 0xD5\n\
                 pub const URL: &str = \"http://example.com\"; // trailing erase comment\n\
                 pub const RAW: &str = r#\"http://example.com\"#; // trailing 0xD4 comment\n\
                 pub const QUOTE: char = '\"'; // trailing program comment\n\
                 pub fn id<'a>(s: &'a str) -> &'a str { s } // trailing erase comment\n\
                 pub fn an_erased_block(erased: bool) -> bool { erased }\n",
            ),
            (
                "src/protocol/brom.rs",
                "pub fn entry() { let _ = \"set_control_lines\"; }\n",
            ),
            (
                "src/protocol/port.rs",
                "pub trait Transport { fn set_control_lines(&mut self, asserted: bool); }\n",
            ),
            (
                "src/protocol/simulator.rs",
                "pub fn set_control_lines(_asserted: bool) {}\n",
            ),
            (
                "tests/read_only.rs",
                "// Test comment mentioning $PMTK182,6 and 0xD5\n\
                 #[test]\n\
                 fn ok() { let erased = 0xd5a7_9147_u32; assert_ne!(erased, 0); }\n",
            ),
        ],
    );
    let (passed, report) = repo.run();
    assert!(
        passed,
        "the safety gate rejected compliant sources:\n{report}"
    );
    assert!(
        report.contains("no write path in m241-core or m241-gui"),
        "the gate passed without its expected summary:\n{report}"
    );
}

#[test]
fn pmtk182_6_in_core_src_is_reported() {
    let repo = FakeRepo::new(
        "pmtk182-6",
        &[(
            "src/protocol/pmtk.rs",
            "pub const CMD: &str = \"$PMTK182,6\";\n",
        )],
    );
    repo.assert_reports("PMTK182,6");
}

#[test]
fn da_opcodes_0xd3_0xd4_0xd5_0xd9_are_all_reported() {
    for (label, snippet, token) in [
        ("opcode-d3", "pub const OP: u8 = 0xD3;\n", "0xD3"),
        ("opcode-d4", "pub const OP: u8 = 0xD4_u8;\n", "0xD4_u8"),
        ("opcode-d5", "pub const OP: u8 = 0xd5u8;\n", "0xd5u8"),
        ("opcode-d9", "pub const OP: u8 = 0xD9;\n", "0xD9"),
    ] {
        let repo = FakeRepo::new(label, &[("src/protocol/brom.rs", snippet)]);
        repo.assert_reports(token);
    }
}

#[test]
fn byte_literal_and_hex_escape_opcodes_are_reported() {
    for (label, snippet, token) in [
        ("byte-d3", "pub const OP: u8 = b'\\xd3';\n", "\\xd3"),
        ("byte-d4", "pub const OP: u8 = b'\\xD4';\n", "\\xD4"),
        (
            "str-d5",
            "pub const OP: &[u8] = b\"\\xd5\\x00\";\n",
            "\\xd5",
        ),
        ("str-d9", "pub const OP: &str = \"\\xD9\";\n", "\\xD9"),
    ] {
        let repo = FakeRepo::new(label, &[("src/protocol/brom.rs", snippet)]);
        repo.assert_reports(token);
    }
}

#[test]
fn double_slash_inside_a_string_does_not_hide_a_destructive_command() {
    let repo = FakeRepo::new(
        "slash-in-string",
        &[(
            "src/protocol/pmtk.rs",
            "pub const S: &str = \"http://example.com/PMTK182,6\"; // safe comment\n",
        )],
    );
    repo.assert_reports("PMTK182,6");
}

#[test]
fn destructive_command_in_core_tests_is_reported() {
    let repo = FakeRepo::new(
        "core-tests-opcode",
        &[("tests/sneaky.rs", "const OP: u8 = 0xD5;\n")],
    );
    repo.assert_reports("tests/sneaky.rs");
}

#[test]
fn write_path_identifiers_in_src_and_tests_are_reported() {
    for (label, path, snippet, ident) in [
        ("ident-erase", "src/lib.rs", "pub fn erase() {}\n", "erase"),
        (
            "ident-erase-log",
            "src/lib.rs",
            "pub fn erase_log() {}\n",
            "erase_log",
        ),
        (
            "ident-flash-write",
            "src/lib.rs",
            "pub fn flash_write() {}\n",
            "flash_write",
        ),
        (
            "ident-program",
            "src/lib.rs",
            "pub fn program() {}\n",
            "program",
        ),
        (
            "ident-program-test",
            "tests/helper.rs",
            "fn program_chip() {}\n",
            "program_chip",
        ),
    ] {
        let repo = FakeRepo::new(label, &[(path, snippet)]);
        repo.assert_reports(ident);
    }
}

#[test]
fn set_control_lines_outside_allowlist_and_basename_bypass_are_reported() {
    let outside = FakeRepo::new(
        "control-outside",
        &[(
            "src/protocol/pmtk.rs",
            "pub fn bad() { set_control_lines(true); }\n",
        )],
    );
    outside.assert_reports("set_control_lines outside");

    // `src/log/port.rs` shares its basename with `src/protocol/port.rs` and
    // must NOT be allowed to bypass the allowlist.
    let basename_bypass = FakeRepo::new(
        "control-basename",
        &[(
            "src/log/port.rs",
            "pub fn bad() { set_control_lines(true); }\n",
        )],
    );
    basename_bypass.assert_reports("crates/m241-core/src/log/port.rs");

    let in_tests = FakeRepo::new(
        "control-tests",
        &[("tests/port.rs", "fn bad() { set_control_lines(true); }\n")],
    );
    in_tests.assert_reports("crates/m241-core/tests/port.rs");
}

#[test]
fn core_depending_on_flash_is_reported() {
    let repo = FakeRepo::new("core-dep-flash", &[]);
    repo.write_manifest(
        "m241-core",
        "\
[package]
name = \"m241-core\"
version = \"0.1.0\"
edition = \"2024\"

[dev-dependencies]
m241-flash = { path = \"../m241-flash\" }
",
    );
    let (passed, report) = repo.run();
    assert!(!passed, "core dev-dep on flash was accepted:\n{report}");
    assert!(
        report.contains("crates/m241-core/Cargo.toml"),
        "manifest check did not fire:\n{report}"
    );
    assert!(
        report.contains("m241-core's dependency tree contains m241-flash"),
        "cargo tree check did not fire:\n{report}"
    );
}

#[test]
fn gui_depending_on_flash_is_reported() {
    let repo = FakeRepo::new("gui-dep-flash", &[]);
    repo.write_manifest(
        "m241-gui",
        "\
[package]
name = \"m241-gui\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
m241-core = { path = \"../m241-core\" }

[dev-dependencies]
m241-flash = { path = \"../m241-flash\" }
",
    );
    let (passed, report) = repo.run();
    assert!(!passed, "gui dev-dep on flash was accepted:\n{report}");
    assert!(
        report.contains("crates/m241-gui/Cargo.toml"),
        "gui manifest check did not fire:\n{report}"
    );
    assert!(
        report.contains("m241-gui's dependency tree contains m241-flash"),
        "gui cargo tree check did not fire:\n{report}"
    );
}

#[test]
fn flash_dropping_its_core_dependency_is_reported() {
    let repo = FakeRepo::new("flash-no-core", &[]);
    repo.write_manifest(
        "m241-flash",
        "\
[package]
name = \"m241-flash\"
version = \"0.1.0\"
edition = \"2024\"
",
    );
    repo.assert_reports("m241-flash does not depend on m241-core");
}
