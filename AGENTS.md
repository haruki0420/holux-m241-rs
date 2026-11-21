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
identifier appears in `m241-core` (`src/` or `tests/`), if `m241-core`'s or
`m241-gui`'s dependency tree contains `m241-flash`, or if `set_control_lines` is
named outside the exact BootROM entry/port/simulator paths. A binary built from
`m241-core` alone therefore cannot link or call the write/erase routines in
`m241-flash` — though raw transport and framing primitives (`Port::open`,
`Transport::write_all`, `pmtk::line`, `Frame`) remain public in `m241-core`
because `m241-flash` builds on them. See
[`docs/dev/11-safety-review.md`](docs/dev/11-safety-review.md).

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
* require an explicit confirmation: type `WRITE` at the prompt, or pass `--yes`.

### 2.3 Take a dump before writing anything

Any image whose differing bytes fall outside the patch offsets of the user's own
backup is refused. An image identical to the backup is accepted. Do not weaken
that check.

### 2.4 Never commit personal data

See §6.

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

### 4.7 The three sync probes must be back to back

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
