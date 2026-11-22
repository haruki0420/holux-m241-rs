# CLI reference

`m241 <subject> <verb>`. Subjects group commands by what they act on, so the help
reads as a map of the device rather than a list of verbs.

```text
m241 device  ports | info | doctor
m241 fw      dump  | patch | flash | probe-da | da-restart
m241 log     info  | download | read | edit | erase | geotag | kmz
m241 nmea    repair | proxy
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | runtime error (I/O, parse, refused by the device) |
| 2 | usage error (argument parsing) |
| 3 | hardware not found (no port, cannot open it, no BootROM sync) |
| 4 | verification failed (hash mismatch, two dumps differ, profile mismatch) |
| 5 | safety refusal (destructive action without confirmation, missing backup) |
| 6 | flash write failed (device state undefined; re-flash your backup) |

## Output

Machine-readable output goes to **stdout**; progress, warnings and errors go to
**stderr**. `--json` prints one object. `--verbose` (global) raises diagnostic
logging on **stderr**. This is not a style preference: a script that pipes
stdout must not receive a progress bar.

A **finding** is machine-readable output: what a command looked at and what it
found, including a bad finding such as "the probe did not answer". A **failure**
is not: the command could not carry out the check at all, so it prints the reason
on stderr and either stops (exit code 1) or leaves that part out of the report.
`m241 device doctor` is where the two meet: a unit that does not answer
`$PMTK605` is a finding — it appears on stdout as `probe : error (...)` and the
exit code stays 0 — while an OS that will not list serial ports is a failure, and
the reason appears only on stderr. Findings that are worth a human's attention
are repeated on stderr as a warning, so `2>&1` and `> report.txt 2> problems.txt`
each show everything they should.

## `m241 device ports`

Lists serial ports and flags likely M-241 cables (CP210x, PL2303, FTDI, CH340).
Exits 0 with an empty list: no port is not an error.

## `m241 device info`

Sends `$PMTK605`, parses `$PMTK705`, prints release, build, model and SDK.
Accepts `--port <PORT|auto>` (default `auto`) and `--json`.

## `m241 device doctor`

Diagnoses the environment, the serial ports, an optional vendor drop
(`--from-drop`) and an optional device (`--port`). Missing ports or failed
`$PMTK605` probes are reported as labelled diagnostic findings rather than
crashing.

Flags: `--port <PORT|auto>` (probe the unit with `$PMTK605` when given),
`--from-drop <DIR>` (read-only bounded-depth scan computing SHA-256 and
identifying known firmware profiles and `DA_v3.8.4.bin` by name and 14,896-byte
size; directories up to four levels below `<DIR>` are read), `--extract-to <DIR>`
(requires `--from-drop`; copies out only an identified `DA_v3.8.4.bin`, refusing
to overwrite with exit code 5 unless `--force` is passed), `--force` (accepted on
its own, where it does nothing), `--json`.

A failed `$PMTK605` probe is a finding on stdout and the exit code stays 0; a
drop that cannot be read, or an `--extract-to` with no identified
`DA_v3.8.4.bin` under `--from-drop`, is a failure with exit code 1. See
[Output](#output) for the finding/failure split.

## `m241 fw dump`

Reads the firmware through the BootROM. **Read-only.** Reads twice and compares
hashes; between the passes it asks the user to power-cycle the logger, because a
second read from a fresh boot is what makes the comparison meaningful.

Flags: `--port <PORT|auto>` (default `auto`), `-o/--output <FILE>`,
`--length <BYTES>` (positive even count; default `406620` for V1.13, use
`406312` for V1.12), `--assume-brom` (skip the NMEA stream probe when the chip
is already in BootROM), `--force` (overwrite an existing output file), `--json`.

## `m241 fw patch`

Rewrites the effective epoch constant and the version marker in a **file**. The
device is never touched. `-o/--output <FILE>` is required unless `--dry-run` is
passed; `--dry-run` prints the plan and writes nothing. Accepts `--json`.

Refuses (including under `--dry-run`): an image whose size does not match a
known profile, an image whose epoch constant is not the expected stock value,
and an image that is already patched.

## `m241 fw flash`

Writes an image through the Download Agent. **Destructive.**

Before writing, `fw flash` compares the unit's whole firmware span with
`--backup` (`DownloadAgent::verify_device`); the library's
`DownloadAgent::write` / `write_vendor` do not call that check themselves — the
CLI does, against the unit, before the DA is loaded.

`--recover` is the explicit exception for a unit that a failed write left
different: it requires the image to be **byte-identical** to `--backup`, skips
the whole-unit comparison, and keeps the vector/family marker check so the
BootROM is still talking to an M-241. It is the only supported route for a
partial unit and is covered by `AGENTS.md` §2.5.

`--allow-vendor` is a separate gate for the **unmodified official V1.13 vendor
image**, not a backup-diff exception. It accepts only the image whose full
SHA-256 is `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435`
and whose length is 406,620 bytes; V1.12 and every other build are refused. The
normal `--backup` and unit comparison remain mandatory, and the command warns
that the vendor image overwrites the unit's 14 bytes of per-unit data and is
the unpatched stock V1.13 release (epoch `315964800`, marker `V1.13`), so dates
are 7168 days behind until the image is patched. This is
the explicit user decision recorded in `AGENTS.md` §2.3, and it is covered by
§2.5.

Before writing it checks, and refuses on any failure:

1. the image matches a known profile, and is either identical to `--backup` or
   differs from it only at that profile's patch offsets. The strict path has
   **no** digest exception; `--allow-vendor` is the separate fixed-SHA gate
   above, not a weakening of this check;
2. the image is acceptable to the driver's pinned V1.13 range (a V1.12 plan is
   shown by `--dry-run` but is not writable by this build);
3. the user passed `--yes` (there is no typed prompt in this build; `AGENTS.md`
   §2.5 governs who may pass it);
4. the layout gate accepts the pinned start `0x00000000` and length `406620`;
5. `--da` names a 14,896-byte vendor `DA_v3.8.4.bin` starting `00 00 A0 E1`
   whose **full SHA-256** equals the pinned value in `AGENTS.md` §2.5. The
   expected digest is a `DaDigest` token: a normal build can obtain one only
   with `DaDigest::vendor()`, so `--da` cannot be pointed at another DA through
   this command. The command still prints the file's full SHA-256, and a file
   with the right length and head but any other digest is refused before a port
   is opened.

A real run then enters the BootROM, compares the whole backup span with the
unit's flash (**before any erase**), loads the DA into the chip's RAM, sends
`0xD3` + `0xD5`, and sends `0xD9`. Under `--recover` the whole-unit comparison
is replaced by the vector/family marker check; under `--allow-vendor` the
comparison is still against `--backup`, and the image written is the vendor
image. The restart boots the firmware, so the read-back **re-enters the
BootROM**: it sets NMEA baud, waits for one `$…` sentence,
sends `$PMTK180*3B`, returns to 115200, and reads the range. Success is reported
only when the read-back equals the image; `WriteReport` prints the start,
length, packet count and verified byte count. For a vendor image the success
output also states that the unit's 14 bytes of per-unit data were overwritten.
A successful write leaves the unit in the BootROM, so power-cycle it before
using it. A failed read-back keeps its error and names the power-cycle and
`fw dump` checks, and it distinguishes three states: the write stopped part-way,
the range was written but the read-back could not be reached, or the read-back
ran and disagreed. None is reported as a success. The backup file is never
modified.

If the DA phase fails, the CLI leaves the chip in a defined restart state:
`0xD3` failing before any `0xD5` byte sends `0xD9` (no reply wait), re-enters
the BootROM through the same NMEA/`$PMTK180` route, reloads the DA, and retries
the write **once**. Both attempts use only `FLASH_WRITE_START = 0` and
`FLASH_WRITE_LENGTH = 0x6345C`; the range is not recomputed. The retry does not
repeat the whole-unit `verify_device` comparison: `0xD3` records a range and
calls no flash driver, so a failure before `0xD5` cannot have changed the flash,
and `AGENTS.md` §2.3's comparison before the DA is loaded was satisfied by the
first load. If the retry also fails before `0xD5`, the error says `0xD9` was
sent and that nothing was written, so the battery does not need to be removed,
and names `fw dump`. (`0xD9` has no reply, so no message claims the restart was
confirmed.) If `0xD5` has already started, the CLI sends `0xD9` but does **not**
retry automatically, because the range may be partly written; the error names
`fw dump` and `--recover` with the user's backup.

Flags: `--backup <FILE>` (required), `--da <FILE>` (required unless
`--dry-run`), `--port <PORT|auto>`, `--yes`, `--recover` (write the backup back
over a partial unit; image must equal `--backup`, conflicts with `--dry-run`),
`--allow-vendor` (write the pinned official V1.13 image; full SHA-256 gate,
conflicts with `--recover`), `--assume-brom` (the chip is already in the
BootROM), `--dry-run` (plan only; opens no port and sends nothing), `--json`.
An approval under `AGENTS.md` §2.5 requires an explicit resolved port;
`--port auto` is not approvable for a write-capable run, and
`--recover --yes` / `--allow-vendor --yes` are inside the same approval rule.

🧪 The whole write path — the post-`0xD9` NMEA re-entry and read-back, the
automatic `0xD9` restart with its one retry, and the `0xD3` reply
resynchronisation — has been exercised only against the fake Download Agent. No
unit has been flashed by this build, and the re-entry/read-back after `0xD9`, the
retry loop and the resynchronised `0xD3` reply are unverified on hardware; see
`docs/dev/05-verification.md`.

## `m241 fw probe-da`

Loads the DA into the chip's RAM and reads the two candidate DA-side firmware
bases. **Writes no flash:** only the BootROM's `0xA1` RAM writes and one `0xA8`
jump are sent, plus the DA's read-only `0xD6`. `0xD3` / `0xD4` / `0xD5` / `0xD9`
are never sent, and the DA is gone at the next power-on.

The client sends no destructive opcode, but the DA it jumps to can erase or
program flash itself. `--yes` is therefore an approval gate, not a formality:
`AGENTS.md` §2.5 covers this command, and the DA gate refuses any `--da` that is
not the pinned vendor image — exact length, exact first four bytes and the
**full SHA-256** — before a port is opened (`check_da_image_against` /
`DaLoader::load_against`, given `da_digest()`, which returns the
`DaDigest::vendor()` token; the pinned `check_da_image` / `DaLoader::load` run the
same check). §2.5 applies the same independent-review rule as `fw flash`.

Flags: `--da <FILE>` (the vendor `DA_v3.8.4.bin`, exactly 14,896 bytes starting
`00 00 A0 E1` and hashing to the pinned value in `AGENTS.md` §2.5; never
committed here), `--port <PORT|auto>` (default `auto`),
`--yes` (required: without it the command stops before opening a port; there is
no typed prompt for this command), `--assume-brom` (the same route
`fw dump --assume-brom` takes when the chip is already in BootROM), `--json`.

It reads one 64-byte page at each of `0` and `0x00200000` — the DA-side base and
the BootROM-side base — and **judges only the first four bytes** of each page
against `18 F0 9F E5`. The first 16 bytes are printed for a person (`first 16:`);
the remaining 48 are neither printed nor judged.

For every candidate the report states, as a line a user can paste into
`docs/dev/05-verification.md`:

```text
0x00000000  : read, first bytes 18 F0 9F E5 (matches the vector 18 F0 9F E5)  first 16: 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5
0x00200000  : read, first bytes 02 02 20 42 (does not match the vector)  first 16: 02 02 20 42 00 00 AA 55 AA 55 AA 55 AA 55
verdict     : the image is at 0x00000000
```

The verdict is one of:

* **the image is at 0x…** — both candidates were read and exactly one starts
  with the vector;
* **inconclusive: … not read** — at least one candidate did not answer (a
  timeout, a refusal, a checksum that stayed wrong). This **blocks** the
  verdict: a read that failed says nothing about what is at that address;
* **inconclusive: … neither starts with the vector** — both were read;
* **inconclusive: … both start with the vector** — both were read and matched,
  which is not a tie the tool breaks.

✅ A run on 2026-11-21 (`/dev/ttyUSB0`, after `--assume-brom`) returned the
output above: the DA's view has the image at `0`, while `0x00200000` reads
`02 02 20 42`, consistent with the unmapped-bus pattern. The same session
measured one `0xD6` page as 64 data bytes + a 2-byte checksum, and observed four
bytes after the banner before the first page. Multi-page reads and the advance
after the final page of a multi-page read are still unmeasured
(`docs/reference/01-da-protocol.md` §5.5, §7 item 4). See
`docs/dev/05-verification.md` §1.2 for the full transcript.

`fw probe-da` was how the DA-side base was settled; it remains a **read-only**
diagnostic and does not write flash. `fw flash` now uses the settled `0`, but the
write path itself has not run against a unit.

## `m241 fw da-restart`

Sends the DA's one-byte `0xD9` restart. **Writes no flash.** This is the software
route back from a unit that a failed DA phase left running the Download Agent in
RAM, so the battery does not have to be removed.

The command opens `--port <PORT|auto>` at 115200, sends the single byte, and
flushes. It reads no reply and does not wait for one: the DA's `0xD9` handler
resets the chip and never returns. It sends no `0xD3` / `0xD4` / `0xD5` / `0xD6`,
never touches DTR/RTS, and needs no `--yes`, because restarting the chip neither
erases nor programs flash. On success it prints:

```text
DA restart sent; no flash was written. 0xD9 has no reply, so the restart cannot be confirmed from this command: run `m241 fw dump` to check the unit answers.
```

⚠️ 未検証 on hardware: no unit has been sent a standalone `0xD9` after a failed
DA phase; the command has run against the fake only
(`docs/dev/05-verification.md` §2). If the restart works, the unit boots its
firmware, but the command cannot confirm that. If a later command needs the
BootROM, pass `--assume-brom` (the chip is silent and listening) or use the
normal NMEA probe and `$PMTK180*3B` route. `fw flash` uses the same byte
automatically when its DA phase fails, before it decides whether a retry is
safe; this command is for the case where that automatic restart could not be
sent or the unit was left by an earlier run.

Flags: `--port <PORT|auto>` (default `auto`).

## `m241 log download`

Reads the track log over PMTK182. Pauses logging for the transfer and resumes it
in a `finally`-equivalent, so a failure does not leave the device not recording.

Flags: `--port <PORT|auto>` (default `auto`), `-o/--output <FILE>`, `--force`
(overwrite an existing output file), `--json`.

## `m241 log read`

Converts a dumped log to CSV, GPX, KML, NMEA or JSON (`--format`, default
`gpx`), writing to `-o/--output <FILE>` or stdout when omitted.
`--utc-offset-days` accepts a number or `auto` (default `auto`); `auto` shifts
only if the newest record looks stale.

The shift, when one was applied, is announced on stderr and — for
`--format json` — also carried as an `offset_days` field in the exported object.
The field is absent when nothing was shifted.

## `m241 log edit`

Applies exactly one edit to a dumped log and exports the result in `read`'s
formats (`--format`, default `gpx`). One of the three operations is required and
two at once is a usage error (exit code 2):

* `--split-at <N>` — the first N points become one track and the rest another.
  The first goes to `-o/--output`; the second to a sibling named after it with
  `-2` before the extension (`track.gpx` → `track-2.gpx`; with no extension,
  `track` → `track-2`). N must leave both halves non-empty, otherwise the split
  is refused with exit code 1. The sibling is a path the user did not name, so
  an existing file there is not replaced: the split stops with exit code 5
  before writing either half, and the file must be removed or renamed first.
* `--merge <FILE>` — repeated once per dump to join. The log plus each dump are
  joined in capture-time order (earliest timestamp first; a dump with no
  timestamps sorts last). The merged track is written to `-o/--output`.
* `--delete <INDICES>` — comma-separated zero-based point indices (`3,5,9`) are
  removed from the track, which is written to `-o/--output`. An index the log
  does not have is an error (exit code 1) rather than a silent no-op.

The three operations are exclusive: none is a usage error (exit code 2), and so
are two at once. `-o/--output` is required, because a split writes two files.
**The input dump is never modified**, and neither is any `--merge` input.
`--utc-offset-days` is not accepted here: the edit works on the timestamps as
recorded, so run `log read --utc-offset-days` when the export needs a shifted
clock.

## `m241 log erase`

Erases the track log. **Destructive.**

The log is downloaded first, through the same `$PMTK182` transfer `log download`
uses, and the raw bytes are written to `-o/--output <FILE>` before any erase is
considered. Unlike `log download`, the erase command does not parse the dump: it
saves the bytes as the device sent them, because that raw copy is the recovery
route and a parser's opinion must not stand between the user and it. If the
device reports an empty log (`0` bytes), nothing is written or erased and the
command stops with exit code 5. If the backup write fails, the device is not
touched. The erase client in `m241-flash` accepts only a `BackupProof` made from
the saved bytes, and refuses an empty one, so there is no path from this command
to `$PMTK182,6,1` without a copy on disk. Logging is paused (`$PMTK182,5`)
immediately before `$PMTK182,6,1` and resumed (`$PMTK182,4`) afterwards even if
the erase fails or the stop's acknowledgement is lost. When the stop cannot be
confirmed, no erase is sent at all.

Without `--yes` the download still runs and the backup is saved; the command then
stops with exit code 5 and says where the copy is. Re-run with `--yes` to erase
(add `--force` when reusing the same output path). `--force` allows replacing an
existing output file.

Flags: `--port <PORT|auto>` (default `auto`), `-o/--output <FILE>` (required),
`--force`, `--yes`, `--json`. The JSON object carries `port`, `output`, `bytes`
and `erased`.

The erase itself lives in the `m241-flash` crate, not in `m241-core`: a binary
that only reads cannot link it. See [`11-safety-review.md`](../dev/11-safety-review.md).

## `m241 log geotag`

Matches photos (`<PHOTOS>...`, files or directories whose immediate regular
files are collected in sorted path order) to track points in `<LOG>` by capture
time and writes GPS EXIF tags.

Flags: `--max-gap <SECONDS>` (default `300`), `--utc-offset-days <DAYS|auto>`
(default `auto`), `--tz-offset-minutes <MINUTES>` (default `0`, for cameras
whose `DateTimeOriginal` is in local time), the four `--shift-*` flags and
`--scope` described below, `--dry-run` (plan without modifying any photo),
`--json`.

### Shifting photo clocks

`--shift-days <DAYS>`, `--shift-hours <HOURS>`, `--shift-minutes <MINUTES>` and
`--shift-seconds <SECONDS>` (each signed, default `0`) move the capture clock of
the photos in `--scope` and match them against the track again. A day is 24
hours (86,400 seconds), not a calendar day. The four values are summed, so there
is no separate single-value shift flag: `--shift-seconds 90` is the one-flag
form, and `--shift-days 1 --shift-hours -2` is the same 79,200 seconds. A shift
that the four values cannot express as one number of seconds is a runtime error
(exit code 1) before any photo is read. The shift is written into
`DateTimeOriginal` in the same rewrite that writes the position, so a later run
matches the photo without being told about the shift again. The exception is a
photo whose capture time comes from the file's modification time rather than
`DateTimeOriginal`: that time is not written (stamping a guess into the EXIF
would turn it into a claim), and the rewrite moves the file's own mtime as well,
so the same shift has to be given again on the next run. The run warns about such
a photo. `--dry-run` reports the shifted capture time — in the human plan line as
`capture <ISO-8601>` and in `--json` as `capture_utc` — and writes nothing;
`--json` also carries the summed shift as `shift_seconds` and the scope as
`shift_scope` beside the per-photo plans. The human summary prints
`shift: <N> s requested (<scope>)` when the shift is not zero: it names the
request, and `matched`/`written` (plus the per-photo lines) say what actually
happened.

`--tz-offset-minutes` is **not** a photo-clock shift: it moves only the track a
photo is matched against and is never written into a photo. The two adjustments
are independent and may be given together, so there is no mutual-exclusion rule
to document; `--shift-seconds` already accepts any single value on its own. With
both set, matching uses the tz-adjusted track and the shifted photo clock, the
position written is the true UTC instant, and `DateTimeOriginal` receives the
shifted (camera-local) clock.

### Which photos a shift moves

`--scope <all|selected|by-camera>` (default `all`) decides whose clock the
`--shift-*` values move. The shift flags are the operation: a `--scope` with an
all-zero shift moves nothing. A photo outside the scope is **not dropped**: it is
planned at its unshifted time and stays eligible for a position under the same
`--max-gap` rule, so being outside the scope never removes a tag the run would
otherwise have written. The scope decides only whose clock moves. A zero shift
with the default scope is exactly the behaviour of a run without any `--shift-*`
flag.

- `--scope selected` requires one or more `--photo <PATH>` (repeatable). The
  value is compared with the paths `<PHOTOS>` **expands to**, not with the text
  of the `<PHOTOS>` argument: a directory contributes its children as
  `<DIR>/<NAME>`, so with `<PHOTOS>` = `photos`, `--photo photos/a.jpg` selects
  the photo while `--photo a.jpg` or `--photo ./photos/a.jpg` does not. A run in
  which no `--photo` matches any planned path is an error (exit code 1) that
  lists a planned path as an example, rather than a silent no-op.
- `--scope by-camera` requires both `--camera-make <MAKE>` and
  `--camera-model <MODEL>`, because the match is the exact pair of Exif IFD0
  tags: `Make` and `Model` are compared with trailing NUL and space padding
  removed and with no case folding, and a name that is not valid UTF-8 is
  treated as unknown, so that photo is never selected. The rule is unconfirmed
  against ezTour and real camera files; see
  [`06-open-questions.md`](../dev/06-open-questions.md).
- `--photo` and the camera flags are read only by their own scope. Passing one
  under another scope is reported on stderr (one warning per flag) and ignored,
  rather than being turned into a second operation.

Example: a camera whose clock was an hour fast, correcting two selected photos:

```bash
m241 log geotag track.bin photos/ --scope selected \
  --photo photos/a.jpg --photo photos/b.jpg --shift-hours -1 --dry-run
