# Prior art: what was taken from where

Two implementations of this repair exist, and this one is the third. Neither of
the first two is a source of truth; both are inputs that were checked against the
vendor binaries where it mattered.

| Project | Language | Licence | What it established | What this project does differently |
|---|---|---|---|---|
| [`NekobaTechnology/holux-m241-wnro-fix`](https://github.com/NekobaTechnology/holux-m241-wnro-fix) | Python | MIT | The `0x402F4` epoch patch and its value, the DA load address, the `$PMTK180*3B` entry route, the failure of the "patch the +1024 sites" approach, and a dump-twice-and-compare gate | Rebuilds the flash path from the DA disassembly instead of its framing; probes NMEA before touching the reset lines; identifies an image by profile instead of by one fixed length |
| `holux-m241-wnro-toolkit` (v1, Python) | Python | MIT | A full CLI, a 470-test suite, the measured protocol constants (echo lags, the two-byte `0xD3` reply, no read trailer), the log format quirks | Rust; the flash path rebuilt from the binary; the log parser and exporters re-derived from the format description, not ported |
| this project | Rust | MIT | — | — |

## What is a hypothesis and what is verified

Four levels, used in every table on this page:

- **verified** — measured on a retail M-241 and recorded in
  `docs/dev/05-verification.md`. This page moves no claim to that level.
- **measured by v1** — reported from v1's own hardware session and documented
  there. Evidence, but not reproduced by this project, so still a hypothesis
  here.
- **read** — verified about a repository's *text*: the code does what the row
  says. It says nothing about the device.
- **hypothesis** — inferred. Marked as such wherever it appears.

The distinction matters most for the flash path. The two prior implementations
disagree in places, and both were written against the same incomplete picture of
the DA command set. That is why the command semantics come from disassembling
`DA_v3.8.4.bin` and are written into `docs/reference/01-da-protocol.md` before any
code implements them.

## How the prior art was read

- Nekoba, read in full at commit `7da7d65` (5 commits, no tags): both READMEs, the
  disclaimer, the four analysis pages and all five scripts. It has no test suite;
  its only dependency is `pyserial`.
- v1, read at `6651f8f` ("release: 1.0.1"): the protocol, firmware, logfile, CLI
  and docs trees. Its suite was run once for this page: **468 passed, 2 skipped**,
  which matches the 470 the v1 README claims. Both skips are the two tests that
  want the vendor firmware drop in a location the run did not provide; the patch
  results below were therefore replayed by hand instead.
- The `0x402F4` and version-marker rewrites were replayed offline against both
  official images in the local `holux/` drop, and the results compared byte for
  byte with v1's recorded patched hashes. Nothing was run against a device.

## Prior-art claims, checked against the code and against each other

| Claim | Nekoba | This project | Verdict |
|---|---|---|---|
| `0xD3` replies with two bytes: ACK plus a status byte | reads two bytes, checks only `r[0] == 0x5A` and ignores the status (`scripts/mtk_proto.py`, `_set_mem_block`) | documented as measured in `docs/dev/03-hardware-facts.md`; the disassembly shows the reply is two bytes only on acceptance (section 4.1 of `docs/reference/01-da-protocol.md`) | both prior implementations read two bytes; the measurement is v1's, and it is a success-path measurement. The second byte is always `0x00` — it is not a result code |
| A non-zero `0xD3` status means failure | never inspects it: reads two bytes and only checks the ACK (`scripts/mtk_proto.py`, `_set_mem_block`) | **v1 implements the claim** — the second byte is read and anything other than `0x00` (or a second `0x5A`) is raised as an error (`src/m241/protocol/da.py`, `_begin_write`) | **refuted by the disassembly**: the status cell is written only with zero, and a refusal sends no second byte at all — it is a single `0xA5`. v1's check can never fire on a successful command; after a refusal it reads the first byte of the next reply as the status |
| WRITE16's echo trails by one byte | hit the same phenomenon: the last chunk's echo is read with a short timeout and a mismatch is printed as a warning and ignored (`scripts/mtk_proto.py`, `write16`) | documented in `AGENTS.md` §4.2; no write path exists here yet to implement it in | two independent implementations meeting the same asymmetry is strong corroboration; the exact N−1 rule is **measured by v1** |
| Reads carry no checksum trailer; `0xA4` is separate | every read is followed by a separate `0xA4` round trip (`read16`, `read32`); no trailer is ever consumed | documented in `AGENTS.md` §4.3; `0xA4` is a distinct opcode with a word count (`OPCODE_CHECKSUM` and `Frame::for_bytes` in `crates/m241-core/src/protocol/brom.rs`), though no method issues it yet | **measured by v1**, corroborated by a second implementation that would have desynchronised if a trailer existed |
| `0xD3` is one ten-byte frame, not split header/begin/end | one `struct.pack(">BBLL", 0xD3, 1, start, end)` in a single write (`_set_mem_block`) | documented in `docs/reference/01-da-protocol.md` | **measured by v1**, corroborated by Nekoba |
| The DA accepts a short final packet | the write loop would send one, but its expected image length (458,752 = `0x70000`) is a whole multiple of `0x100`, so it never does | not recorded here; the 92-byte tail of a 406,620-byte image is a v1 measurement (v1's `AGENTS.md` §5 during its one hardware flash) | **measured by v1** with a real short tail; Nekoba neither confirms nor contradicts it, and this project has not flashed |
| `0xD9` restarts the chip and the firmware boots | writes a single `0xD9` and expects no reply (`restart`), then asks the user to power-cycle | documented in `docs/reference/01-da-protocol.md` | **measured by v1**; the argument form stayed open on both sides |
| Load address `0x00100800` | `BASE_ADDR_DA`, with the comment that `0x100000`–`0x1007FF` is BootROM stack and that this is the vendor utility's `DA_SetStartAddr` value (`scripts/mtk_proto.py:27`) | `BASE_ADDR_DA` in `crates/m241-core/src/protocol/brom.rs` | **measured by v1**; corroborated by Nekoba and by `push 0x100800` in the vendor utility — three agreeing sources |
| DA info report: `>BHBLHHHHL`, 20 bytes, first byte `0xC0` | same struct and length (`scripts/mtk_proto.py`, `struct.unpack(">BHBLHHHHL", info)`), and its recorded report is `c0 03 08 49 00 08 00 00 00 c2 22 ba …` (Nekoba's 03-brom-analysis.md) | measured report `C0 03 08 49 …`, same field layout (`docs/dev/03-hardware-facts.md`) | **independent agreement on observed bytes** — the strongest corroboration on this page: 512 KiB part, id `0x49`, mfr `0x00C2`, device `0x22BA`, DA `0x0308` |
| The DA version must be `0x0308` | defines `EXPECTED_DA_VERSION = 0x0308` and refuses any other version and any flash id of `0xFF` (`scripts/mtk_proto.py:359`, checked in `DownloadAgent.start`) | recorded in `docs/dev/03-hardware-facts.md` as the report's second field, and modelled by the simulator; no code enforces it yet | both prior implementations enforce it and both derived it from DA v3.8.4; whether that is a hardware limit or a vendor-tool assumption stays unsettled |
| BootROM firmware base is `0x00200000` | the constant is `0x20000000` and is the dumper's default; the `--base-addr` help says "use 0x00000000 for MT3318"; its own analysis page puts the main firmware at `0x200000` (Nekoba's 02-firmware-map.md, "Main Firmware") | `0x0020_0000` verified: `0x20000000` reads back `42 20 02 02` (unmapped bus); the DA's linear view `0x0` and the MT333x `0x2000_0000` are kept as separate, labelled constants | three different values appear inside Nekoba; **verified** by this project for `0x00200000` only |
| The high-baud argument form | `0xD2, <index>, 0x00`, three bytes, one ACK (`set_baud_rate`) | no implementation; the argument form is an open question in `docs/reference/01-da-protocol.md` | the two prior implementations **disagree** (v1 sends `0xD2` plus a `u32` baud, five bytes, one ACK) — resolved only by the disassembly |
| Long image lengths are a dump artefact | the patcher requires exactly 458,752 bytes | images are the measured release lengths, 406,312 and 406,620 | different image conventions, not a patch disagreement; see difference 3 |

## The five differences that matter, ordered by consequence

### 1. How the BootROM is entered

**Nekoba.** `MtkBootRom.start()` pulses RTS and DTR (low, high, then a 0.3 s wait)
unconditionally, with no check for a running firmware
(`scripts/mtk_proto.py:74-91`). Its read-only dumper takes that default
(`scripts/dump_firmware.py:54`); its flashing script passes `use_rts_dtr=False`
and uses `$PMTK180*3B` instead (`scripts/flash_firmware.py:127`), so the risky
route is the one used for the *safe* operation.

**This project.** The entry routine probes the NMEA stream first and returns a
tri-state — streaming, silent, cannot tell — and asserting the lines is allowed
only on "definitively silent" (`AGENTS.md` §2.1, `StreamState` and `probe_stream`
in `crates/m241-core/src/protocol/pmtk.rs`, and the warning on
`set_control_lines` in `crates/m241-core/src/protocol/port.rs:44`).
For a live stream it writes `$PMTK180*3B` (like Nekoba's flashing path), using
the 350/200/500 ms windows and sequence from the v1 Python client's
`_send_pmtk180` (`m241/protocol/brom.py`, measured by v1 — clear, drain, clear,
write, flush, quiet, listen; `AGENTS.md` §4.6 and
`PmtkClient::enter_bootrom_after`). The Rust client differs from the Python one
in three details, listed on `EntryTiming`: it does not switch the baud first
(the port is already at the NMEA baud), the pre-write drain is not a reason to
refuse, and a failure leaves the baud alone. ⚠️ The Rust sequence is 未検証: the
measurement is v1's, and it has not been re-run from `m241` on hardware.

**Verdict: deliberate divergence.** Both projects know the vendor utility asserts
the lines — Nekoba's own notes trace `EscapeCommFunction(SETRTS/SETDTR)` through
the vendor DLL (Nekoba's 03-brom-analysis.md) — but the consequence at a *running*
firmware (the unit stops answering NMEA and needs the battery pulled) is a
warning this project carries and Nekoba does not. Its `DISCLAIMER.md` covers brick
risk in general, not this specific way to reach a silent-but-recoverable unit.
Not a defect: a different trade of reliability for safety.

### 2. What is checked before a destructive write, and after it

**Nekoba.** The three files must exist (`scripts/flash_firmware.py:91-94`), the
user types `WRITE` or passes `--yes` (`:111-119`), and the image must not be
larger than the DA-reported flash size (`:148-153`). The backup is only ever
*printed*: its size and SHA-256 are computed at `:97-105` and never compared with
`--fw`. After the write the script calls `da.restart()`, prints NEXT STEPS and
exits (`:176-185`); nothing is read back. `verify` appears in that script only in
the docstring, about dumping twice.

**This project.** The specification has three refusals — the image matches a
known profile **or** every byte that differs from `--backup` falls inside the
patch offsets; the image fits the flash size the DA reports; the user typed
`WRITE` (`docs/reference/02-cli.md` §`m241 fw flash`, `AGENTS.md` §2.3,
`CONTRIBUTING.md` safety-gate table) — plus a read-back after the write. **It is
not implemented.** `crates/m241-flash/src/da.rs` refuses to operate and
`crates/m241-cli/src/lib.rs` returns "not implemented" for `fw flash`; the
offline gates that *are* code are the profile and epoch checks in
`crates/m241-core/src/firmware/patch.rs`.

**Verdict: the largest difference, and the one where this project is currently
the weakest of the three.** Nekoba and v1 both have a working write path with a
typed confirmation; this project has a specification, a refusal and no write path.
The backup diff check and the read-back are the two gates that do not exist
upstream in any form, and they stay unproven until the flash path lands.

### 3. Which firmware images the patch tool will accept

**Nekoba.** Exactly 458,752 bytes (`0x70000`), and a SHA-256 that must begin
`b83204e0cb4a1daa` unless `--no-verify` is passed (`scripts/patch_firmware.py:33`,
gates at `:68` and `:78`; the length has no override). A patch whose "old" byte
does not match is skipped with a warning, the output file is written anyway and
the exit status is 0 (`:105-119`).

**This project.** Two measured profiles — 406,312 bytes (V1.12) and 406,620
(V1.13) — matched by exact hash **or** by structure: size, family string, device
type and version marker (`identify` in `crates/m241-core/src/firmware/profiles.rs`),
with the header's declared length available as a completeness cross-check
(`declared_size` in `crates/m241-core/src/firmware/patch.rs`; note the field holds
the file size **minus four**, so it is not equal to the image length).
An unexpected byte is a refusal, never a skip.

**Verdict: the two tools are written for different image conventions.** Verified
offline: the official V1.13 package is 406,620 bytes and its header at `0xF4`
declares 406,616; 458,752 is neither. Padding the official image with `0x00` or
`0xFF` to `0x70000` or `0x80000` does not reproduce the `b832…` prefix, so the
expected image is not the official package in a padded wrapper. Also verified:
Nekoba's own dumper derives its length from the header when `--length` is absent
(`scripts/dump_firmware.py:57-66`), so its own default dump (406,616 bytes) is
refused by its own patcher. **Hypothesis:** the 458,752-byte image is a dump taken
with an explicit `--length 0x70000`, which the dumper's options allow; the
repository does not record which read produced the hash. Consequence for a
reader: copying Nekoba's length constant would reject the official update package
and every default dump.

### 4. Which byte the version patch writes

**Nekoba.** One byte at file offset `0x59C2C`, `'3'` → `'4'`
(`scripts/patch_firmware.py:44-50`). That is the last digit of `V1.13` at
`0x59C28`, so the patch is V1.13-only. Verified by replaying the script on the
V1.12 image padded to 458,752: that offset holds `0x00`, the patch is skipped with
a warning, only the epoch constant changes, and the script still exits 0.

**This project.** The marker offset comes from the profile — `0x59C28` on V1.13,
`0x59B14` on V1.12 — and only the final digit is rewritten, so both releases
report V1.14 (`PROFILE_V1_12`, `PROFILE_V1_13` and `patched_marker` in
`crates/m241-core/src/firmware/profiles.rs`; `version_digit_offset` and
`version_edit` in `crates/m241-core/src/firmware/patch.rs`). v1 reaches the same
place by finding the `V1\.xx` run in the image (`src/m241/firmware/patcher.py`,
`_version_run`).

**Verdict: same fix, different coverage.** On V1.13 the two schemes produce
byte-identical output — verified: three bytes change (`0x402F6`, `0x402F7`,
`0x59C2C`), patched SHA-256
`901555843895054f77716364a446e8ad41c00b799c781b4ff95144352c141c9a`, which matches
v1's recorded value. So this is a coverage difference on a release Nekoba does not
target, not a wrong byte on the release it does.

### 5. Where the flash is read from

**Nekoba.** `BASE_ADDR_FIRMWARE = 0x20000000` (`scripts/mtk_proto.py:24`) is the
dumper's default through `--base-addr` (`scripts/dump_firmware.py:101-107`), whose
help text suggests `0x00000000` for the MT3318, while the same repository's
analysis page puts the main firmware at `0x200000`
(Nekoba's 02-firmware-map.md, "Main Firmware").

**This project.** `0x0020_0000` verified: reading `0x20000000` returns
`42 20 02 02` (unmapped bus) and `0x00200000` returns the vector table
(`docs/dev/03-hardware-facts.md`). The DA's linear view (`0x0`) and the MT333x
`0x2000_0000` are separate constants with separate provenance notes in
`crates/m241-core/src/protocol/brom.rs`.

**Verdict: three addresses inside one repository, none of them the one this
project measured.** Which of them produced the dump behind Nekoba's own analysis
page is not recorded, and the epoch patch offsets are file offsets, so this
affects the read path only. Not a patch-correctness problem — a
reproducibility problem: the command that produced the recorded dump cannot be
reconstructed from the repository.

## What is dangerous in the prior art, and why this project does not copy it

1. **Pulsing the reset lines without asking the firmware first.** See difference
   1. The failure mode is a unit that answers nothing until the battery is
   removed, which is recoverable but indistinguishable from a brick at the time.
2. **An erase range taken from user input.** `--start-addr` defaults to `0x0` and
   goes straight into the `0xD3` range, while the help text for the same flag says
   the main firmware lives at `0x200000`. `0xD3` only records that range, but the
   `0xD5` write that follows erases every sector it covers — so a wrong value
   erases the reset vector. Nekoba's quick-start even passes it explicitly; v1 has
   the same flag. This project's rule is the opposite: the range is a hard-coded
   constant checked against an expected value, never derived from an argument
   (`AGENTS.md` §2.2).
3. **No read-back after the write.** Nekoba stops at the restart; v1 documents the
   same choice (v1's CLI reference page says: "There is no read-back
   or compare after the write"). A write that every packet accepted can still be
   a wrong image, and the unit reports nothing until the next boot.
4. **Warnings that do not stop the write.** A skipped patch still produces an
   output file and exit status 0; a checksum mismatch inside `write16` is printed
   and the write continues. The pattern is reasonable for a script its author
   watched run, and wrong for a tool whose user will not read stderr.

## Designs this project deliberately does not adopt

- **The default reset-line entry.** Replaced by the NMEA tri-state probe and a
  refusal on "cannot tell".
- **A single fixed image length plus a stock hash prefix as the identity test**
  (458,752 bytes, `b83204e0…`). Replaced by measured profiles, a structural
  fallback and the header's declared length — a real backup carries per-unit
  fields and will not match a published hash.
- **The padded image that length implies.** This project writes an image at the
  length it was read, not at a round `0x70000`.
- **A hardcoded version-marker offset.** Replaced by a per-profile offset, so
  V1.12 and V1.13 both get their marker bumped.
- **Skipping a patch whose old byte does not match, then exiting 0.** Replaced by
  a refusal: an unexpected byte means the image is not what the patch was
  measured on.
- **`--start-addr` as user input** (both prior projects). Replaced by a constant
  range with an expected-value check.
- **Not verifying after the write** (both prior projects). Replaced — in the
  specification, not yet in code — by re-entering the BootROM and re-reading.
- **Porting the DA framing.** Neither prior implementation is a source for the
  command semantics; they disagree, and the disassembly decides
  (`CONTRIBUTING.md`, "The one rule that is not negotiable").

## What was taken

Facts, not code:

- The epoch constant at `0x402F4` and the value `935_280_000` (v1 credits the
  origin of the discovery to the catea.tech write-up, and its README names both
  that and Nekoba).
- That only `0x402F4` is effective and `0x402DC` is an unused copy of the same
  constant.
- The DA load address `0x00100800`.
- The `$PMTK180*3B` route into the BootROM for a firmware that is still running.
- The negative result that patching the `movs r1, #1; lsls r1, r1, #10` sites does
  not change the reported date.
- The dump-twice-and-compare gate. Nekoba's dumper reads twice, asks for a
  power-cycle between the passes, compares SHA-256 and keeps the second file when
  they differ (`scripts/dump_firmware.py:136-174`); this project keeps that gate.
- One idea from Nekoba worth adopting here: after uploading the DA it reads the
  first four bytes back and refuses to jump if the read fails
  (`scripts/mtk_proto.py`, `DownloadAgent.start`). A dead DA is otherwise only
  visible as a timeout after the jump. v1 does not do this.

## Licences, and why no code was copied

Both prior projects are MIT:

- Nekoba: `MIT`, "Copyright (c) 2026 holux-m241-wnro-fix contributors", with a
  closing paragraph that the grant covers the source, docs and scripts and that
  the proprietary firmware and download agents stay with Holux, MediaTek and
  GlobalTop.
- v1: `MIT`, "Copyright (c) 2026 holux-m241-wnro-toolkit contributors".

Copying would therefore be lawful. It is still not done, for three reasons:

1. **The flash path may not be ported.** `CONTRIBUTING.md` requires a citation
   into `docs/reference/01-da-protocol.md` for every DA constant, and a citation
   from a prior implementation is not accepted. This rule predates this page and
   is what keeps a wrong assumption out of a path that can destroy a unit.
2. **The rest is re-derived, not translated.** Rust has no `pyserial`, the log
   format is re-read from its description rather than from a Python parser, and
   the offline paths must stay clear of a device.
3. **Attribution here is by fact.** What this project owes the two of them is the
   list above, which is why each item is named rather than merged into a licence
   header.

No firmware image, download agent, vendor DLL or vendor executable is
redistributed here; Nekoba's repository says the same and blocks the same
filenames in `.gitignore`.

## What stays open

The disagreements above are resolved by the disassembly, not by argument, and
`docs/reference/01-da-protocol.md` now carries the answers: the non-zero `0xD3`
status does not exist, `0xD3`'s range is in bytes (with an even-start / odd-end
constraint), `0xD2` takes a baud index plus an echo count and needs a further
`0xC0` / `0x5A` handshake, `0xD9` takes no argument, and the DA verifies the
packet sum itself. What remains unmeasured there is the hardware behaviour of the
read path and the two final `0x5A` bytes; that page's §7 lists it.
