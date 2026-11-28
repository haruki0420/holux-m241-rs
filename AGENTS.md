# Working notes for automated contributors

Read this before touching `crates/m241-core/src/protocol/`. It records what was
expensive to learn so it does not have to be rediscovered.

## 0. How to work here

**One check command, run before every commit:**

```bash
./tools/check.sh
```

It runs six gates: formatting, lint, the workspace tests, the docs link check,
the design invariant check and the safety check. It prints one line per gate.

**The Tauri app needs its frontend built first.** `crates/m241-app` embeds
`ui/dist` at compile time (`tauri::generate_context!`), so a checkout without it
cannot build the crate. `check.sh` prepares it itself: when
`crates/m241-app/ui/dist/index.html` is missing it runs
`pnpm --dir crates/m241-app/ui install --frozen-lockfile` and
`pnpm --dir crates/m241-app/ui build` (pnpm only, never npm/npx). If pnpm is
missing or the build fails, the app is excluded from clippy and test, the
reason is printed (`[SKIP] m241-app: ui/dist missing: …`) and the run reports
5/6 plus that explicit skip — it does not claim 6/6. A clean clone with Node.js
and pnpm installed therefore reaches 6/6 on the first `./tools/check.sh`.

The design gate (`tools/check_design.py`) is not cosmetic. It checks three
invariants mechanically: amber appears only in the destructive module, a corner
radius is always `theme::RADIUS`, and a status colour always sits beside a text
label. It fails the build when one is broken. Those three rules each encode a
decision from `DESIGN.md`, and a rule enforced only by review holds until the
reviewer is busy.

The safety gate (`tools/check_safety.py`) is the other mechanical one, and it is
what keeps reading and writing apart. `m241-core` reads; `m241-flash` writes.
The gate fails if a destructive command (`$PMTK182,6`, `0xD3`, `0xD4`, `0xD5`,
`0xD9`, including byte-literal / `\xNN` escape forms) or a write-shaped
identifier appears in `m241-core` (`src/` or `tests/`), if `m241-core`'s
dependency tree contains `m241-flash`, or if `set_control_lines` is named
outside the exact BootROM entry/port/simulator paths. A binary built from
`m241-core` alone therefore cannot link or call the write/erase routines in
`m241-flash` — though raw transport and framing primitives (`Port::open`,
`Transport::write_all`, `pmtk::line`, `Frame`) remain public in `m241-core`
because `m241-flash` builds on them.

**The GUI writes too (user decision, 2026-11-23).** `m241-gui` now depends on
`m241-flash` on purpose: the Firmware screen is the primary operator surface,
and the user asked for the CLI's write capability there. The gate therefore
requires the GUI's dependency tree to contain `m241-flash`, requires its write
actions to call `m241_flash::flow::flash`, requires the Firmware screen to build
the `WRITE` confirmation and the Connect screen the `PROBE` confirmation from
their own typed words, and refuses `Confirmation::granted` / `Job::Flash` in any
other GUI file. It also fails if a destructive opcode (`0xD3` / `0xD4` / `0xD5`
/ `0xD9`, `PMTK182,6`) appears in the GUI's *code* — bare hex literal, `\xNN`
escape or the PMTK erase string — while the screens' operator prose in strings
and comments stays allowed. The reverse edge stays forbidden: `m241-core` still
must not reach the write crate, so a read-only binary cannot erase or program.

**That new gate is weaker than the one it replaced, and this is deliberate.**
The old rule was the dependency graph (`m241-gui` did not link `m241-flash`), so
no edit inside the GUI could bypass it. The replacement is a text check; an edit
that also weakens `tools/check_safety.py` would pass it. What still holds
structurally is everything inside `m241-flash`: the fixed range,
`check_layout`, `check_image`, `verify_device` and the full read-back. Do not
describe the GUI's gates as equivalent to the removed edge. See
[`docs/dev/11-safety-review.md`](docs/dev/11-safety-review.md).

