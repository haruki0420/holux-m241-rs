#!/usr/bin/env python3
"""Fail if the interface breaks one of its three design invariants.

Run from the repository root:

    python3 tools/check_design.py

DESIGN.md states three rules that are cheap to break and expensive to notice:

1. **Amber is reserved.** ``theme::DANGER`` may appear only in the destructive
   module and only where DESIGN.md §5 says it may: the destructive button, the
   write step, and the undefined-device state. Anywhere else it stops meaning
   "this can lose the device" and becomes decoration.
2. **One corner radius.** The product has exactly one, ``theme::RADIUS``; a
   literal radius is a second visual language.
3. **Colour is never the only signal.** A status colour must sit next to a text
   label, so the meaning survives a monochrome screenshot and a colour-blind
   reader.

A rule enforced only by review is a rule that holds until the reviewer is busy.
These are mechanical so they hold every commit.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
GUI = REPO / "crates" / "m241-gui" / "src"

#: The only file allowed to name amber directly.
PALETTE_FILE = GUI / "theme.rs"

#: Modules where amber is allowed to be *used*. Add to this list only with a
#: DESIGN.md change that says what the new use means.
DESTRUCTIVE_MODULES = ("screens/firmware.rs",)

AMBER_PATTERN = re.compile(r"\bDANGER\b")
HEX_PATTERN = re.compile(r"Color32::from_rgb\(\s*0x([0-9A-Fa-f]{2})\s*,\s*0x([0-9A-Fa-f]{2})\s*,\s*0x([0-9A-Fa-f]{2})\s*\)")
RADIUS_LITERAL = re.compile(r"(?:CornerRadius::same|corner_radius\s*=\s*CornerRadius::same)\(\s*(\d+)\s*\)")
STATUS_COLOURS = ("OK", "ERR", "DANGER", "theme::OK", "theme::ERR", "theme::DANGER")

#: 0xE3A008 as separate channels, to catch the same colour written out by hand.
AMBER_CHANNELS = (0xE3, 0xA0, 0x08)


def rust_files() -> list[Path]:
    if not GUI.is_dir():
        return []
    return sorted(path for path in GUI.rglob("*.rs") if path.is_file())


def relative(path: Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def line_of(text: str, index: int) -> int:
    return text.count("\n", 0, index) + 1


def check_amber(files: list[Path]) -> list[str]:
    problems = []
    for path in files:
        if path == PALETTE_FILE:
            continue
        rel = relative(path).replace("\\", "/")
        text = path.read_text(encoding="utf-8")
        allowed = any(module in rel for module in DESTRUCTIVE_MODULES)

        for match in AMBER_PATTERN.finditer(text):
            number = line_of(text, match.start())
            if allowed:
                continue
            problems.append(
                f"{rel}:{number}: amber (DANGER) used outside {', '.join(DESTRUCTIVE_MODULES)}"
            )

        for match in HEX_PATTERN.finditer(text):
            channels = tuple(int(group, 16) for group in match.groups())
            if channels == AMBER_CHANNELS:
                number = line_of(text, match.start())
                problems.append(
                    f"{rel}:{number}: amber written as a literal instead of theme::DANGER"
                )
    return problems


def check_radius(files: list[Path]) -> list[str]:
    """Flag any literal corner radius other than the token.

    A literal that happens to equal ``theme::RADIUS`` is still a violation:
    the next person changes the token and this one silently keeps the old
    shape. The only correct radius is the reference to it.
    """
    problems = []
    for path in files:
        if path.parent.name == "src" and path.name == "theme.rs":
            continue  # the token itself is defined here
        text = path.read_text(encoding="utf-8")
        for match in RADIUS_LITERAL.finditer(text):
            number = line_of(text, match.start())
            problems.append(
                f"{relative(path)}:{number}: literal corner radius {match.group(1)}; use theme::RADIUS"
            )
    return problems


def check_status_colour_has_a_label() -> list[str]:
    """A status colour must appear on a line that also carries text.

    The check is deliberately shallow: it looks for a `Color32`/theme status
    colour used in a paint call whose arguments contain a string literal. That
    catches `painter.text(..., ERR, "failed")` and flags a bare
    `rect_filled(rect, RADIUS, ERR)`.
    """
    problems = []
    for path in rust_files():
        if path.parent.name == "src" and path.name == "theme.rs":
            continue
        text = path.read_text(encoding="utf-8")
        for name in STATUS_COLOURS:
            for match in re.finditer(rf"\b{re.escape(name)}\b", text):
                number = line_of(text, match.start())
                line = text.splitlines()[number - 1]
                # Declarations and imports are not uses.
                if line.lstrip().startswith(("use ", "pub const", "pub use")):
                    continue
                if '"' in line:
                    continue
                problems.append(
                    f"{relative(path)}:{number}: status colour {name} without a text label on the same line"
                )
    return problems


def main() -> int:
    files = rust_files()
    if not files:
        print("no GUI sources found; nothing to check")
        return 0

    problems = check_amber(files) + check_radius(files) + check_status_colour_has_a_label()
    print(f"checked {len(files)} GUI source file(s) against 3 design invariants")
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} design invariant violation(s)")
        return 1
    print("all design invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
