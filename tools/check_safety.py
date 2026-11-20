#!/usr/bin/env python3
"""Fail if the read-only crate can reach the write path.

Run from the repository root:

    python3 tools/check_safety.py

The split this repository relies on is that `m241-core` reads and
`m241-flash` writes, so a binary built from `m241-core` cannot erase or program
a logger. That is not a property the compiler can verify on its own: nothing
stops someone adding `0xD3` back to a helper in `m241-core`, or naming the
write crate in its manifest. This gate is the mechanical half.

Five checks, each for a different way the split can rot:

1. **No destructive command in `m241-core`'s sources.** `$PMTK182,6` (erase the
   log) and the DA opcodes `0xD3` / `0xD5` / `0xD9` (record range, erase and
   write, restart) may not appear in code. Comment lines are exempt: naming a
   command in prose is how the module documents that it is absent, and a doc
   comment cannot be linked into a binary.
2. **No write-shaped identifier in `m241-core`'s sources.** `erase`,
   `erase_*`, `flash_write`, `program` and `program_*` are the vocabulary of the
   write path. `erased` is not on the list on purpose: the log parser calls a
   block of blank flash "erased" while *reading*, and forcing it to avoid the
   word would make the read path harder to follow without making it safer.
   The suffix forms are included because `program_chip` is as much a write
   routine as `program`, and the review that caught it is the review this gate
   exists to make unnecessary.
3. **The dependency runs one way.** `cargo tree -p m241-core` must not contain
   `m241-flash`, and `m241-flash`'s tree must contain `m241-core`. Both
   directions are checked; only one of them is the dangerous one, but a check
   that only forbids the bad direction would pass if the good one were never
   declared.
4. **The manifest does not name the write crate.** Checked as text as well as
   by `cargo tree`, so a dependency that is declared and then optimised out of
   the resolved graph is still a finding.
5. **DTR/RTS only in the BootROM entry path.** `set_control_lines` pulses the
   reset lines, which at a streaming firmware leaves the unit unresponsive until
   the battery is removed. It may be named only in `brom.rs` (the entry path),
   `port.rs` (the trait) and `simulator.rs` (the fake records it so a test can
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
CORE_MANIFEST = CORE / "Cargo.toml"
FLASH = REPO / "crates" / "m241-flash"

#: Commands that destroy or rewrite what is on the device. `PMTK182,6` erases
#: the log; the three DA opcodes record a write range, erase-and-write it, and
#: restart the chip.
DESTRUCTIVE_SYMBOLS = (
    re.compile(r"PMTK182,6"),
    # `0xD3_u8` and `0xD5u8` are valid Rust, so the boundary has to allow an
    # integer suffix. `0xd5a7_9147` (a SHA-256 word elsewhere in the crate) must
    # not match, which is why the lookahead is there.
    re.compile(r"(?i)\b0x[dD]([359])(?:_?(?:u8|u16|u32|i32))?(?![0-9a-zA-Z_])"),
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
CONTROL_LINE = re.compile(r"\bset_control_lines\b")
CONTROL_LINE_ALLOWED = {"brom.rs", "port.rs", "simulator.rs"}

def rust_sources(root: Path) -> list[Path]:
    if not root.is_dir():
        return []
    return sorted(path for path in root.rglob("*.rs") if path.is_file())


def relative(path: Path) -> str:
    try:
        return str(path.relative_to(REPO)).replace("\\", "/")
    except ValueError:
        return str(path)


def code_lines(text: str) -> Iterable[tuple[int, str]]:
    """Yield `(line number, code)` with comment-only lines removed.

    A line that *starts* with `//` is prose: it explains the protocol and cannot
    be linked. A line with code before a trailing comment is code, because the
    forbidden token would be on the code side of it. Block comments are not used
    in this crate; if one appears, its lines are not `//`-prefixed and would be
    reported, which is the safe direction to fail in.
    """
    for number, line in enumerate(text.splitlines(), start=1):
        if line.lstrip().startswith("//"):
            continue
        yield number, line


def check_destructive_symbols() -> list[str]:
    problems = []
    for path in rust_sources(CORE_SRC):
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
    for path in rust_sources(CORE_SRC):
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
    for path in rust_sources(CORE_SRC):
        if path.name in CONTROL_LINE_ALLOWED:
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

    files = rust_sources(CORE_SRC)
    problems = (
        check_destructive_symbols()
        + check_identifiers()
        + check_control_lines()
        + check_dependency_direction()
    )
    print(
        f"checked {len(files)} m241-core source file(s) for destructive commands, "
        "write identifiers and the dependency direction"
    )
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} safety violation(s)")
        return 1
    print("no write path in m241-core; the dependency runs one way")
    return 0


if __name__ == "__main__":
    sys.exit(main())
