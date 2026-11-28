#!/usr/bin/env python3
"""Fail if the read-only crate can reach the write path, or the GUI can bypass it.

Run from the repository root:

    python3 tools/check_safety.py

The split this repository relies on is that `m241-core` reads and
`m241-flash` writes, so a binary built from `m241-core` cannot link or call any
write or erase routine. That is not a property the compiler can verify on its
own: nothing stops someone adding `0xD3` back to a helper in `m241-core`, or
naming the write crate in its manifest. This gate is the mechanical half.

The GUI is the other half. It is allowed to write (the user's explicit
decision), so the gate checks the direction that remains: the GUI must reach the
write path through the shared session, must build its `WRITE` / `PROBE`
confirmations where the typed word is checked, and must not carry a destructive
opcode of its own.

Twelve checks, each for a different way the split can rot:


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
3. **The dependency runs one way, from the reader to the writer.** `m241-core`
   must not contain `m241-flash`; `m241-flash` must contain `m241-core`; and
   `m241-gui` must contain both. The GUI writes firmware now — the user's
   explicit decision — so it is required to link the write crate rather than
   forbidden from it. What must never happen is the reverse edge: a read-only
   binary that can reach the erase.
4. **The manifests match the graph.** `crates/m241-core/Cargo.toml` must not
   name the write crate, and `crates/m241-gui/Cargo.toml` must name it, checked
   as text as well as by `cargo tree`, so a dependency that is declared and then
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
   (`accept_vendor_image`), pinned to the official V1.13 and V1.12 releases and
   tested separately; this check deliberately does not treat that function as
   the strict path. The rule is textual because a synthetic fixture never hashes
   to the real vendor digest, so an end-to-end test alone cannot catch the old
   strict-path branch coming back.
7. **Every public writer runs the layout gate.** `crates/m241-flash/src/da.rs`'s
   public functions that reach `write_range` or `write_range_accepted` must call
   `check_layout()?` at their own brace depth, before that range call. That call
   is what refuses every start other than the settled `0`; without it the fixed
   range is a comment. `DownloadAgent::write`, `write_vendor` and
   `write_unverified` are all covered, and any future public writer is found by
   its range call rather than by its name. The check looks inside each body
   only, so a call in another function, a comment, or an uncalled nested
   function does not satisfy it.
8. **The GUI's write goes through the shared session and its own typed word.**
   `crates/m241-gui/src/device.rs` must call `flow::flash(`, and the firmware
   screen must build the confirmation from its own typed word
   (`Confirmation::granted` plus `Self::CONFIRMATION`). Without the first, the
   GUI would be a second write implementation; without the second, a path could
   write without the user typing anything. The connect screen must do the same
   for `fw probe-da` (`PROBE_CONFIRMATION` plus `Confirmation::granted`), and
   `Confirmation::granted` / `Job::Flash` may be constructed only in the files
   that own those actions. Destructive opcodes (`0xD3`, `0xD4`, `0xD5`,
   `0xD9`, `PMTK182,6`) must not appear in the GUI's code at all: it reaches
   them only through `m241-flash`. The opcode scan has two halves because the
   forms live in different places: identifiers and bare integer literals are
   scanned with comments and string contents blanked (the screen says `0xD9` in
   its operator prose on purpose), while `\\xNN` escapes and `$PMTK182,6` can
   only appear inside a string literal and are scanned with comments stripped
   but strings kept. This check proves the token is built in the right file, not
   that the user typed the word: the action's own re-check and its unit test are
   what refuse an untyped write, and the gate says so rather than implying more.
9. **The DA digest token has one production constructor.** `DaDigest` is what
   `check_da_image_against` / `DaLoader::load_against` take, and the only
   constructor that exists in a normal build is `DaDigest::vendor()`, which is
   the pinned vendor `DA_v3.8.4.bin` digest. `DaDigest::for_test_da` is the one
   other constructor, for this crate's tests and a dependent crate's end-to-end
   tests; it must stay directly gated by
   `#[cfg(any(test, feature = "simulator"))]`. If the attribute comes off, a
   shipped binary could build a token for any digest, and `AGENTS.md` §2.5's
   "the DA must be the vendor image" would stop being a property of the type.
10. **`m241-app` links `m241-flash` and routes writes through `flow::flash`.**
    With `APP_MAY_LINK_FLASH = True`, `crates/m241-app/Cargo.toml` and its
    resolved `cargo tree` must contain `m241-flash`. `crates/m241-app/src/device.rs`
    must call `flow::flash(` and guard `Confirmation::granted(` with `"WRITE"`
    and `"PROBE"`, and no other Rust source in `m241-app` may construct
    `Confirmation::granted(`.
11. **`m241-app` and its frontend carry no destructive command.** The same
    `0xD3` / `0xD4` / `0xD5` / `0xD9` / `$PMTK182,6` scan (plus `OPCODE_*` in
    Rust) runs over `crates/m241-app/src` and `crates/m241-app/ui/src`, with
    the same two passes (masked source for bare literals, comments stripped
    for escapes and the PMTK sentence).
12. **Serial port opening in `m241-app` is centralised in `src/device.rs`.**
    Every `Port::open` in `crates/m241-app/src` must sit in `src/device.rs`,
    `commands::device_info` must call `device::probe(`, and any direct
    `serialport` constructor (`SerialPort::open`, `serialport::new`) is
    refused.

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
APP = REPO / "crates" / "m241-app"
APP_SRC = APP / "src"
APP_UI_SRC = APP / "ui" / "src"
APP_MANIFEST = APP / "Cargo.toml"

#: Whether `m241-app` is allowed to link the write crate.
#:
#: Step 2A wires the Firmware screen's backend in `m241-app`, so this flag is
#: `True`: `m241-app` links `m241-flash` and reaches flash exclusively through
#: `m241_flash::flow::flash`, with typed `"WRITE"` / `"PROBE"` confirmation
#: guards centralised in `crates/m241-app/src/device.rs` and no destructive
#: opcodes of its own.
APP_MAY_LINK_FLASH = True

#: Opening a port is the one operation that can reach the device. It is allowed
#: only inside `crates/m241-app/src/device.rs`.
APP_PORT_OPEN = re.compile(r"\bPort\s*::\s*open\s*\(")
APP_OTHER_PORT_OPEN = re.compile(
    r"\b(?:SerialPort\s*::\s*(?:open|open_native)|serialport\s*::\s*(?:open|new)|"
    r"serialport\s*::\s*TTYPort\s*::\s*open)\s*\("
)

#: `m241-app` write-route checks (mirroring the GUI's `check_gui_write_route`).
APP_FLOW_CALL = re.compile(r"\b(?:m241_flash\s*::\s*)?flow\s*::\s*flash\s*\(")
APP_CONFIRMATION = re.compile(r"\bConfirmation\s*::\s*granted\s*\(")
APP_WRITE_GUARD = re.compile(r'"WRITE"')
APP_PROBE_GUARD = re.compile(r'"PROBE"')

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

#: Destructive vocabulary the GUI must not carry itself. It reaches these
#: through `m241-flash`; naming one here would be a second write implementation.
#:
#: The scan is split by where each form can appear. Identifiers and bare integer
#: literals are matched against masked source (comments and string contents
#: blanked), because the screen says `0xD9` / `0xD5` in operator prose on
#: purpose. `\xNN` escapes and `$PMTK182,6` can only ever appear inside a string
#: literal, so they are matched against comment-stripped source where strings are
#: kept — masking both was the bug that made those two patterns unfireable.
GUI_DESTRUCTIVE_CODE = (
    re.compile(r"\bOPCODE_(?:MEM_CMD|WRITE|RESTART|ERASE_RANGE|READ_RANGE)\b"),
    re.compile(
        r"(?i)\b0x[dD]([3459])(?:_?(?:[ui](?:8|16|32|64|128|size)))?(?![0-9a-zA-Z_])"
    ),
)
GUI_DESTRUCTIVE_TEXT = (
    re.compile(r"(?i)\\x[dD][3459]"),
    re.compile(r"PMTK182,6"),
)

#: GUI files whose `0xD9`-shaped literal is data, not an opcode. `theme.rs` names
#: RGB channels (`Color32::from_rgb(0xD9, 0x85, 0xA8)`), and the log screen and its
#: render test hold bytes of a journal fixture the parser is tested against. The
#: exemption is path- and literal-scoped on purpose: every other GUI file is
#: scanned, so the rule stays "no opcode anywhere it could become code" instead
#: of narrowing the scan to the three files that talk to the device today. A test
#: file is exempt for the literal only: the escape and `PMTK182,6` scans still
#: run there, and tests are not shipped.
GUI_OPCODE_LITERAL_EXEMPT = {
    "src/theme.rs",
    "src/screens/log.rs",
    "tests/render.rs",
}

#: The GUI's write path must be the shared session, not a second implementation.
GUI_FLOW_CALL = re.compile(r"\bflow\s*::\s*flash\s*\(")
GUI_CONFIRMATION = re.compile(r"\bConfirmation\s*::\s*granted\s*\(")
GUI_WORD = re.compile(r"\bCONFIRMATION\b")
GUI_PROBE_WORD = re.compile(r"\bPROBE_CONFIRMATION\b")

#: Only the screens that own the typed word may construct the token or the
#: flash job: `firmware.rs` for `WRITE`, `connect.rs` for `PROBE`, and
#: `device.rs` because it is where the job enum, its dispatch and its own test
#: fixtures live. A construction anywhere else is a call path that reaches the
#: writer without the screen's check — the regression `safety_gate.rs` records as
#: the gate's own mutation. `device.rs` holds no production token construction:
#: `run` only passes the token a screen already built, and the write job itself
#: cannot be constructed outside `firmware.rs` (next rule).
GUI_CONFIRMATION_ALLOWED = {
    "src/screens/firmware.rs",
    "src/screens/connect.rs",
    "src/device.rs",
}
GUI_FLASH_JOB = re.compile(r"\bJob\s*::\s*Flash\b")
GUI_FLASH_JOB_ALLOWED = {"src/device.rs", "src/screens/firmware.rs"}

#: The one constructor of the DA digest token that a shipped build must not
#: have. `DaDigest::vendor()` is the pinned value; this is the test-only one.
DA_DIGEST_TEST_CTOR = re.compile(r"\bpub\s+fn\s+for_test_da\s*\(")
DA_DIGEST_TEST_CFG = re.compile(
    r'^\s*#\[cfg\(any\(test,\s*feature\s*=\s*"simulator"\)\)\]\s*$'
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
WRITE_RANGE_CALL = re.compile(
    r"\b(write_range(?:_accepted)?(?:_with_progress)?)\s*\("
)


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
    `check_da_image` is the DA file's own check, and its full SHA-256 (added by
    follow-up W2a) legitimately lives there.
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


def check_da_digest_gate() -> list[str]:
    """The test-only DA digest constructor must stay cfg-gated.

    `DaDigest` is the token the DA check and the loader take instead of a digest
    string. A normal build can construct one only with `DaDigest::vendor()`, the
    pinned vendor `DA_v3.8.4.bin` digest, which is what makes "only the vendor DA
    is loaded" a property of the type rather than a rule the call sites follow.
    `DaDigest::for_test_da` exists for this crate's tests and a dependent crate's
    end-to-end tests (the simulator-feature pattern `FakeDa` uses) and must stay
    behind `#[cfg(any(test, feature = "simulator"))]`. A type cannot be checked
    for "how many constructors does the shipped build see", so the gate reads the
    attribute: without it, any downstream build could require any digest.
    """
    if not FLASH_DA.is_file():
        return [f"{relative(FLASH_DA)} is missing; cannot verify the DA digest gate"]
    lines = FLASH_DA.read_text(encoding="utf-8").splitlines()
    problems = []
    found = False
    for index, line in enumerate(lines):
        if not DA_DIGEST_TEST_CTOR.search(line):
            continue
        found = True
        previous = lines[index - 1] if index > 0 else ""
        if not DA_DIGEST_TEST_CFG.match(previous):
            problems.append(
                f"{relative(FLASH_DA)}:{index + 1}: `DaDigest::for_test_da` is not directly "
                'gated by `#[cfg(any(test, feature = "simulator"))]`; a shipped build could '
                "then name any digest as the DA's"
            )
    if not found:
        problems.append(
            f"{relative(FLASH_DA)}: `DaDigest::for_test_da` is missing; cannot verify that a "
            "shipped build has only the pinned DA digest"
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
        if "m241-flash" not in gui_manifest and "m241_flash" not in gui_manifest:
            problems.append(
                f"{relative(GUI_MANIFEST)}: does not name `m241-flash`; the GUI's write "
                "actions must go through the shared write crate"
            )
        gui_tree = cargo_tree("m241-gui")
        if gui_tree is None:
            problems.append("cargo tree -p m241-gui could not be read")
        else:
            if "m241-flash" not in gui_tree:
                problems.append(
                    "m241-gui's dependency tree does not contain m241-flash; the GUI "
                    "cannot reach the shared write session"
                )
            if "m241-core" not in gui_tree:
                problems.append("m241-gui's dependency tree does not contain m241-core")
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


def gui_relative(path: Path) -> str:
    """The path relative to `crates/m241-gui`, or the absolute path if outside."""
    try:
        return str(path.relative_to(GUI)).replace("\\", "/")
    except ValueError:
        return str(path)


def check_gui_write_route() -> list[str]:
    """The GUI must write through `m241-flash` and its own typed words.

    The GUI is allowed to reach the write path (the user's decision), so the
    gate that replaces "must not link the writer" is "must use the same writer,
    and must not carry the destructive bytes itself". Both halves are textual:
    the call graph is what the property is about, and a source check is what
    survives a refactor that moves the call somewhere else.

    What this check does and does not prove: it proves `flow::flash(` is called,
    that `firmware.rs` / `connect.rs` build their confirmation tokens from a
    typed-word constant, and that no other GUI file constructs the token or the
    flash job. It cannot prove the user typed the word — the action re-checks
    the field (`FirmwareScreen::start_write` / `ConnectScreen::start_probe_da`)
    and their unit tests are what refuse an untyped call. This docstring says so
    because a gate that claims more than it checks is how the previous version
    stayed green while two of its three opcode patterns could never fire.
    """
    problems = []
    device = GUI / "src" / "device.rs"
    firmware = GUI / "src" / "screens" / "firmware.rs"
    connect = GUI / "src" / "screens" / "connect.rs"
    if not device.is_file():
        problems.append(
            f"{relative(device)} is missing; cannot verify that the GUI writes through flow::flash"
        )
    else:
        text = device.read_text(encoding="utf-8")
        if GUI_FLOW_CALL.search(text) is None:
            problems.append(
                f"{relative(device)}: no `flow::flash(` call; the GUI must use the "
                "shared session rather than a second write path"
            )
    if not firmware.is_file():
        problems.append(
            f"{relative(firmware)} is missing; cannot verify the typed confirmation"
        )
    else:
        text = firmware.read_text(encoding="utf-8")
        if GUI_CONFIRMATION.search(text) is None or GUI_WORD.search(text) is None:
            problems.append(
                f"{relative(firmware)}: does not build the confirmation from its typed "
                "word; a write route without `WRITE` must not exist"
            )
    if not connect.is_file():
        problems.append(
            f"{relative(connect)} is missing; cannot verify the `PROBE` confirmation "
            "of the DA probe (the DA runs in RAM and can program flash itself)"
        )
    else:
        text = connect.read_text(encoding="utf-8")
        if GUI_CONFIRMATION.search(text) is None or GUI_PROBE_WORD.search(text) is None:
            problems.append(
                f"{relative(connect)}: does not build the DA-probe confirmation from its "
                "typed `PROBE` word; a probe route without it loads the DA unacknowledged"
            )

    for path in rust_sources(GUI / "src") + rust_sources(GUI / "tests"):
        relative_path = gui_relative(path)
        text = path.read_text(encoding="utf-8")
        masked = masked_source(text)
        # Identifiers are safe to scan everywhere: `OPCODE_*` has no prose use.
        for number, line in enumerate(masked.splitlines(), start=1):
            for match in GUI_DESTRUCTIVE_CODE[0].finditer(line):
                problems.append(
                    f"{relative(path)}:{number}: destructive command "
                    f"`{match.group(0)}` in the GUI; reach flash through m241-flash"
                )
        # Bare integer literals are code, so masked source hides the operator
        # prose that names `0xD9`; the two data uses are exempt and listed.
        if relative_path not in GUI_OPCODE_LITERAL_EXEMPT:
            for number, line in enumerate(masked.splitlines(), start=1):
                for match in GUI_DESTRUCTIVE_CODE[1].finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command "
                        f"`{match.group(0)}` in the GUI; reach flash through m241-flash"
                    )
        # Escapes and the PMTK erase sentence only exist inside string literals,
        # so they are scanned with comments stripped and strings kept.
        for number, line in code_lines(text):
            for pattern in GUI_DESTRUCTIVE_TEXT:
                for match in pattern.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command "
                        f"`{match.group(0)}` in the GUI; reach flash through m241-flash"
                    )
        # The token and the flash job may be built only by the screens that own
        # the typed word (and device.rs, which names the enum).
        if relative_path not in GUI_CONFIRMATION_ALLOWED:
            for number, line in enumerate(masked.splitlines(), start=1):
                for match in GUI_CONFIRMATION.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: `Confirmation::granted(` outside the "
                        "screens that check the typed word; there is no --yes equivalent "
                        "in the GUI"
                    )
        if relative_path not in GUI_FLASH_JOB_ALLOWED:
            for number, line in enumerate(masked.splitlines(), start=1):
                for match in GUI_FLASH_JOB.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: `Job::Flash` constructed outside the "
                        "Firmware screen; the write must come from its typed word"
                    )
    return problems


def app_relative(path: Path) -> str:
    """The path relative to `crates/m241-app`, or the absolute path if outside."""
    try:
        return str(path.relative_to(APP)).replace("\\", "/")
    except ValueError:
        return str(path)


def check_app_dependency() -> list[str]:
    """`m241-app` must stay structurally read-only until P3.

    The compiler cannot verify this: adding one line to `Cargo.toml` is enough to
    link the write crate. The two halves are the manifest text (a dependency
    declared and then optimised out is still a finding) and the resolved tree
    (a dependency inherited through another crate would not appear in the
    manifest).

    When P3 flips [`APP_MAY_LINK_FLASH`], the clause inverts: the write crate
    must be present, and the writer route checks (the same shape as
    [`check_gui_write_route`]) must be added in the same change. Until then the
    app is a read-only binary and this check is what says so.
    """
    problems = []
    if not APP_MANIFEST.is_file():
        return [f"{relative(APP_MANIFEST)} is missing; cannot verify m241-app dependencies"]
    manifest = APP_MANIFEST.read_text(encoding="utf-8")
    # The manifest explains in a comment why the write crate is absent, so the
    # name is matched against code only. A TOML `#` comment cannot declare a
    # dependency, and treating the prose as a declaration would make the gate
    # fail on the sentence that documents the rule.
    manifest_code = "\n".join(
        line.split("#", 1)[0] for line in manifest.splitlines()
    )
    named = "m241-flash" in manifest_code or "m241_flash" in manifest_code

    if APP_MAY_LINK_FLASH:
        if not named:
            problems.append(
                f"{relative(APP_MANIFEST)}: does not name `m241-flash` although "
                "APP_MAY_LINK_FLASH is True; the write route must go through the shared "
                "write crate, and the write-route clauses must be added with it"
            )
    elif named:
        problems.append(
            f"{relative(APP_MANIFEST)}: names `m241-flash` while APP_MAY_LINK_FLASH is "
            "False; the app is read-only until P3, and the write crate must not be "
            "linkable from it"
        )

    tree = cargo_tree("m241-app")
    if tree is None:
        problems.append("cargo tree -p m241-app could not be read")
    elif APP_MAY_LINK_FLASH and "m241-flash" not in tree:
        problems.append(
            "m241-app's dependency tree does not contain m241-flash although "
            "APP_MAY_LINK_FLASH is True"
        )
    elif not APP_MAY_LINK_FLASH and "m241-flash" in tree:
        problems.append(
            "m241-app's dependency tree contains m241-flash; the read-only binary can "
            "reach the write path"
        )
    return problems


def check_app_write_route() -> list[str]:
    """`m241-app` must write through `m241-flash::flow::flash` and typed words.

    When `APP_MAY_LINK_FLASH` is True, `crates/m241-app/src/device.rs` must call
    `flow::flash(`, guard `Confirmation::granted(` with `"WRITE"` and `"PROBE"`,
    and no other Rust source file in `m241-app` may construct `Confirmation::granted(`.
    """
    if not APP_MAY_LINK_FLASH:
        return []
    problems = []
    device = APP_SRC / "device.rs"
    if not device.is_file():
        return [
            f"{relative(device)} is missing; cannot verify that m241-app writes through flow::flash"
        ]
    text = device.read_text(encoding="utf-8")
    masked = masked_source(text)
    if APP_FLOW_CALL.search(masked) is None:
        problems.append(
            f"{relative(device)}: no `flow::flash(` call; m241-app must use the "
            "shared m241-flash session rather than a second write path"
        )
    if APP_CONFIRMATION.search(masked) is None:
        problems.append(
            f"{relative(device)}: no `Confirmation::granted(` call; cannot verify "
            "the typed confirmation guard"
        )
    device_code = "\n".join(line for _, line in code_lines(text))
    if APP_WRITE_GUARD.search(device_code) is None:
        problems.append(
            f"{relative(device)}: missing `\"WRITE\"` confirmation guard; a write route "
            "without `WRITE` must not exist"
        )
    if APP_PROBE_GUARD.search(device_code) is None:
        problems.append(
            f"{relative(device)}: missing `\"PROBE\"` confirmation guard; a DA probe route "
            "without `PROBE` must not exist"
        )

    for path in rust_sources(APP_SRC):
        if path == device:
            continue
        other_masked = masked_source(path.read_text(encoding="utf-8"))
        for number, line in enumerate(other_masked.splitlines(), start=1):
            if APP_CONFIRMATION.search(line):
                problems.append(
                    f"{relative(path)}:{number}: `Confirmation::granted(` outside "
                    "crates/m241-app/src/device.rs; confirmation checks are centralised there"
                )
    return problems


def check_app_destructive_code() -> list[str]:
    """No destructive command may appear in `m241-app`, Rust or frontend.

    `0xD3` / `0xD4` / `0xD5` / `0xD9`, `OPCODE_*` and `$PMTK182,6` have no
    business in `m241-app` code: all destructive bytes live in `m241-flash`.
    Prose in `//` comments is exempt; the scan strips `//` comments and keeps
    string contents for escapes and `$PMTK182,6`, while bare integer literals
    and `OPCODE_*` identifiers are matched against masked source.
    """
    problems = []
    for path in rust_sources(APP_SRC):
        text = path.read_text(encoding="utf-8")
        masked = masked_source(text)
        for number, line in enumerate(masked.splitlines(), start=1):
            for pattern in (GUI_DESTRUCTIVE_CODE[0], DESTRUCTIVE_SYMBOLS[1]):
                for match in pattern.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command `{match.group(0)}` "
                        "in m241-app; reach flash only through m241-flash"
                    )
        for number, line in code_lines(text):
            for pattern in (DESTRUCTIVE_SYMBOLS[0], DESTRUCTIVE_SYMBOLS[2]):
                for match in pattern.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command "
                        f"`{match.group(0)}` in m241-app; reach flash only through m241-flash"
                    )

    ui_root = APP_UI_SRC
    if not ui_root.is_dir():
        problems.append(
            f"{relative(ui_root)} is missing; cannot verify the frontend carries no "
            "destructive command"
        )
        return problems
    frontend = sorted(
        path
        for path in ui_root.rglob("*")
        if path.is_file() and path.suffix in {".ts", ".tsx"}
    )
    for path in frontend:
        text = path.read_text(encoding="utf-8")
        masked = masked_source(text)
        for number, line in enumerate(masked.splitlines(), start=1):
            for match in DESTRUCTIVE_SYMBOLS[1].finditer(line):
                problems.append(
                    f"{relative(path)}:{number}: destructive command `{match.group(0)}` "
                    "in the frontend; the webview has no serial route"
                )
        for number, line in code_lines(text):
            for pattern in (DESTRUCTIVE_SYMBOLS[0], DESTRUCTIVE_SYMBOLS[2]):
                for match in pattern.finditer(line):
                    problems.append(
                        f"{relative(path)}:{number}: destructive command "
                        f"`{match.group(0)}` in the frontend; the webview has no serial "
                        "route"
                    )
    return problems


def check_app_port_path() -> list[str]:
    """Serial port opening in `m241-app` must be centralised in `src/device.rs`.

    Every `Port::open` call in `crates/m241-app/src` must live inside
    `src/device.rs`; any other `Port::open` or raw `serialport` constructor is
    a finding.
    """
    problems = []
    occurrences: list[tuple[Path, int]] = []
    for path in rust_sources(APP_SRC):
        text = path.read_text(encoding="utf-8")
        for number, line in code_lines(text):
            for _ in APP_PORT_OPEN.finditer(line):
                occurrences.append((path, number))
            for match in APP_OTHER_PORT_OPEN.finditer(line):
                problems.append(
                    f"{relative(path)}:{number}: `{match.group(0)}` opens a serial port "
                    "outside src/device.rs"
                )

    expected = APP / "src" / "device.rs"
    if not occurrences:
        problems.append(
            f"{relative(expected)}: no `Port::open(` call; cannot verify the app's "
            "serial port path"
        )
        return problems

    for path, number in occurrences:
        if path != expected:
            problems.append(
                f"{relative(path)}:{number}: `Port::open` outside src/device.rs; all "
                "serial port opens must be centralised in src/device.rs"
            )

    commands = APP / "src" / "commands.rs"
    if commands.is_file():
        text = commands.read_text(encoding="utf-8")
        if "device::probe(" not in text:
            problems.append(
                f"{relative(commands)}: does not call `device::probe(`; the read-only "
                "port path has no command behind it"
            )
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
        + check_da_digest_gate()
        + check_gui_write_route()
        + check_app_dependency()
        + check_app_write_route()
        + check_app_destructive_code()
        + check_app_port_path()
    )
    print(
        f"checked {len(files)} m241-core source/test file(s) for destructive commands, "
        "write identifiers and the dependency direction, and the flash write path for "
        "its layout gate and its DA digest token"
    )
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} safety violation(s)")
        return 1
    print(
        "no write path in m241-core; m241-gui and m241-app write through m241-flash only; "
        "the dependency runs one way; "
        "the strict image path accepts only the backup diff; the vendor gate is a separate "
        "fixed-SHA function; every public writer runs check_layout() first; the DA digest "
        "token has no production constructor but the pinned one; GUI and m241-app carry no "
        "destructive opcode, centralise serial opens in device.rs, and guard writes with "
        "WRITE / PROBE"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