**Map tiles are the only network use, from two places.** The egui GUI's Log
screen fetches OpenStreetMap raster tiles through the `TileFetcher` seam in
`crates/m241-gui/src/screens/log/tiles.rs`; `--screenshot` and a headless render
never fetch there (the window gate is `native_pixels_per_point`, and the sample
frame says it shows no tiles). The Tauri app (`crates/m241-app`) fetches the
same tiles from the webview with MapLibre; that is why its CSP in
`tauri.conf.json` names `tile.openstreetmap.org`, and its persisted tile switch
is what turns the requests off. There is no seam on the Tauri side because the
WebView owns the request. Every other command stays offline; `nmea proxy`'s TCP
listener is loopback by default. Any further network use needs its own note here
and an update to `DESIGN.md`.

**Long commands go in the background.** A first `cargo build` of the GUI pulls
in `wgpu` and takes several minutes. Do not block a turn on it.

**Never manage files with a script.** Moving or renaming: `git mv`. Editing:
the editor tools, one change at a time. A bulk find-and-replace once corrupted
Python identifiers in the predecessor project (`True` became `hrue`) and had to
be reverted from git.

**Do not claim what has not been verified.** If something has not been run on a
real M-241, the docs say 未検証 and so does your report. The hardware session in
`docs/dev/05-verification.md` is the only file allowed to move a claim from
unverified to verified.

**A subagent request is a tool call.** Writing the request into the message is
not a request: the child never starts. Read the session id from the tool result
and say "running" only when that id exists.

**Before asking the user to unplug the battery or the cable, look for a software
way back.** A unit left in the DA's RAM is sent the one-byte `0xD9` restart with
`fw da-restart` — it has no reply, so the restart itself cannot be confirmed; run
`m241 fw dump` to see what the unit answers. A unit that has booted its firmware
is reached with the `$PMTK180` route; a unit that is already sitting in the
BootROM is reached with `--assume-brom`. A physical power cycle is the last
resort.

**The simulator is a regression pin, not an oracle.** The hardware decides
unknown behaviour; the fake records what the hardware has already shown. A green
fake is not evidence that the write path works on a unit.

**Read §9 before merging or accepting a fix.** It records what has already gone
wrong here, including the two times 5/5 and a readable diff were mistaken for
verification.

## 1. What this project is

A cross-platform (Windows / Linux / macOS) tool for the **Holux M-241** GPS
logger (MediaTek **MT3318**, firmware `B-core_1.20`, model `01017-00F`).

Three jobs:

1. **Repair the GPS week-number rollover.** The 2006-era firmware hard-codes the
   GPS-epoch Unix offset `315964800` and never accounts for the second rollover
   (2019-04-06), so every timestamp is 7168 days in the past. The 4-byte constant
   at file offset `0x402F4` is replaced (`315964800` → `935280000`); **3 bytes
   actually change**.
2. **Download and convert the track log**, including matching photos by capture
   time.
3. **Repair or relay a live NMEA stream** for hosts that cannot be patched.

The only runtime dependency is `pyserial`… no: in v2 the dependencies are the
Rust standard library, `serialport` (the only code that touches a terminal) and
`image` with only its `jpeg` feature (KMZ long-edge reduction, spec §6.3); the
offline commands never open a port.

## 2. Non-negotiable safety rules

### 2.1 Never pulse DTR/RTS at a logger that is streaming NMEA

`MtkBootRom::sync()` on its own is safe. Asserting the reset lines is not: at a
running firmware it leaves the unit unresponsive until the battery is removed.

The entry routine therefore checks for NMEA first and returns a tri-state:

* `true` — firmware is running, use `$PMTK180*3B`;
* `false` — definitively silent, the reset route is safe;
* `None` — cannot tell, **refuse**.

Do not "simplify" that tri-state away.

### 2.2 `0xD5` erases every sector the recorded range touches