```

Drop `--dry-run` to write the tags. `--shift-hours -1` moves both the match and
the written `DateTimeOriginal`, so the next run needs no shift for those photos
— unless their capture time came from the file's modification time, which is
never written (see above).

## `m241 log kmz`

Bundles `<LOG>` and its photos into a KMZ archive at `-o/--output <FILE>`: a
Store (uncompressed) ZIP with `doc.kml` and each matched photo under `photo/`.

`[PHOTOS]...` is optional and takes files or directories (a directory
contributes its immediate regular files, in sorted path order). Photos are
matched to track points by capture time with the same matcher `log geotag` uses,
so `--max-gap <SECONDS>` (default `300`) decides how far a photo may be from the
nearest point. A photo the matcher leaves out is not embedded, and the warning
on stderr carries the matcher's reason (past the gap limit, not a JPEG, no
usable capture time, and so on) rather than a guess at one, because the reason
tells the user which next step would help. A photo that cannot be placed is
never put at a coordinate it cannot support.

`--utc-offset-days <DAYS|auto>` (default `auto`) applies to the track before
matching, which is what makes a photo from an unpatched unit line up.

`--tz-offset-minutes <MINUTES>` (default `0`) is for a camera whose
`DateTimeOriginal` is local time, as in `log geotag`: matching moves a copy of
the track into the camera's clock, while the archive and the times recorded for
its photos stay UTC.

`--max-photo-edge <PIXELS>` caps the long edge of every embedded photo
(`kmzMaxPhotoEdgeSize`). A photo at or under the cap is stored byte for byte; one
over it is scaled down and re-encoded as JPEG, which **drops its EXIF** (see
[`kmz.rs`](../../crates/m241-core/src/log/kmz.rs)). A photo over the cap that
cannot be decoded is an error naming the photo, not a full-size embed.

The archive is checked by this project's own ZIP reader and CRC in `m241-core`'s
tests; **Google Earth and `unzip` have not been run on the output**. The KML
track style colour and width are this project's choice; see the same module.

## `m241 nmea repair`

Rewrites the date field in captured `$GPRMC` / `$GPZDA` lines and recomputes the
checksum. Everything else passes through byte for byte. Reads `[INPUT]` (default
`-`, standard input), writes to `-o/--output <FILE>` or stdout when omitted, and
accepts `--offset-days <DAYS|auto>` (default `auto`).

## `m241 nmea proxy`

Relays a live NMEA stream from the serial port with corrected dates to stdout
and/or a TCP server. Defaults to stdout when `--bind` is omitted. When `--bind`
is given, the stream goes to connected clients only, unless `--stdout` is also
passed, in which case it goes to both. `--bind` refuses any non-loopback address
with exit code 5 unless `--allow-non-loopback` is passed.

It runs until `--max-lines` lines have been relayed, or until the port fails
(exit code 1). A quiet port is idle, not closed: it keeps waiting. A fragment
that grows past 128 bytes without a line terminator is dropped, because it is a
baud mismatch or noise rather than a sentence.

Flags: `--port <PORT|auto>` (default `auto`), `--baud <BAUD>` (default `38400`),
`--offset-days <DAYS|auto>` (default `auto`), `--stdout`,
`--bind <HOST:PORT>`, `--allow-non-loopback`, `--max-lines <N>`.
