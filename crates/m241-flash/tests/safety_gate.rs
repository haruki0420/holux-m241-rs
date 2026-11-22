//! Prove that `tools/check_safety.py` is wired into `tools/check.sh` and
//! actually detects every safety invariant it claims to enforce.
//!
//! A gate that silently stops matching is worse than no gate: it stays green
//! while the read/write boundary rots. These tests run the real Python script
//! against throwaway workspaces laid out the way the script expects (never
//! touching the real repository), asserting:
//!
//! * the GUI's write route: its manifest and tree must reach `m241-flash`,
//!   `device.rs` must call `flow::flash(`, and `firmware.rs` must build the
//!   confirmation from its typed word; a destructive opcode in GUI code fails;
//! * each forbidden command, escape form, write-shaped identifier, misplaced
//!   `set_control_lines` reference, reversed dependency, and digest-based image
//!   acceptance fails and names the violated rule;
//! * `DaDigest::for_test_da` without its cfg gate fails, because a shipped
//!   build could otherwise name any digest as the DA's;
//! * `DownloadAgent::write` without its `check_layout()?` call at the body's
//!   top level fails, including when the call appears only in a comment,
//!   another function, or an uncalled nested function;
//! * the mutation test removes that line from both an LF and a CRLF checkout;
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
m241-flash = { path = \"../m241-flash\" }
";

/// The smallest `da.rs` that satisfies both `m241-flash` rules: `check_image`
/// accepts only by the backup diff (the doc comment names the forbidden tokens
/// on purpose: prose must stay allowed), and `write` calls `check_layout()`
/// before it reaches `write_range`. Not compiled; the gate reads it as text.
const CLEAN_DA_RS: &str = "\
//! Acceptance is the backup diff; this comment may name sha256 and firmware::digest.
pub fn check_image(backup: &[u8], image: &[u8]) -> bool {
    // A trailing comment may also name profile.sha256.
    backup == image
}

pub fn all_within(backup: &[u8], image: &[u8]) -> bool {
    backup == image
}

// The DA check may hash the file; only the firmware acceptance is forbidden a digest.
pub fn check_da_image(image: &[u8]) -> bool {
    m241_core::firmware::sha256_hex(image).len() == 64
}

// The one DA digest token constructor a shipped build must not see.
#[must_use]
#[cfg(any(test, feature = \"simulator\"))]
pub fn for_test_da(sha256_hex: &str) -> DaDigest {
    DaDigest::from_hex(sha256_hex)
}

pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {
    check_layout()?;
    write_range(backup, image)
}
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
        std::fs::write(flash.join("src").join("da.rs"), CLEAN_DA_RS).unwrap();

        let gui = root.join("crates").join("m241-gui");
        std::fs::create_dir_all(gui.join("src")).unwrap();
        std::fs::write(gui.join("Cargo.toml"), CLEAN_GUI_TOML).unwrap();
        std::fs::write(gui.join("src").join("lib.rs"), "pub fn ui() {}\n").unwrap();
        // The GUI write path's two invariants: it goes through the shared
        // session, and its confirmation comes from the screen's typed word.
        std::fs::write(
            gui.join("src").join("device.rs"),
            "pub fn go() { flow::flash(); }\n",
        )
        .unwrap();
        std::fs::create_dir_all(gui.join("src").join("screens")).unwrap();
        std::fs::write(
            gui.join("src").join("screens").join("firmware.rs"),
            "pub const CONFIRMATION: &str = \"WRITE\";\npub fn c() { Confirmation::granted(); }\n",
        )
        .unwrap();

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

    /// Write a file under `crates/m241-flash/` (e.g. `src/da.rs`).
    fn write_flash_file(&self, relative: &str, content: &str) {
        let path = self.root.join("crates").join("m241-flash").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Replace a crate manifest under `crates/<crate_name>/Cargo.toml`.
    fn write_manifest(&self, crate_name: &str, content: &str) {
        let path = self.root.join("crates").join(crate_name).join("Cargo.toml");
        std::fs::write(path, content).unwrap();
    }

    /// Replace `crates/m241-flash/src/da.rs`, which the layout-gate check reads.
    fn write_flash_da(&self, content: &str) {
        let path = self
            .root
            .join("crates")
            .join("m241-flash")
            .join("src")
            .join("da.rs");
        std::fs::write(path, content).unwrap();
    }

    /// Run the gate. Returns whether it passed and everything it printed.
    fn run(&self) -> (bool, String) {
        let output: Output = Command::new(python())
            .arg(self.root.join("tools").join("check_safety.py"))
            .current_dir(&self.root)
            // The gate's violations quote `AGENTS.md §2.3`. A pipe on Windows
            // uses the ANSI code page, so `§` reached the test as `?` and an
            // assertion on the exact message failed for an encoding reason, not
            // because the rule stopped firing. Pin the child's output to UTF-8.
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
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

/// Remove one `check_layout()?;` line from the named public function.
///
/// A Windows checkout reads `da.rs` with `\r\n`. The pattern below is written
/// with `\n`, so normalising first is what keeps the mutation test from
/// panicking with "the line was not found" on that platform. The function name
/// matters: `write` and `write_vendor` both have the line, and a mutation must
/// remove only the one under test.
fn without_layout_gate_in(da_rs: &str, function: &str) -> String {
    let normalized = da_rs.replace("\r\n", "\n");
    let marker = format!("pub fn {function}(");
    let start = normalized
        .find(&marker)
        .expect("the function is missing from da.rs");
    let (head, tail) = normalized.split_at(start);
    let tail = tail.replacen("        check_layout()?;\n", "", 1);
    format!("{head}{tail}")
}

/// Remove `DownloadAgent::write`'s layout-gate line, accepting LF or CRLF.
fn without_layout_gate(da_rs: &str) -> String {
    without_layout_gate_in(da_rs, "write")
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
        report.contains("no write path in m241-core"),
        "the gate passed without its expected summary:\n{report}"
    );
}

#[test]
fn digest_based_image_acceptance_in_flash_is_reported() {
    // This is the mutation the review found: put the old vendor branch back in
    // `check_image`, and the Rust tests still pass because no synthetic fixture
    // hashes to the real vendor digest. The safety gate reads the source, so it
    // must fail even when the runtime branch would never fire in a test.
    let fully_qualified = "\
pub fn check_image(backup: &[u8], image: &[u8]) -> bool {
    if m241_core::firmware::digest::matches(image, PROFILE.sha256) {
        return true;
    }
    backup == image
}
";
    let imported = "\
use m241_core::firmware::digest::matches;

pub fn check_image(backup: &[u8], image: &[u8], profile: &Profile) -> bool {
    if matches(image, profile.sha256) {
        return true;
    }
    backup == image
}
";
    let hash_comparison = "\
pub fn check_image(backup: &[u8], image: &[u8], profile: &Profile) -> bool {
    let digest = m241_core::firmware::sha256_hex(image);
    if digest == profile.sha256 {
        return true;
    }
    backup == image
}
";
    let all_within_bypass = "\
pub fn check_image(backup: &[u8], image: &[u8]) -> bool {
    backup == image
}