`0xD3` only records a range: the handler runs `0x00101F98`–`0x00102034` and calls
no flash driver, so on its own it writes nothing and erases nothing. The erase
is inside `0xD5`, per sector, immediately before that sector is programmed
(`0x0010251C`–`0x0010252C` calls the driver's sector erase, `FUN_00100D5C`).

The danger is unchanged: a range that starts at `0` and is then written with
`0xD5` erases the start of the firmware image, including the reset vector, and
the unit stops booting.

`0xD4` is a second, separate erase command: it erases a range outright and
interleaves `0x04 <percent>` progress frames before its final byte
(`docs/reference/01-da-protocol.md` §5.4). It is destructive on its own and is
not part of the write sequence.

Rules:

* the range must be a hard-coded constant, not a value derived from user input;
* refuse to run if that constant is anything unexpected;
* verify a known-intact marker before erasing;
* require an explicit confirmation: type `WRITE` at the CLI prompt, type
  `WRITE` in the GUI's confirmation field, or pass `--yes` to the CLI;
* the prompt is for the user. An agent must not answer it — not even by driving
  a PTY or clicking the GUI field — because typing `WRITE` is the user's own
  acknowledgement, not an approval an agent can supply. The GUI has no `--yes`
  equivalent: its buttons stay disabled until the user types the word, the
  action re-checks it, and `tools/check_safety.py` refuses `Job::Flash` /
  `Confirmation::granted` outside the screens that own the typed word. Those are
  source checks, not a type: the token proves a call-site acknowledgement, not
  that a person typed the word (`da.rs`'s `Confirmation` says so itself). The
  only route an agent may take is `--yes` in the CLI, and only once the
  approval §2.5 requires has been obtained (by default the user; when
  delegated, the independent reviewer);
* an agent may pass `--yes` to `fw flash` **or `fw probe-da`** only when the
  approval §2.5 requires has been obtained. `fw probe-da` loads the DA into RAM
  and jumps to it, so the loaded DA can erase or program flash itself; it is
  inside §2.5, not outside it.

### 2.3 Take a dump before writing anything

Any image whose differing bytes fall outside the patch offsets of the user's own
backup is refused on the strict path. An image identical to the backup is
accepted. Do not weaken that check (`check_image` supports both V1.13 `406,620`
bytes and V1.12 `406,312` bytes on the strict path).

**Official vendor image gate (explicit user decision, 2026-11-22; extended
2026-11-28).** The user has explicitly decided that writing the *unmodified*
official V1.13 or V1.12 vendor image is allowed, accepting that its 14 bytes of
per-unit data overwrite this unit's values. This is a separate gate, never a
digest branch inside the backup check:

* only the pinned V1.13 vendor image (`32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435`,
  406,620 bytes) or V1.12 vendor image (`4103601a29f256ae5da771bbcbc3ca85a89deb6268ce74865dc84b2583ae98f7`,
  406,312 bytes; hardware write ⚠️ 未検証); any other build is refused;
* `--allow-vendor` and `--yes` are both required, and `--backup` and `--da` stay
  required (`--backup` must be a valid M-241 dump, allowing cross-profile
  official writes such as V1.12 onto a V1.13 unit);
* the unit must still match that backup before the DA is loaded
  (`verify_device`), and the written range is read back and compared with the
  vendor image under `ReadBackPolicy::AllowPerUnitRestore` (decision A,
  2026-11-28: firmware-restored bytes strictly inside `0x5C..0x60` are counted
  in `restored_per_unit_bytes`, while any difference outside `0x5C..0x60` fails);
* the command prints which per-unit data is overwritten before writing and on
  success;
* the vendor V1.13 / V1.12 images are the **stock, unpatched** releases: epoch
  `315964800`, marker `V1.13` / `V1.12`. Writing them leaves the date 7168 days
  behind until patched, so the dry run, the pre-write warning and the success
  output all say so;
* `--allow-vendor --yes` is inside §2.5. The strict `check_image` must not gain a
  digest branch: `accept_vendor_image` is the only way to obtain the
  `VendorImage` token `write_vendor` accepts.

**Unverified / flexible image gate (explicit user decision, 2026-11-28).** The
user has explicitly decided that `--allow-unverified` (`accept_unverified_image`
/ `write_unverified`) may write any physically valid image (`check_physical_image`:
non-empty, even byte length, `<= FLASH_CAPACITY_BYTES` = 512 KiB) from
`FLASH_WRITE_START = 0`, subject to `--yes`, pre-write warnings (SHA-256 and ARM
reset vector check), `verify_device` against `--backup`, and read-back under
`ReadBackPolicy::AllowPerUnitRestore` (`0x5C..0x60` restore tolerated; any other
difference fails). Physical constraints (`check_physical_image`) remain hard
refusals that no flag can bypass.

### 2.4 Never commit personal data

See §6.

### 2.5 Who may approve a write-capable run (`fw flash --yes`, `fw flash --recover --yes`, `fw flash --allow-vendor --yes`, `fw probe-da --yes`, and the GUI's write / recovery / vendor / probe-da actions)

By default, passing `--yes` to `fw flash` (**including `--recover` and
`--allow-vendor`**) or `fw probe-da` requires the
user's own approval. The GUI's equivalents are the same approvals: its
「書き込む」 / 「復旧」 / vendor write / 「DA を確認」 actions run only after the
user types `WRITE` or `PROBE`, and no `--yes`-style bypass exists in the GUI. Subagent approval under this section applies **only** when
the user has explicitly delegated approval for these write-capable runs in that
session or instruction; any explicit user requirement for prior human approval
on destructive changes **always** takes precedence and overrides this section.

`fw probe-da` is covered even though this client never sends `0xD3` / `0xD4` /
`0xD5` / `0xD9`: it loads the DA into RAM and jumps to it with `0xA8`, and the
loaded DA can erase or program flash itself. The DA gate requires the `--da` file
to be exactly 14,896 bytes, to start `00 00 A0 E1`, **and** to hash to the pinned
vendor full SHA-256
`190d7064cf2320752d8eb65681142730b21cddc90e0d1491d4e8379c3f2e89aa`
(the V1.12 and V1.13 vendor packages ship the same 14,896-byte DA). The expected
digest is a `DaDigest` token: a normal build can construct one only with
`DaDigest::vendor()`, so neither command can be told to accept another DA. The
CLI calls `check_da_image_against` / `DaLoader::load_against` with `da_digest()`,
which returns that vendor token; `check_da_image` / `DaLoader::load` are the same
pinned check. A file with the right length and head but any other digest is
refused before the port is opened, so the approval's digest requirement is
enforced by the code rather than only checked by a person. Any other value is
still not approvable.

When delegated, the agent preparing the run asks an **independent**
reviewer session — not the preparing session and **not forked** from the
preparing session's history — whether the exact run may proceed, and passes
`--yes` only after that reviewer records `"approved"`. The preparer must pass
raw artifacts and command paths only, **never** its own summary, narrative, or
conclusions (`docs/dev/09-review-protocol.md`). If a reviewer refuses, the
preparer must fix the underlying cause and restart the request from fresh
artifacts; shopping for a different approver without fixing the cause and
re-gathering fresh artifacts is forbidden.

For `fw probe-da --yes`, the request supplies the current-session `device info`
log, the resolved `--port`, the `--da` path and its full SHA-256, and the exact
command line. The image, backup, write-range and read-back items below apply
only to `fw flash`, for which the request must also supply raw artifacts that
the reviewer independently verifies:

* current-session raw command logs for `device info`, `fw dump`, and
  `fw probe-da`, confirming the unit's identity (`B-core_1.20`, model
  `01017-00F`, flash `00C2:22BA`) and the exact resolved serial port (never
  `--port auto`);
* the backup path and its SHA-256 recomputed independently by the reviewer from
  the current session's `fw dump` artifact;
* the write image path, its independently recomputed SHA-256, and its
  provenance per §2.3 and `da.rs::check_image`: it must be either identical to
  the user's own backup from this session or differ from that backup **only**
  at the recorded patch offsets for the unit's supported profile (currently
  V1.13 `01017-00F`, length `0x6345C`, `PATCH_OFFSETS` `[0x402F6, 0x402F7, 0x59C2C]`),
  verified independently to have zero differing bytes outside those offsets
  (never an unrelated image or unverified vendor package);
