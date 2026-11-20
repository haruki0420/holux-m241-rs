# CLI reference

`m241 <subject> <verb>`. Subjects group commands by what they act on, so the help
reads as a map of the device rather than a list of verbs.

```text
m241 device  ports | info | doctor
m241 fw      dump  | patch | flash
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

## `m241 device ports`

Lists serial ports and flags likely M-241 cables (CP210x, PL2303, FTDI, CH340).
Exits 0 with an empty list: no port is not an error.

## `m241 device info`

Sends `$PMTK605`, parses `$PMTK705`, prints release, build, model and SDK.
Accepts `--port <PORT|auto>` (default `auto`) and `--json`.

## `m241 device doctor`

Diagnoses the environment, the ports, the vendor drop (with `--from-drop`) and the
device (with `--port`). Reports a missing `pyserial` equivalent as a labelled
failure, never a traceback.

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

## `m241 log erase`

Erases the track log. **Destructive.** Requires confirmation or `--yes`, and
`--require-backup FILE` refuses unless the log you meant to keep is already on
disk.

## `m241 log geotag`

Matches photos to track points by capture time and writes GPS EXIF data, with a
configurable maximum gap and a dry-run that lists what would change.

## `m241 nmea repair`

Rewrites the date field in captured `$GPRMC` / `$GPZDA` lines and recomputes the
checksum. Everything else passes through byte for byte. Reads `[INPUT]` (default
`-`, standard input), writes to `-o/--output <FILE>` or stdout when omitted, and
accepts `--offset-days <DAYS|auto>` (default `auto`).

## `m241 nmea proxy`

Relays a live NMEA stream with corrected dates, to stdout or a TCP port. Binds to
loopback only unless explicitly told otherwise.
