#!/usr/bin/env python3
"""Fail if the read-only crate can reach the write path.

Run from the repository root:

    python3 tools/check_safety.py

The split this repository relies on is that `m241-core` reads and
`m241-flash` writes, so a binary built from `m241-core` cannot link or call any
write or erase routine. That is not a property the compiler can verify on its
own: nothing stops someone adding `0xD3` back to a helper in `m241-core`, or
naming the write crate in its manifest. This gate is the mechanical half.

Seven checks, each for a different way the split can rot:

1. **No destructive command in `m241-core`'s sources or tests.** `$PMTK182,6`
   (erase the log) and the DA opcodes `0xD3` / `0xD4` / `0xD5` / `0xD9` (record
   range, erase range, erase and write, restart) — including byte-literal and
   `\\xNN` escape forms — may not appear in code (`src/` or `tests/`). `//`
   comments (both comment-only lines and trailing inline comments) are exempt:
   naming a command in prose is how the module documents that it is absent, and
   a comment cannot be linked into a binary.
2. **No write-shaped identifier in `m241-core`'s sources or tests.** `erase`,
   `erase_*`, `flash_write`, `program` and `program_*` are the vocabulary of the
   write path. `erased` is not on the list on purpose: the log parser calls a
   block of blank flash "erased" while *reading*, and forcing it to avoid the
   word would make the read path harder to follow without making it safer.
   The suffix forms are included because `program_chip` is as much a write
   routine as `program`, and the review that caught it is the review this gate
   exists to make unnecessary.
3. **The dependency runs one way.** `cargo tree -p m241-core` and
   `cargo tree -p m241-gui` must not contain `m241-flash`, and `m241-flash`'s
   tree must contain `m241-core`. Both directions are checked; only one of them
   is the dangerous one, but a check that only forbids the bad direction would
   pass if the good one were never declared.
4. **Neither read-only manifest names the write crate.** Both
   `crates/m241-core/Cargo.toml` and `crates/m241-gui/Cargo.toml` are checked as
   text as well as by `cargo tree`, so a dependency that is declared and then
   optimised out of the resolved graph is still a finding.
5. **DTR/RTS only in the BootROM entry path.** `set_control_lines` pulses the
   reset lines, which at a streaming firmware leaves the unit unresponsive until
   the battery is removed. It may be named only in the exact crate-relative
   paths `src/protocol/brom.rs` (the entry path), `src/protocol/port.rs` (the
   trait) and `src/protocol/simulator.rs` (the fake records it so a test can
   prove it was not pulsed). A second call site is how a "safe because it is
   gated" function stops being gated.
6. **No digest-based acceptance in the strict backup-diff path.**
   `AGENTS.md` §2.3 allows an image in the strict path only when it is the
   user's own backup or differs from it at the recorded patch offsets. A digest
   branch in `check_image` / `all_within` would be a second acceptance route
   that weakens that check. The vendor image is a **separate** fixed-SHA gate
   (`accept_vendor_image`), pinned to one V1.13 release and tested separately;
   this check deliberately does not treat that function as the strict path.
   The rule is textual because a synthetic fixture never hashes to the real
   vendor digest, so an end-to-end test alone cannot catch the old strict-path
   branch coming back.
7. **Every public writer runs the layout gate.** `crates/m241-flash/src/da.rs`'s
   public functions that reach `write_range` or `write_range_accepted` must call
   `check_layout()?` at their own brace depth, before that range call. That call
   is what refuses every start other than the settled `0`; without it the fixed
   range is a comment. Both `DownloadAgent::write` and `write_vendor` are
   covered, and a future public writer is found by its range call rather than by
   its name. The check looks inside each body only, so a call in another
   function, a comment, or an uncalled nested function does not satisfy it.

A rule enforced only by review is a rule that holds until the reviewer is busy.
These are mechanical so they hold every commit.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable

REPO = Path(__file__).resolve().parents[1]
CORE = REPO / "crates" / "m241-core"
CORE_SRC = CORE / "src"
CORE_TESTS = CORE / "tests"
CORE_MANIFEST = CORE / "Cargo.toml"
FLASH = REPO / "crates" / "m241-flash"
FLASH_DA = FLASH / "src" / "da.rs"
GUI = REPO / "crates" / "m241-gui"
GUI_MANIFEST = GUI / "Cargo.toml"

#: Commands that destroy or rewrite what is on the device. `PMTK182,6` erases
#: the log; the four DA opcodes record a write range (`0xD3`), erase a range
#: (`0xD4`), erase-and-write (`0xD5`), and restart the chip (`0xD9`).
DESTRUCTIVE_SYMBOLS = (
    re.compile(r"PMTK182,6"),
    # `0xD3_u8` and `0xD5u8` are valid Rust, so the boundary has to allow an
    # integer suffix. `0xd5a7_9147` (a SHA-256 word elsewhere in the crate) must
    # not match, which is why the lookahead is there.
    re.compile(
        r"(?i)\b0x[dD]([3459])(?:_?(?:[ui](?:8|16|32|64|128|size)))?(?![0-9a-zA-Z_])"
    ),
    # Byte-literal and string-escape forms such as `b'\xd3'`, `b'\xD4'`,
    # `b"\xd5"`, and `"\xD9"`. In Rust `\x` always consumes exactly two hex
    # digits, so `b"\xd5a"` still encodes byte 0xD5 and must match.
    re.compile(r"(?i)\\x[dD]([3459])"),
)

#: Identifiers that only the write path has any business naming.
#:
#: The token must be the whole identifier, with `_` treated as a separator so
#: that `erase`, `erase_log` and `erase_range` all match. A word that merely
#: starts with those letters does not: the log parser's `erased` and
#: `an_erased_block_is_counted_but_not_called_damage` describe a block of blank
#: flash *detected while reading*, and renaming them would make the read path
#: less clear without making it safer. The exemption is a property of the word,
#: not a file allowlist, so it cannot quietly cover a new call site.
DESTRUCTIVE_IDENTIFIERS = re.compile(
    r"(?i)(?:^|[^A-Za-z0-9_])"
    r"(erase(?:_[A-Za-z0-9_]+)?|flash_write|program(?:_[A-Za-z0-9_]+)?)"
    r"(?![A-Za-z0-9_])"
)

#: `set_control_lines` asserts DTR and RTS. Pulsing them at a running firmware
#: leaves the unit unresponsive until the battery is removed, so the only code
#: allowed to call it is the BootROM entry path. The trait declaration and the
#: fake's implementation are the other two places the name legitimately occurs.
#: Matched against exact paths relative to `crates/m241-core` so an unrelated
#: file with the same basename (e.g. `src/log/port.rs`) cannot bypass the gate.
CONTROL_LINE = re.compile(r"\bset_control_lines\b")
CONTROL_LINE_ALLOWED = {
    "src/protocol/brom.rs",
    "src/protocol/port.rs",
    "src/protocol/simulator.rs",
}

#: `m241-flash`'s only image-acceptance rule lives in these functions. A digest
#: comparison here is the vendor bypass `AGENTS.md` §2.3 forbids: the code may
#: name the file's digest in prose elsewhere, but the acceptance path must not
#: read `profile.sha256` or call `firmware::digest::matches`. `all_within` is
#: included because moving the bypass into the byte-diff helper would otherwise
#: escape a check that only looked at `check_image`.
ACCEPTANCE_FUNCTIONS = ("check_image", "all_within")
DIGEST_ACCEPTANCE = re.compile(
    r"(?i)(?:firmware\s*::\s*digest|digest\s*::\s*matches|sha256(?:_hex)?\b)"
)

#: Start of a Rust raw string literal (`r"`, `r#"`, `br#"`, `cr#"`).
RAW_STRING_START = re.compile(r'(?:b|c)?r(#*)"')

#: Every public function that reaches the range programmer must run the layout
#: gate first. `check_layout()` refuses a start other than the settled `0`; if
#: the call moves, disappears, or sits in another function, that write runs
#: ungated. This is intentionally not a name allowlist: it finds both
#: `DownloadAgent::write` and `write_vendor`, and any future public writer that
#: reaches `write_range` or `write_range_accepted`.
PUB_FN = re.compile(r"\bpub\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
WRITE_LAYOUT_CALL = re.compile(r"\bcheck_layout\s*\(\s*\)\s*\?")
WRITE_RANGE_CALL = re.compile(r"\b(write_range|write_range_accepted)\s*\(")


def rust_sources(root: Path) -> list[Path]:
    if not root.is_dir():
        return []
    return sorted(path for path in root.rglob("*.rs") if path.is_file())


def core_rust_sources() -> list[Path]:
    return sorted(rust_sources(CORE_SRC) + rust_sources(CORE_TESTS))


def relative(path: Path) -> str:
    try:
        return str(path.relative_to(REPO)).replace("\\", "/")
    except ValueError:
        return str(path)


def core_relative(path: Path) -> str:
    try:
        return str(path.relative_to(CORE)).replace("\\", "/")
    except ValueError:
        return str(path)


def code_lines(text: str) -> Iterable[tuple[int, str]]:
    """Yield `(line number, code)` with `//` comments stripped.

    Both comment-only lines and trailing `//` comments are removed, while `//`
    inside normal, byte, or raw string literals is preserved so tokens inside
    strings are still inspected. Character literals containing `"` (`'"'` or
    `'\\"'`) are skipped without opening a string, and Rust lifetimes (`'a`) are
    not mistaken for character literals. Block comments (`/* ... */`) are not
    stripped: they are unused in this crate, and failing closed on them is the
    safer direction.
    """
    in_string = False
    raw_hashes: int | None = None

    for number, line in enumerate(text.splitlines(), start=1):
        out: list[str] = []
        i = 0
        n = len(line)
        while i < n:
            if raw_hashes is not None:
                ch = line[i]
                out.append(ch)
                if ch == '"':
                    end = i + 1 + raw_hashes
                    if end <= n and line[i + 1 : end] == "#" * raw_hashes:
                        out.append(line[i + 1 : end])
                        i = end
                        raw_hashes = None
                        continue
                i += 1
            elif in_string:
                ch = line[i]
                out.append(ch)
                if ch == "\\" and i + 1 < n:
                    out.append(line[i + 1])
                    i += 2
                    continue
                if ch == '"':
                    in_string = False
                i += 1
            else:
                if line[i : i + 2] == "//":
                    break
                raw_match = RAW_STRING_START.match(line, i)
                if raw_match and (
                    i == 0 or not (line[i - 1].isalnum() or line[i - 1] == "_")
                ):
                    out.append(raw_match.group(0))
                    raw_hashes = len(raw_match.group(1))
                    i = raw_match.end()
                    continue
                if line[i : i + 3] == "'\"'":
                    out.append("'\"'")
                    i += 3
                    continue
                if line[i : i + 4] == "'\\\"'":
                    out.append("'\\\"'")
                    i += 4
                    continue
                ch = line[i]
                out.append(ch)
                if ch == '"':
                    in_string = True
                i += 1

        code = "".join(out)
        if code.strip():
            yield number, code


def masked_source(text: str) -> str:
    """Return `text` with comments and literal contents blanked.

    Offsets and line breaks survive: replaced characters become spaces, so a
    brace inside a string or a comment cannot close `write`'s body early. The
    same literals `code_lines` recognises are handled, plus block comments,
    because this is used for brace matching over a whole file rather than one
    line at a time.
    """
    out = list(text)
    i = 0
    n = len(text)
    in_string = False
    in_block_comment = False
    raw_hashes: int | None = None

    while i < n:
        ch = text[i]
        if raw_hashes is not None:
            if ch == '"' and text[i + 1 : i + 1 + raw_hashes] == "#" * raw_hashes:
                i += 1 + raw_hashes
                raw_hashes = None
                continue
            if ch != "\n":
                out[i] = " "
            i += 1
        elif in_block_comment:
            if text[i : i + 2] == "*/":
                out[i] = out[i + 1] = " "
                i += 2
                in_block_comment = False
            else:
                if ch != "\n":
                    out[i] = " "
                i += 1
        elif in_string:
            if ch == "\\" and i + 1 < n:
                out[i] = " "
                if text[i + 1] != "\n":
                    out[i + 1] = " "
                i += 2
                continue
            if ch == '"':
                in_string = False
                i += 1
                continue
            if ch != "\n":
                out[i] = " "
            i += 1
        else:
            if text[i : i + 2] == "//":
                while i < n and text[i] != "\n":
                    out[i] = " "
                    i += 1
                continue
            if text[i : i + 2] == "/*":
                out[i] = out[i + 1] = " "
                i += 2
                in_block_comment = True
                continue
            raw_match = RAW_STRING_START.match(text, i)
            if raw_match and (
                i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")
            ):
                raw_hashes = len(raw_match.group(1))
                i = raw_match.end()
                continue
            if text[i : i + 3] == "'\"'":
                i += 3
                continue
            if text[i : i + 4] == "'\\\"'":
                i += 4
                continue
            if ch == "'":
                # A character literal (`'x'`, `'\xNN'`), not a lifetime (`'a`).
                end = i + 1
                if end < n and text[end] == "\\":
                    end += 2
                    while end < n and text[end] != "'":
                        end += 1
                elif end < n:
                    end += 1
                if end < n and text[end] == "'":
                    for blank in range(i + 1, end):
                        out[blank] = " "
                    i = end + 1
                    continue
            if ch == '"':
                in_string = True
            i += 1

    return "".join(out)


def pub_fn_body_span(masked: str, match: re.Match[str]) -> tuple[int, int, int] | str:
    """The span of one `pub fn` and its body in masked source.

    Returns `(function_start, body_open, body_end)`, where `body_open` is the
    index of the body's opening `{` and `body_end` is just past its matching
    `}`. Returning the brace positions lets the caller require a call at the
    body's own level rather than anywhere inside it.
    """
    name = match.group(1)
    opening = masked.find("{", match.end())
    if opening < 0:
        return f"`pub fn {name}` has no body"
    depth = 0
    for index in range(opening, len(masked)):
        if masked[index] == "{":
            depth += 1
        elif masked[index] == "}":
            depth -= 1
            if depth == 0:
                return (match.start(), opening, index + 1)
    return f"`pub fn {name}`'s body is not balanced"


def top_level_source(body: str) -> str:
    """Blank everything below the function body's first brace depth.

    `body` starts at the body's opening `{`. Only code directly inside it
    (depth 1) survives. An uncalled nested function is depth 2 and is blanked,
    so its `check_layout()` cannot satisfy the gate; neither can a call inside
    a conditional block, which is the conservative reading of "the gate runs
    before the write".
    """
    out = list(body)
    depth = 0
    for index, ch in enumerate(body):
        if ch == "{":
            depth += 1
            out[index] = " "
        elif ch == "}":
            out[index] = " "
            depth -= 1
        elif depth != 1 and ch != "\n":
            out[index] = " "
    return "".join(out)


def check_write_layout_gate() -> list[str]:
    """Every public writer must run `check_layout()` before the range call."""
    if not FLASH_DA.is_file():
        return [
            f"{relative(FLASH_DA)} is missing; cannot verify that the write path "
            "runs the layout gate"
        ]

    text = FLASH_DA.read_text(encoding="utf-8")
    masked = masked_source(text)
    problems = []
    writers = 0
    for match in PUB_FN.finditer(masked):
        span = pub_fn_body_span(masked, match)
        if isinstance(span, str):
            problems.append(f"{relative(FLASH_DA)}: {span}")
            continue
        function_start, body_open, body_end = span
        top = top_level_source(masked[body_open:body_end])
        range_call = WRITE_RANGE_CALL.search(top)
        if range_call is None:
            continue
        writers += 1
        name = match.group(1)
        number = text.count("\n", 0, function_start) + 1
        layout = WRITE_LAYOUT_CALL.search(top)
        if layout is None:
            problems.append(
                f"{relative(FLASH_DA)}:{number}: `pub fn {name}` does not call "
                "`check_layout()?` at its top level; the fixed write range is not gated"
            )
        elif layout.start() > range_call.start():
            problems.append(
                f"{relative(FLASH_DA)}:{number}: `pub fn {name}` calls "
                f"`{range_call.group(1)}` before `check_layout()`; the layout gate is not "
                "in front of the write"
            )
    if writers == 0:
        problems.append(
            f"{relative(FLASH_DA)}: no `pub fn` reaches `write_range` or "
            "`write_range_accepted`; cannot verify the layout gate"
        )
    return problems


def check_destructive_symbols() -> list[str]:
    problems = []
    for path in core_rust_sources():
        for number, line in code_lines(path.read_text(encoding="utf-8")):
            for pattern in DESTRUCTIVE_SYMBOLS:
                for match in pattern.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command "
                        f"`{match.group(0)}` belongs in m241-flash, not m241-core"
                    )
    return problems


def check_identifiers() -> list[str]:
    problems = []
    for path in core_rust_sources():
        for number, line in code_lines(path.read_text(encoding="utf-8")):
            for match in DESTRUCTIVE_IDENTIFIERS.finditer(line):
                problems.append(
                    f"{relative(path)}:{number}: write-path identifier "
                    f"`{match.group(1)}` in m241-core"
                )
    return problems


def check_control_lines() -> list[str]:
    """Only the BootROM entry path may assert DTR/RTS."""
    problems = []
    for path in core_rust_sources():
        if core_relative(path) in CONTROL_LINE_ALLOWED:
            continue
        for number, line in code_lines(path.read_text(encoding="utf-8")):
            if CONTROL_LINE.search(line):
                problems.append(
                    f"{relative(path)}:{number}: set_control_lines outside the "
                    f"BootROM entry path (allowed in {', '.join(sorted(CONTROL_LINE_ALLOWED))})"
                )
    return problems


def function_body_range(text: str, name: str) -> tuple[int, int] | None:
    """The 1-based line range of a top-level function body, inclusive.

    `check_safety.py` runs on a rustfmt-formatted tree, so a top-level function
    closes with a `}` in column zero. This is deliberately a range rather than a
    brace counter over strings and comments: the caller only needs "inside this
    function", and a parser would be a second thing to keep right.
    """
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.startswith(f"pub fn {name}") or line.startswith(f"fn {name}"):
            start = index + 1
            break
    if start is None:
        return None
    for index in range(start, len(lines)):
        if lines[index].rstrip() == "}":
            return (start, index + 1)
    return None


def check_flash_acceptance() -> list[str]:
    """`check_image` and `all_within` must accept images only by the backup diff.

    `check_image` is the place the strict path decides that an image is
    acceptable. It must compare the image with the user's backup and permit only
    the recorded patch offsets; a digest comparison here would be a second
    acceptance route. The vendor image is a separate fixed-SHA gate
    (`accept_vendor_image`), not part of this check. A synthetic test fixture
    cannot catch a digest branch in the strict path because it does not hash to
    the real vendor digest, so this check looks at the function source.
    Comments and doc comments are stripped first: the file explains why the
    strict path has no digest route, and that prose must remain allowed.

    The ranges are the `check_image`/`all_within` bodies, not all of `da.rs`:
    `check_da_image` is the DA file's own length/head check, and follow-up W2a
    will legitimately add its SHA-256 there.
    """
    if not FLASH_DA.is_file():
        return [f"{relative(FLASH_DA)} is missing; cannot verify the image acceptance rule"]
    text = FLASH_DA.read_text(encoding="utf-8")
    ranges = []
    for name in ACCEPTANCE_FUNCTIONS:
        body = function_body_range(text, name)
        if body is None:
            return [
                f"{relative(FLASH_DA)}: {name} is missing; cannot verify the image "
                "acceptance rule"
            ]
        ranges.append(body)
    problems = []
    for number, line in code_lines(text):
        if not any(start <= number <= end for start, end in ranges):
            continue
        for match in DIGEST_ACCEPTANCE.finditer(line):
            problems.append(
                f"{relative(FLASH_DA)}:{number}: digest-based image acceptance "
                f"`{match.group(0)}` in the strict path; AGENTS.md §2.3 allows only the backup diff"
            )
    return problems


def cargo_tree(package: str) -> list[str] | None:
    """The package names in `cargo tree -p <package>`, or None when cargo failed.

    Dev and build edges are included: a dev-dependency on the write crate would
    let `cargo test -p m241-core` link the erase, which is exactly the accident
    this gate exists to catch.
    """
    try:
        result = subprocess.run(
            [
                "cargo",
                "tree",
                "-p",
                package,
                "--prefix",
                "none",
                "--no-dedupe",
                # Dev and build edges have to be named: the default `cargo tree`
                # leaves dev-dependencies out, and a dev-dependency on the write
                # crate would let `cargo test -p m241-core` link the erase.
                "--edges",
                "normal,build,dev",
            ],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as err:  # cargo is not installed
        print(f"  [!] cannot run cargo tree: {err}")
        return None
    if result.returncode != 0:
        print(f"  [!] cargo tree -p {package} failed:\n{result.stderr.strip()}")
        return None
    return [line.split()[0] for line in result.stdout.splitlines() if line.strip()]


def check_dependency_direction() -> list[str]:
    problems = []
    manifest = CORE_MANIFEST.read_text(encoding="utf-8")
    for name in ("m241-flash", "m241_flash"):
        if name in manifest:
            problems.append(
                f"{relative(CORE_MANIFEST)}: names `{name}`; the write crate must "
                "depend on the read crate, never the reverse"
            )

    core_tree = cargo_tree("m241-core")
    if core_tree is None:
        problems.append("cargo tree -p m241-core could not be read")
    elif "m241-flash" in core_tree:
        problems.append("m241-core's dependency tree contains m241-flash")

    if GUI_MANIFEST.is_file():
        gui_manifest = GUI_MANIFEST.read_text(encoding="utf-8")
        for name in ("m241-flash", "m241_flash"):
            if name in gui_manifest:
                problems.append(
                    f"{relative(GUI_MANIFEST)}: names `{name}`; m241-gui must not "
                    "depend on the write crate"
                )
        gui_tree = cargo_tree("m241-gui")
        if gui_tree is None:
            problems.append("cargo tree -p m241-gui could not be read")
        elif "m241-flash" in gui_tree:
            problems.append("m241-gui's dependency tree contains m241-flash")
    else:
        problems.append(
            f"{relative(GUI_MANIFEST)} is missing; cannot verify m241-gui dependencies"
        )

    if FLASH.is_dir():
        flash_tree = cargo_tree("m241-flash")
        if flash_tree is None:
            problems.append("cargo tree -p m241-flash could not be read")
        elif "m241-core" not in flash_tree:
            problems.append(
                "m241-flash does not depend on m241-core; the direction is wrong "
                "or the dependency was dropped"
            )
    else:
        problems.append("crates/m241-flash is missing; the write path has no crate")
    return problems


def main() -> int:
    if not CORE_SRC.is_dir():
        print(f"no m241-core sources found at {relative(CORE_SRC)}")
        return 1

    files = core_rust_sources()
    problems = (
        check_destructive_symbols()
        + check_identifiers()
        + check_control_lines()
        + check_flash_acceptance()
        + check_dependency_direction()
        + check_write_layout_gate()
    )
    print(
        f"checked {len(files)} m241-core source/test file(s) for destructive commands, "
        "write identifiers and the dependency direction, and the flash write path for "
        "its layout gate"
    )
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} safety violation(s)")
        return 1
    print(
        "no write path in m241-core or m241-gui; the dependency runs one way; "
        "the strict image path accepts only the backup diff; the vendor gate is a separate "
        "fixed-SHA function; every public writer runs check_layout() first"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