* the `--da` binary (`DA_v3.8.4.bin`, 14,896 bytes) path and its independently
  recomputed SHA-256, which must equal the full value recorded above (a
  mismatch is a refusal);
* the write range: `FLASH_WRITE_START` and `FLASH_WRITE_LENGTH` (`0x6345C`),
  covering the firmware and nothing else, with `FLASH_WRITE_START` **already
  settled** by a hardware run recorded in `docs/dev/05-verification.md` §1 and
  `UNRESOLVED_STARTS` emptied in `crates/m241-flash/src/da.rs` (if
  `UNRESOLVED_STARTS` is non-empty or §1 lacks that settlement row, approval
  must be refused; §1 defines what the row must contain);
* the read-back plan (whole-image BootROM compare) and the recovery step
  matching `crates/m241-flash/src/da.rs` (`0xD9` restart without waiting for a
  reply, or power-cycle, then re-enter BootROM with `--assume-brom` or retry the
  four-probe sync and write the user's own backup image back; do not download
  the log from a partly written unit);
* for `fw flash --recover --yes`, the image must be **byte-identical** to the
  backup (zero differing bytes); a patched image or any other image is a
  refusal. `--recover` skips `verify_device` because a partial unit cannot match
  the backup, so approval must confirm that the unit is the same one the backup
  came from and that the run is a recovery, not a normal write. The request
  names `--recover` explicitly; without it, the normal `verify_device` path
  applies;