pub fn all_within(backup: &[u8], image: &[u8], profile: &Profile) -> bool {
    let digest = m241_core::firmware::sha256_hex(image);
    digest == profile.sha256 || backup == image
}

pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {
    check_layout()?;
    write_range(backup, image)
}
";
    let support = "\
pub fn all_within(backup: &[u8], image: &[u8]) -> bool {
    backup == image
}

pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {
    check_layout()?;
    write_range(backup, image)
}
";
    for (label, source) in [
        ("digest-fully-qualified", fully_qualified),
        ("digest-imported", imported),
        ("digest-hash-comparison", hash_comparison),
        ("digest-all-within", all_within_bypass),
    ] {
        let repo = FakeRepo::new(label, &[]);
        let source = if label == "digest-all-within" {
            source.to_owned()
        } else {
            format!("{source}\n{support}")
        };
        repo.write_flash_file("src/da.rs", &source);
        repo.assert_reports("digest-based image acceptance");
        let (passed, report) = repo.run();
        assert!(!passed, "{label} was accepted:\n{report}");
        assert!(
            report.contains("crates/m241-flash/src/da.rs"),
            "{label} did not name the file:\n{report}"
        );
        assert!(
            report.contains("AGENTS.md §2.3"),
            "{label} did not name the rule:\n{report}"
        );
    }
}

