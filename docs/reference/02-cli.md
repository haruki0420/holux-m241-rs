# CLI reference

`m241 <subject> <verb>`. Subjects group commands by what they act on, so the help
reads as a map of the device rather than a list of verbs.

```text
m241 device  ports | info | doctor
m241 fw      dump  | patch | flash | probe-da
m241 log     info  | download | read | erase | geotag
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

Before writing it checks, and refuses on any failure:

1. the image matches a known profile, **or** every differing byte against
   `--backup` falls inside the allowed patch offsets;
2. the image fits the flash size the DA reports;
3. the user typed `WRITE` (or passed `--yes`).

## `m241 fw probe-da`

Loads the DA into the chip's RAM and reads the two candidate DA-side firmware
bases. **Writes no flash:** only the BootROM's `0xA1` RAM writes and one `0xA8`
jump are sent, plus the DA's read-only `0xD6`. `0xD3` / `0xD4` / `0xD5` / `0xD9`
are never sent, and the DA is gone at the next power-on.

Flags: `--da <FILE>` (the vendor `DA_v3.8.4.bin`, exactly 14,896 bytes; never
committed here), `--port <PORT|auto>` (default `auto`), `--yes` (required:
without it the command stops before opening a port), `--assume-brom` (skip the
NMEA stream probe when the chip is already in BootROM), `--json`.

It reads 64 bytes at each of `0` and `0x00200000` — the two addresses the
DA-side base is documented to be — and compares the first four bytes with the
known vector table `18 F0 9F E5`. The verdict is:

* **the image is at 0x…** when exactly one address matches;
* **inconclusive** when neither matches or both do.

`fw probe-da` exists so the address `fw flash` refuses on can be settled by
measurement; it does not change `fw flash` by itself.

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

## `m241 log erase`

Erases the track log. **Destructive.** Requires confirmation or `--yes`, and
`--require-backup FILE` refuses unless the log you meant to keep is already on
disk.

The command lives in the `m241-flash` crate, not in `m241-core`: a binary that
only reads cannot link it. See [`11-safety-review.md`](../dev/11-safety-review.md).

## `m241 log geotag`

Matches photos (`<PHOTOS>...`, files or directories whose immediate regular
files are collected in sorted path order) to track points in `<LOG>` by capture
time and writes GPS EXIF tags.

Flags: `--max-gap <SECONDS>` (default `300`), `--utc-offset-days <DAYS|auto>`
(default `auto`), `--tz-offset-minutes <MINUTES>` (default `0`, for cameras
whose `DateTimeOriginal` is in local time), `--dry-run` (plan without modifying
any photo), `--json`.

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