* for `fw flash --allow-vendor --yes`, the image's **full** SHA-256 must equal
  either `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435`
  (V1.13, 406,620 bytes) or
  `4103601a29f256ae5da771bbcbc3ca85a89deb6268ce74865dc84b2583ae98f7`
  (V1.12, 406,312 bytes; ⚠️ 未検証 on hardware). Any other digest or length is
  a refusal. The request must state that the user knows the vendor image
  overwrites the unit's 14 bytes of per-unit data, and must include the same
  backup and unit-identity artifacts as a normal write. The normal
  `verify_device` is still required; `--allow-vendor` does not skip it;
* for `fw flash --allow-unverified --yes`, the image must pass
  `check_physical_image` (non-empty, even length, `<= 0x0008_0000` bytes), with
  its SHA-256 and ARM vector status explicitly reported before writing, and
  `verify_device` against `--backup` still required;
* the exact full command line to run, including `--yes`, the resolved `--port`,
  `--da`, the image path, and the backup path, plus `--recover`,
  `--allow-vendor`, or `--allow-unverified` when used. `fw flash` implements
  these flags; re-check the command against `m241 fw flash --help` before
  approving.

**Approval is per execution.** A previous approval covers one invocation of
the command in this section (`fw flash`, including `--recover` / `--allow-vendor`
/ `--allow-unverified`, or `fw probe-da`) only. Before every invocation the
preparer asks an independent reviewer, as in the delegation paragraph above, for
the exact command, image, backup, DA, unit, port, flags and retry behaviour, and
passes `--yes` only after that reviewer records `"approved"`. A changed image,
backup, DA, unit, port, flag, range or retry policy needs a fresh approval.

**The reviewer must independently show that the write cannot leave the pinned
bounds.** Each of these is a refusal condition:

* the write start comes only from `FLASH_WRITE_START = 0` and the write length
  comes from the checked image (`image.len()`, bounded by
  `FLASH_CAPACITY_BYTES = 0x0008_0000` and even-length alignment via
  `check_physical_image`); no flag, environment variable or user input may move
  the start address or exceed 512 KiB;
* `0xD3` records exactly `0` .. `image.len() - 1` (`end = range_end(start,
  image.len())`) and `0xD5` programs only that range (sector erase granularity
  is §2.2);
* if the implementation retries automatically, the retry re-enters the BootROM
  and re-runs the same command and the same range; it may not extend, shift or
  recompute the range;
* `check_layout` and `check_image` (or `accept_vendor_image` /
  `accept_unverified_image`) run before the first `0xD3`, and the reviewer has
  seen the call path.

A missing item, a hash or byte-diff mismatch, an unsettled
`FLASH_WRITE_START`, `--port auto`, or a non-independent/forked reviewer is a
refusal. The independent reviewer does not open the port and does not edit the
preparing branch, and **authors its own durable verdict record directly** — a
PR comment, or a review record the reviewer commits on a branch or PR; a chat
or session log alone is not a record — citing the verified SHA-256s, command
line, and reviewer session ID, **before** `--yes` is passed. The preparer must
never author or edit the approval record on the reviewer's behalf, and pre-write
approvals must **never** be written into `docs/dev/05-verification.md` §1 (which
records actual post-run hardware measurements only).

`log erase` is not covered: it destroys the user's track data, which a reviewer
cannot consent to on the user's behalf. It still needs the user's own
instruction.

## 3. Verified hardware facts — do not re-derive