#[test]
fn a_da_digest_constructor_outside_its_cfg_gate_is_reported() {
    // The `DaDigest` token is what keeps "the DA must be the vendor image" a
    // compile-time property: a shipped build can construct it only from the
    // pinned digest. `for_test_da` is the one other constructor, and its cfg
    // attribute is what keeps it out. Removing that attribute is the change the
    // gate has to catch, because nothing in `cargo test` compiles the shipped
    // branch that would then be missing its only guard.
    let ungated = CLEAN_DA_RS.replace("#[cfg(any(test, feature = \"simulator\"))]\n", "");
    assert_ne!(
        ungated, CLEAN_DA_RS,
        "the clean fixture must carry the cfg attribute this test removes"
    );
    let repo = FakeRepo::new("da-digest-ungated", &[]);
    repo.write_flash_da(&ungated);
    repo.assert_reports("DaDigest::for_test_da");
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
fn removing_check_layout_from_the_real_write_body_is_reported() {
    // The exact regression this gate exists for: the call is deleted from
    // `DownloadAgent::write` and every unit test still passes, because they
    // exercise `write_range` directly. The temporary workspace holds the real
    // file with only that line removed.
    let real = std::fs::read_to_string(
        repo_root()
            .join("crates")
            .join("m241-flash")
            .join("src")
            .join("da.rs"),
    )
    .expect("crates/m241-flash/src/da.rs is missing");
    let normalized = real.replace("\r\n", "\n");
    let without = without_layout_gate(&real);
    assert_ne!(
        without, normalized,
        "the `check_layout()?;` line was not found in the real write body"
    );

    let repo = FakeRepo::new("da-real-minus-layout", &[]);
    repo.write_flash_da(&without);
    repo.assert_reports("does not call `check_layout()?`");
}

#[test]
fn removing_check_layout_from_the_real_write_vendor_body_is_reported() {
    // `write_vendor` reaches `write_range_accepted`, not `write_range`; the
    // gate must cover it too. Remove only its own line, leaving `write`'s line
    // intact, so a gate that still checked only `pub fn write(` would pass.
    let real = std::fs::read_to_string(
        repo_root()
            .join("crates")
            .join("m241-flash")
            .join("src")
            .join("da.rs"),
    )
    .expect("crates/m241-flash/src/da.rs is missing");
    let without = without_layout_gate_in(&real, "write_vendor");
    assert_ne!(without, real.replace("\r\n", "\n"));
    assert!(
        without.contains("        check_layout()?;\n"),
        "the `write` layout-gate line must remain for this mutation"
    );

    let repo = FakeRepo::new("da-real-minus-vendor-layout", &[]);
    repo.write_flash_da(&without);
    repo.assert_reports("`pub fn write_vendor` does not call `check_layout()?`");
}

#[test]
fn a_crlf_checkout_detects_the_removed_layout_gate() {
    // windows-latest checks the file out with CRLF. The gate itself reads
    // either ending, but a mutation test that matched only `\n` panicked
    // before it could exercise the gate on that platform.
    let real = std::fs::read_to_string(
        repo_root()
            .join("crates")
            .join("m241-flash")
            .join("src")
            .join("da.rs"),
    )
    .expect("crates/m241-flash/src/da.rs is missing");
    let crlf = real.replace("\r\n", "\n").replace('\n', "\r\n");
    assert!(
        crlf.contains("        check_layout()?;\r\n"),
        "the CRLF fixture does not contain the layout-gate line:\n{crlf}"
    );

    // A clean CRLF file passes: CRLF must not become a false positive.
    let clean = FakeRepo::new("da-crlf-clean", &[]);
    clean.write_flash_da(&crlf);
    let (passed, report) = clean.run();
    assert!(
        passed,
        "the safety gate rejected a compliant CRLF da.rs:\n{report}"
    );

    let without = without_layout_gate(&crlf);
    assert!(
        without.contains("check_layout()?;"),
        "the other writer's layout-gate line must remain: {without}"
    );
    assert!(
        without_layout_gate_in(&crlf, "write_vendor").contains("check_layout()?;"),
        "the write_vendor layout-gate line must remain"
    );
    // Feed the gate a CRLF file that is missing the call, not an LF one.
    let without_crlf = without.replace('\n', "\r\n");
    let repo = FakeRepo::new("da-crlf-minus-layout", &[]);
    repo.write_flash_da(&without_crlf);
    repo.assert_reports("does not call `check_layout()?`");
}

#[test]
fn a_layout_gate_only_in_a_comment_does_not_count() {
    let repo = FakeRepo::new("da-layout-comment", &[]);
    repo.write_flash_da(
        "pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {\n\
         // check_layout()?;\n\
         write_range(backup, image)\n\
         }\n",
    );
    repo.assert_reports("does not call `check_layout()?`");
}

#[test]
fn a_layout_gate_only_in_another_function_does_not_count() {
    let repo = FakeRepo::new("da-layout-other-function", &[]);
    repo.write_flash_da(
        "fn prepare() -> Result<()> { check_layout()?; Ok(()) }\n\
         pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {\n\
         write_range(backup, image)\n\
         }\n",
    );
    repo.assert_reports("does not call `check_layout()?`");
}

#[test]
fn a_nested_function_that_calls_the_layout_gate_does_not_count() {
    // The nested `prepare` is never called. Its `check_layout()` is at brace
    // depth 2 inside `write`, so it must not satisfy the top-level gate.
    let repo = FakeRepo::new("da-layout-nested-function", &[]);
    repo.write_flash_da(
        "pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {\n\
         fn prepare() -> Result<()> {\n\
         check_layout()?;\n\
         Ok(())\n\
         }\n\
         write_range(backup, image)\n\
         }\n",
    );
    repo.assert_reports("does not call `check_layout()?` at its top level");
}

#[test]
fn a_layout_gate_after_write_range_is_reported() {
    let repo = FakeRepo::new("da-layout-after-write", &[]);
    repo.write_flash_da(
        "pub fn write(backup: &[u8], image: &[u8]) -> Result<()> {\n\
         write_range(backup, image)?;\n\
         check_layout()?;\n\
         Ok(())\n\
         }\n",
    );
    repo.assert_reports("calls `write_range` before `check_layout()`");
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
fn gui_without_flash_is_reported() {
    // The GUI writes firmware now, so the missing link is a regression: its
    // write actions would either be a second implementation or absent.
    let repo = FakeRepo::new("gui-no-flash", &[]);
    repo.write_manifest(
        "m241-gui",
        "\
[package]
name = \"m241-gui\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
m241-core = { path = \"../m241-core\" }
",
    );
    let (passed, report) = repo.run();
    assert!(!passed, "gui without flash was accepted:\n{report}");
    assert!(
        report.contains("crates/m241-gui/Cargo.toml"),
        "gui manifest check did not fire:\n{report}"
    );
    assert!(
        report.contains("does not contain m241-flash"),
        "gui cargo tree check did not fire:\n{report}"
    );
}

#[test]
fn a_gui_write_that_bypasses_the_shared_session_is_reported() {
    // A second write implementation in the GUI is the regression this rule
    // exists for: it would carry its own opcodes and could skip the range gate.
    let repo = FakeRepo::new("gui-no-flow", &[]);
    std::fs::write(
        repo.root
            .join("crates")
            .join("m241-gui")
            .join("src")
            .join("device.rs"),
        "pub fn go() { da.write(...); }\n",
    )
    .unwrap();
    repo.assert_reports("no `flow::flash(` call");
}

#[test]
fn a_destructive_opcode_in_gui_code_is_reported() {
    let repo = FakeRepo::new("gui-opcode", &[]);
    std::fs::write(
        repo.root
            .join("crates")
            .join("m241-gui")
            .join("src")
            .join("device.rs"),
        "pub fn go() { send(OPCODE_RESTART); }\n",
    )
    .unwrap();
    repo.assert_reports("destructive command");
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
