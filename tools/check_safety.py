#!/usr/bin/env python3
"""Fail if the read-only crate can reach the write path.

Run from the repository root:

    python3 tools/check_safety.py

The split this repository relies on is that `m241-core` reads and
`m241-flash` writes, so a binary built from `m241-core` cannot link or call any
write or erase routine. That is not a property the compiler can verify on its
own: nothing stops someone adding `0xD3` back to a helper in `m241-core`, or
naming the write crate in its manifest. This gate is the mechanical half.

Five checks, each for a different way the split can rot:

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

#: Start of a Rust raw string literal (`r"`, `r#"`, `br#"`, `cr#"`).
RAW_STRING_START = re.compile(r'(?:b|c)?r(#*)"')


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
        + check_dependency_direction()
    )
    print(
        f"checked {len(files)} m241-core source/test file(s) for destructive commands, "
        "write identifiers and the dependency direction"
    )
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} safety violation(s)")
        return 1
    print("no write path in m241-core or m241-gui; the dependency runs one way")
    return 0


if __name__ == "__main__":
    sys.exit(main())