| Fact | Value | Established by |
|---|---|---|
| Firmware base (BootROM view) | `0x00200000` | `0x20000000` returns `42 20 02 02` (unmapped bus); `0x00200000` returns `18 f0 9f e5 …` |
| DA load address | `0x00100800` | `0x100000`–`0x1007FF` is BootROM scratch; writing there kills the chip |
| DA info report | `>BHBLHHHHL`, 20 bytes, starts `0xC0` | DA v3.8.4 answers `C0 03 08 49 …`; the four middle `H` fields are mfr `0x00C2`, device `0x22BA`, then two zeros — ids, not geometry, and the trailing `L` comes from RAM |
| Flash on the retail unit | 512 KiB, MX29LV400CB, mfr `0xC2`, device `0x22BA`, DA id `0x49` | DA info report |
| Firmware image size | 406,620 bytes (`0x6345C`) | read back and hashed |
| NMEA | 38400 8N1, `$GPGGA/$GPGSA/$GPGSV/$GPRMC/$GPVTG` at 1 Hz | |
| UI languages | English / French / German — no Japanese | release notes list TC/SC/DE/FR/EN |

The DA's `flash_size` describes the chip it can program. A unit was measured
holding 212,860 bytes of log **alongside** a 406,620-byte firmware image, so the
log area is not that chip. Unresolved; see `docs/dev/06-open-questions.md`.

## 4. The protocol gotchas that cost the most time

### 4.1 `0xD3` replies with two bytes on success, one on refusal

Success is `5A 00`. The ACK is loaded at `0x0010201C` and sent at `0x00102020`;
the second byte is read back at `0x00102024` from the status cell that
`0x00102018` had just written, with the zero loaded at `0x00102004`, and is
tail-call sent at `0x00102034`. That cell is written **nowhere else, and only
with zero**, so the status is a fixed success marker, not a result code, and no
non-zero value exists.

A refusal is a **single** `A5`: the failure path jumps to `0x0010202C` with `A5`
in `r0`, skipping both the ACK send and the status load, and the one shared tail
call at `0x00102034` sends that single byte. Both fixed-length reads are wrong in
one direction:

* always reading one byte leaves the `0x00` in the buffer, shifting every later
  frame by one — the next command times out, and it looks like the `0xD5` step
  is broken;
* always reading two bytes consumes the first byte of the next reply after a
  refusal.

Read one byte; read the second only when the first is `0x5A`.

A refused `0xD3` also **leaves the previously recorded ranges in the table**, so
`A5` does not mean "nothing is recorded".

The refusal path is read from the instruction sequence, not from a device:
⚠️ 未検証 — no `0xD3` was ever refused on hardware, so the single-byte reply has
never been observed.

### 4.2 WRITE16's echo trails by one byte

After N bytes only N−1 echo bytes are available: the echo of the final byte
never arrives because nothing follows it. Writing one byte and waiting for its
echo hangs; requiring an exact echo for every chunk fails on the last chunk.
Accumulate and compare against `sent[..len-1]`, then drain the tail.

### 4.3 Reads carry no checksum trailer

`0xA2` / `0xAF` return echo + data only. The checksum is a separate `0xA4`
command. Consuming a two-byte trailer desynchronises every read.

### 4.4 The two widths order bytes differently

| Opcode | Wire order | Conversion |
|---|---|---|
| `0xA2` / `0xA1` | 16-bit pairs swapped | `swap_pairs` |
| `0xAF` | each 32-bit word byte-reversed | `reverse_words32` |

Using the pair swap for `0xAF` yields plausible garbage, not an error.

### 4.5 Count fields are in units, not bytes

`0xAF` takes dwords, `0xA2`/`0xA1`/`0xA4` take words. `Frame::for_bytes` encodes
that, and refuses a count that is not a whole number of units.

### 4.6 `$PMTK180*3B` needs a live NMEA stream

If the stream has already stopped the sentence is ignored, so the client checks
for the stream first and reports a precise error instead of a bare sync timeout.
Send at the NMEA baud, then switch to 115200.

A live stream is necessary but not sufficient: the timing around the sentence
is part of the sequence. **`PmtkClient::enter_bootrom_after` in
`crates/m241-core/src/protocol/pmtk.rs` is the authoritative description** of
that sequence; the numbers are the `DEFAULT_ENTRY_*` constants and
`EntryTiming` beside them. In outline: clear the input, drain the line, clear
again, write `$PMTK180*3B`, flush, stay quiet, then listen. **Any** byte in
that last window means the sentence was not honoured, and the error quotes it so
"still NMEA" can be told from "answered, but did not switch".

The timing constants are the previous Python client's values, from its
`_send_pmtk180` function (a private v1 repository, not this tree); that is the
only `$PMTK180` route measured on this unit. Three details deliberately differ
and are listed on `EntryTiming`: no baud switch before the sequence, the
pre-write drain is not a verdict, and a failure does not move the baud.

Probing the stream twice is a bug, not a harmless extra check: the caller probes
once and passes its `StreamState` to `enter_bootrom_after`, because a second
probe consumes the line the hand-over depends on. ✅ measured 2026-11-22:
`fw dump` and both successful writes entered the BootROM through this path on the
unit (`docs/dev/05-verification.md` §1.5).

### 4.7 The four sync probes must be back to back

`0xA0` → `0x5F`, then `0x0A`→`0xF5`, `0x50`→`0xAF`, `0x05`→`0xFA` with no pause.
Inserting delays drops the chip out of the handshake.

## 5. Log download (PMTK182)

```
$PMTK182,2,8            -> $PMTK182,3,8,<hex bytes used>
$PMTK182,5              -> logging disabled
$PMTK182,7,<addr>,<len> -> $PMTK182,8,<addr>,<hex data>
$PMTK182,4              -> logging enabled
$PMTK182,6              -> erase the log
```

Two things are easy to get wrong:

* **Address and length are hexadecimal without a `0x` prefix.** `$PMTK182,7,64,64`
  reads from `0x64` and returns `0x64` bytes.
* **Logging must be paused for the transfer** and resumed in a `finally` block,
  or a failure leaves the device not recording.

Chunks of `0x800` bytes are accepted. A 200 KB log is about 100 round trips.

### Record size comes from the block header

Each 64 KiB block starts with a `0x200` header whose bitmask says which fields
each record carries. A V1.13 unit can therefore produce 16-byte records (UTC +
lat + lon + height) or 20-byte records (with speed). Do not assume a size from
the firmware version.

Lat/lon are 4-byte floats, height is a 3-byte `u24`, and the per-record checksum
is one byte with no `'*'` separator.

A block is not only a header and a run of records: the unit interleaves a
settings / state journal between record runs — 16-byte entries (`AA`×7, a type
byte, a little-endian `u32` value, `BB`×4) and 20-byte `HOLUXGR241LOGGER` name
separators. Those bytes are not records; `crates/m241-core/src/log/journal.rs`
has the observed shapes, and their type meanings are inference, not
firmware-confirmed.

The header mask can be stale for a block: the real record length is decided by
the checksum chain (an unbroken run of matching trailers), while the mask
supplies the field meanings. A block that holds only the journal has no points
at all and is not an erased log.

## 6. Personal data

This repository is public. Before any commit that touches docs, tests or
examples:

* **No real coordinates.** Use `35.6812360, 139.7671250` or the no-fix
  placeholder `8960.0000,N,00000.0000,E`.
* **No local paths.** No home directories from any OS.
* **No serial-port instance names from a specific machine.** `COM3` in a usage
  example is fine; an enumeration of someone's ports is not.
* **Never commit** `holux/`, `vendor/`, `backup/` or device captures. All are
  gitignored; keep it that way.

## 7. Open questions

Recorded in `docs/dev/06-open-questions.md`. Do not fabricate answers:

* Where the log actually lives, given the DA reports a 512 KiB part while a unit
  held 212 KB of log plus a 406 KB image.
* Whether a Japanese-language firmware SKU exists.
* Bluetooth SPP was never exercised.

## 8. Review and fix: three stages, three different agents

A review that finds a problem does not hand it back to the author, and the fix
is not accepted on the fixer's word. Three stages, each a different agent:

1. **Find.** A reviewer reads the code and writes the findings with
   `file:line`. They do not fix anything.
2. **Fix.** A *different* agent applies the fixes. If a finding is wrong, they
   say so and leave the code alone — silence plus a wrong change is worse than
   an argued disagreement.
3. **Verify.** A *third* agent checks the fix against the findings and the
   tests. They read the findings and the diff. They do not read the fixer's
   explanation: an explanation is how a bad fix gets talked past.

Why bother: the agent that made the mistake reads its own fix with the same
assumption it read the original. A different agent reading the same diff with
the same instructions does not.

Every stage ends with `./tools/check.sh` at 6/6.

Before a PR is merged — after Stage 3, immediately before the merge — the
independent reviewer posts the `review-gate` status on the reviewed head SHA.
**The author's session does not post it; only an independent reviewer does.**
Branch protection requires it in every tier, and the status is per-commit: a new
push needs a new review. The tier table and the command are in
[`docs/dev/09-review-protocol.md`](docs/dev/09-review-protocol.md); that document
is authoritative and this paragraph is a summary.

## 9. What went wrong here

2026-11-20, the review-and-fix round. This section records failures that
actually happened — symptom, cause, and the check that catches it next time. It
is evidence for §8, not general advice.

### 9.1 Four fix branches went in with stage three unstarted

**Symptom.** `fix/fix-core`, `fix/fix-gui`, `fix/fix-cli` and `fix/fix-docs`
went into `main` (`52f023e`) while the verification column of
`docs/dev/10-review-log.md` still read 未検証 for all four. The merge commit
itself says "Gates 6/6 after the merge".

**Cause.** A green `./tools/check.sh` was read as "the fixes are good".

**Check.** 6/6 proves the tree is not broken; it does not prove the findings are
resolved. A gate cannot see a finding that was never fixed — the test for it
does not exist, so nothing fails. No merge until the verification column is
filled: 未検証 is a stop sign, not a default.

### 9.2 The agent handing out the findings started writing the fix

**Symptom.** After the readability review, the lead began editing
`crates/m241-core/src/error.rs` and `crates/m241-cli/src/main.rs` directly
instead of assigning the findings.

**Cause.** Speed: the change looked small and the reason was already known.

**Check.** §8 admits no exception, and it applies to the agent coordinating the
round too. The user stopped this one: 修正はあなたが直すのではなく割り振って
(assign the fix, do not write it yourself). A fixer who is also the reviewer
re-reads the changed line with the assumption that produced the bug.

### 9.3 A diff was read and declared sound

**Symptom.** `fix/simulator-d3` added 302 lines to the simulator's `0xD3` /
`0xD5` model. The diff was read, judged sound, and was about to be committed.

**Cause.** "I can read this fix" was treated as "this fix was verified".

**Check.** Reading a diff is stage one (discovery). Accepting a fix is stage
three (§8). One head cannot do both: a reader who wants the change to be
correct stops looking for what it misses. Acceptance is another agent re-running
`./tools/check.sh` and matching each finding to a hunk.

### 9.4 An agent's failure was read as its artifact failing

**Symptom.** The `stream-f-exif` agent wrote a 1389-line EXIF module under
`crates/m241-core/src/log/` and then failed. The next step considered was to
abandon that worktree and start a fresh one.

**Cause.** The worker's failure was taken as the work being invalid.

**Check.** `git status` and `git log` in that worktree first. An agent can die
after committing, or with the files already complete. The module was carried
into `feat/exif2` instead of being written again; discarding the tree would have
thrown 1389 lines away.

### 9.5 `.cargo-home` showed as untracked in every worktree

**Symptom.** The parallel worktrees symlink `.cargo-home` to one shared cache.
`/.cargo-home/` in `.gitignore` did not match, so `git status` listed the link
as untracked in every worktree — one `git add -A` from being committed.

**Cause.** A pattern ending in a slash matches a directory, and a symlink is not
a directory.

**Check.** `/.cargo-home`, with no trailing slash. Fixed in `7edfadd`; the
comment above the pattern in `.gitignore` keeps the reason attached.

### 9.6 Ten worktrees filled the disk to 85%

**Symptom.** 214 GB used; each worktree's `target/` was about 3 GB.

**Cause.** `[profile.dev]` defaulted to full debug info, and every worktree
built and kept its own copy.

**Check.** `debug = 0` in `Cargo.toml` (done in `b818676`), raised to 1 locally
only when a debugger is actually needed, and a finished worktree's `target/`
deleted. Dev builds here answer "does it compile and pass", nothing else.

### 9.7 The same incomplete work was reported twice

**Symptom.** On `feat/exif2`, 3 clippy warnings and zero tests remained while
the owner twice reported the work complete.

**Cause.** Completion was self-assessed. The stated conditions — 6/6 and at
least one test — were never checked against the artifact.

**Check.** Count what the report claims. "Tests added" means
`grep -c '#\[test\]'` finds them in the diff. "6/6" means `./tools/check.sh`
was run by the reader, not quoted from the report. A report is a claim; the
artifact is the evidence.
