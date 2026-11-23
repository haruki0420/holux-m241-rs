# CLI reference

`m241 <subject> <verb>` groups commands by target:

```text
m241 device  ports | info | doctor
m241 fw      dump  | patch | flash | probe-da | da-restart
m241 log     info  | download | read | project | edit | erase | geotag | kmz
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

Machine-readable output goes to **stdout**; progress, warnings and errors go to **stderr**. `--json` prints one object; global `--verbose` raises diagnostic logging on stderr.

In `device doctor`, a **finding** (including a failed `$PMTK605` probe) goes to stdout with exit code 0 and is repeated on stderr as a warning; a **failure** (cannot list ports, unreadable drop, or `--extract-to` with no `DA_v3.8.4.bin`) prints to stderr and exits 1.

## GUI との対応

`m241-gui` は書き込み系を含め `m241_flash::flow` を共有する（`check_layout` / `check_image` / `verify_device` / 読み戻しは CLI と同じ）。

| GUI の操作 | 同じ CLI | 確認 |
|---|---|---|
| 接続「接続して確認」 / 「診断する」 | `device info` / `device doctor` | なし（読み取りのみ） |
| 接続「DA を確認」 / 「0xD9 を送る」 | `fw probe-da --yes` / `fw da-restart` | `PROBE` / `RESTART` を入力 |
| ファーム「吸い出す」 | `fw dump` | なし（2 回読み＋ハッシュ比較。1 回目のあと電源入れ直し） |
| ファーム「書き換えたイメージを保存」 | `fw patch` | なし（PC 内ファイルのみ） |
| ファーム「書き込む」 / 「復旧」 / ベンダー書き込み | `fw flash [--recover\|--allow-vendor] --yes` | `WRITE` を入力 |
| 書き込み失敗後の「0xD9 再起動」 / 「BootROM 再入を確認」 | `fw da-restart` / `fw dump --assume-brom` | `RESTART` 入力 / なし |
| ログ「ログ操作」コンソール | `log download/erase/project/read`, `nmea repair/proxy` | 消去はバックアップ＋同意後に拒否停止（実行は CLI のみ） |
| ログ地図タイル | CLI 相当なし（GUI 唯一の外部通信） | 既定 ON・設定で OFF 可（headless / `--screenshot` は非通信） |

GUI に `--yes` 相当はなく、`WRITE` / `PROBE` / `RESTART` はユーザー本人が入力する（`AGENTS.md` §2.2 / §2.5）。CLI との差: (1) 自動リトライなし、(2) `check_layout` は DA ロード後・最初の `0xD3` より前に実行（範囲定数は同じ）、(3) GUI 書き込みは**実機未検証**（成功表示は読み戻し一致のみ）。

## `m241 device ports`

Lists serial ports and flags likely M-241 cables (CP210x, PL2303, FTDI, CH340). Exits 0 when empty.

## `m241 device info`

Sends `$PMTK605`, parses `$PMTK705`, and prints release, build, model and SDK. Flags: `--port <PORT|auto>` (default `auto`), `--json`.

## `m241 device doctor`

Read-only check of environment, serial ports, optional vendor drop and optional device.

- Flags: `--port <PORT|auto>`, `--from-drop <DIR>` (scans up to 4 levels deep, hashes files, identifies known profiles and 14,896-byte `DA_v3.8.4.bin`), `--extract-to <DIR>` (requires `--from-drop`; copies out only `DA_v3.8.4.bin`, refusing overwrite with exit 5 unless `--force`), `--force`, `--json`.

## `m241 fw dump`

Reads firmware through the BootROM (**read-only**), reading twice across a user power-cycle and comparing hashes.

- Flags: `--port <PORT|auto>` (default `auto`), `-o/--output <FILE>`, `--length <BYTES>` (positive even; default `406620` for V1.13, `406312` for V1.12), `--assume-brom`, `--force`, `--json`.

## `m241 fw patch`

Rewrites the epoch constant and version marker in a local file (never touches hardware).

- Flags: `-o/--output <FILE>` (required unless `--dry-run`), `--dry-run` (prints plan, writes nothing), `--json`.
- Refuses (including under `--dry-run`): unknown profile size, unexpected stock epoch constant, or already-patched image.

## `m241 fw flash`

Writes an image through the Download Agent (**destructive**).

### Pre-write gates (refuses on any failure)

1. Image matches a known profile and is identical to `--backup` or differs only at that profile's patch offsets. No digest bypass exists in the strict path.
2. Image fits the pinned V1.13 range (`0x00000000`, length `406620`); V1.12 is shown by `--dry-run` but not writable.
3. `--yes` is passed (no typed prompt; governed by `AGENTS.md` §2.5).
4. `check_layout` accepts the pinned range.
5. `--da` is the 14,896-byte `DA_v3.8.4.bin` starting `00 00 A0 E1` whose full SHA-256 equals the pinned value (`AGENTS.md` §2.5), enforced via `DaDigest::vendor()`.

### Special modes

- `--recover`: requires image byte-identical to `--backup`, skips whole-unit `verify_device` (keeps vector/family marker check). Covered by `AGENTS.md` §2.5.
- `--allow-vendor`: separate gate for the unmodified official V1.13 image only (full SHA-256 `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435`, 406,620 bytes; V1.12 refused). Still requires `--backup` and `verify_device`. Warns that it overwrites 14 bytes of per-unit data and is unpatched stock V1.13 (7,168 days behind). Covered by `AGENTS.md` §2.3 / §2.5.

### Execution, retry and read-back

1. Enters BootROM, runs `verify_device` against `--backup` (unless `--recover`), loads DA, sends `0xD3` + `0xD5`, sends `0xD9`.
2. Re-enters BootROM via NMEA/`$PMTK180*3B` and reads back the full range. Reports success only on exact match (leaving unit in BootROM; power-cycle before use).
3. On failure before `0xD5`: sends `0xD9` (no reply wait), re-enters BootROM, reloads DA and retries **once** with the same pinned range (without repeating `verify_device`, since `0xD3` does not touch flash). On failure after `0xD5` starts: sends `0xD9`, does **not** retry, and directs to `fw dump` / `--recover`.

- Flags: `--backup <FILE>` (required), `--da <FILE>` (required unless `--dry-run`), `--port <PORT|auto>` (explicit port required under §2.5; `--port auto` is not approvable), `--yes`, `--recover` (conflicts with `--dry-run`), `--allow-vendor` (conflicts with `--recover`), `--assume-brom`, `--dry-run`, `--json`.
- 🧪 The write path, post-`0xD9` re-entry/read-back, automatic retry and `0xD3` resync have been exercised against the fake DA; see [`docs/dev/05-verification.md`](../dev/05-verification.md) for what was verified on hardware.

## `m241 fw probe-da`

Loads DA into RAM (`0xA1` + `0xA8`) and reads one 64-byte page at `0` and `0x00200000` via read-only `0xD6` (never sends `0xD3`/`0xD4`/`0xD5`/`0xD9`). Still covered by `AGENTS.md` §2.5 and the pinned `DaDigest::vendor()` check because the loaded DA can write flash.

- Flags: `--da <FILE>`, `--port <PORT|auto>` (default `auto`), `--yes` (required; no typed prompt), `--assume-brom`, `--json`.
- Judges **only the first 4 bytes** against `18 F0 9F E5` (prints `first 16:` for inspection). Verdicts: `the image is at 0x…` (both read, exactly one matches) or `inconclusive` (`not read`, `neither`, or `both`).
- ✅ Verified on hardware 2026-11-21 (`/dev/ttyUSB0`): `0x00000000` matched `18 F0 9F E5`, `0x00200000` returned `02 02 20 42` (`05-verification.md` §1.2). Multi-page `0xD6` reads remain unmeasured (`01-da-protocol.md` §5.5).

## `m241 fw da-restart`

Opens `--port <PORT|auto>` (default `auto`) at 115200 and sends one byte `0xD9` without reading a reply, touching DTR/RTS, or requiring `--yes`. Prints that `0xD9` was sent and cannot be confirmed (check with `m241 fw dump`).

- ⚠️ 未検証 on hardware (fake only; `05-verification.md` §2).

## `m241 log download`

Reads the track log over PMTK182, pausing logging and resuming it on exit/error.

- Flags: `--port <PORT|auto>` (default `auto`), `-o/--output <FILE>`, `--force`, `--json`.

## `m241 log info`

Parses a dump and reports metadata, parse counters, recomputed stats (`07-eztour-spec.ja.md` §4.2) and start/end positions. Missing facts print `-` (text) or `null` (JSON); journal-only dumps print `no track points (settings journal only)` with exit 0. Exits 1 if not an M-241 dump.

- Flags: `--coord-format decimal|dms|utm` (default `decimal`; UTM refuses polar tracks beyond 80° S / 84° N), `--json`.
- Stats: `distance` (great-circle sum across consecutive positioned points; `-` when 0 fixes), `duration` (whole seconds), `average speed` (recomputed km/h), `max speed` (largest device `SPEED`), `altitude` (min/max), `position` (`null` unless both start and end exist).

| `parse` field | Unit | Meaning |
|---|---|---|
| `blocks`, `empty_blocks`, `damaged_blocks` | blocks | walked; never-written; holding a failed record |
| `records` | records | accepted into track |
| `bad_checksums` / `empty_records` | records | failed checksum / no time or position (not in track) |
| `truncated_records` | records | bytes ran out mid-record |
| `journal_items` | items | settings-journal entries (16 B) and name separators (20 B) skipped |
| `layout_mismatched_blocks` | blocks | header mask disagreed with data record length; detected length used |
| `unparsed_bytes` | bytes | unexplained bytes skipped |
| `journal_only` | flag | ≥1 journal item, 0 records/empty/damage/unparsed |
| `used_bytes` | bytes | record bytes walked (excluding journal/padding/skipped) |

## `m241 log read`

Converts a raw dump — or `--nmea` / `--gpx` / `--kml` / `--kmz` — to `--format gpx|kml|csv|nmea|json` (default `gpx`) or a report (`--csv-report`, `--html`, `--rtf`). Writes to `-o/--output <FILE>` or stdout.

- Flags: `--format <FMT>`, `--csv-report`, `--html`, `--rtf` (mutually exclusive), `--nmea` / `--gpx` / `--kml` / `--kmz` (mutually exclusive and incompatible with `--cache`), `--utc-offset-days <DAYS|auto>` (default `auto`), `--coord-format decimal|dms|utm`, `--simplify <N>`, `--cache <FILE>`, `-o/--output <FILE>`.

### Input kinds

- **Raw dump** (default): parser warnings go to stderr; journal-only dumps warn and write an empty track (exit 0).
- **`--nmea`**: parses `$GPRMC`/`$GPGGA` (merged on shared timestamp), printing `imported` / `dropped` (bad checksum, no-fix, or missing time/coord on RMC/GGA) / `ignored` (other sentences or non-NMEA, address checked before checksum) to stderr. Non-empty file yielding 0 points is an error (exit 1); empty file exits 0.
- **`--gpx`**: reads GPX 1.0/1.1 `<trkpt>` (`ele`, `time`, `<speed>`, `<extensions><speed_kmh>`). `<wpt>` is not read. 0 `<trkpt>` is an error (exit 1).
- **`--kml`**: reads KML 2.0/2.2 `<Point>` and `<LineString>` `<coordinates>` in document order (skips `<LinearRing>`, ignores timestamps/speed). 0 geometry is an error.
- **`--kmz`**: reads `doc.kml` (or first `.kml`) from a Store/Deflate ZIP via `--kml`. Refuses ZIP64, encryption, unknown compression, or >64 MiB decompressed (enforced via `Read::take(64 MiB + 1)`). Entry CRC is **not** verified.
- **`--utc-offset-days auto`**: shifts only raw dumps whose newest record falls in the stale WNRO window. **Never applied to imported documents** (`--gpx`, `--kml`, `--kmz`); warns instead. Explicit day counts apply to every input.
- **`.dst` ("Destination File")**: not implemented (no documented layout or sample).

### Extra `--format json` fields (omitted when inapplicable)

`"nmea"` (`imported`/`dropped`/`ignored`), `"input"` (`"gpx"`/`"kml"`/`"kmz"`), `"offset_days"`, `"simplify"`, `"position"` (`format`/`start`/`end`, reflecting `--coord-format`).

### Point thinning & parse cache

- **`--simplify <N>`**: keeps every Nth point plus the last point (deterministic stride). `1` is a no-op (no report/JSON field); `0` is exit 2.
- **`--cache <FILE>`**: JSON parse cache (not ezTour's `.tki`), validated by file length + 64-bit FNV-1a hash. Replays stored `ParseReport` warnings on hit; corrupt/old cache warns and re-parses. Replaces existing `<FILE>`.

### Output formats

- **`--format csv`**: ezTour 12 columns (`INDEX`, `TRACK NUMBER`, `UTC DATE`, `UTC TIME`, `LOCAL DATE`, `LOCAL TIME`, `LATITUDE`, `N/S`, `LONGITUDE`, `E/W`, `ALTITUDE`, `SPEED`). `LOCAL DATE`/`TIME` are empty (unconfirmed vs ezTour: 未検証).
- **`--csv-report`**: `REPORT` header, `GENERATED BY,m241`, `TRACK NAME,M241 log`, 10 recomputed summary rows (`RECORDS` .. `MAX ALTITUDE M`), blank line, then the 12-column table (layout inferred; 未検証).
- **`--html`**: self-contained offline HTML with inline SVG map and summary table (no tiles, no JS, no remote requests). **MHT is not implemented.**
- **`--rtf`**: printable RTF with summary and full point table (escaped ASCII / `\uN?`; layout 未検証 vs ezTour).
- **`--format gpx`**: GPX 1.1 with 9-decimal coords, `<metadata>`, `<trkseg>`, `<speed>` (m/s, ezTour compat), `<extensions><speed_kmh>`, and `<wpt>` for manual waypoints.
- **`--format kml`**: KML 2.0 (`FeatureType` order: `<name>`, `<LookAt>`, `<Style>`s, `<Placemark>`s; validated with `xmllint` against minimal schema, `05-verification.md` §2).
- **`--format nmea`**: regenerates `$GPGGA`, `$GPGSA`, `$GPRMC` per timed/positioned point. Because measured masks (`0x1D`/`0x3D`) do not record NSAT/DOP, GGA satellite count/HDOP/geoid and GSA satellite/DOP fields are written **empty** (never `00`); GSA fix type (`3` with altitude, `2` without) is inferred from the mask.

## `m241 log project save` / `load`

Saves or executes an 8-key JSON project file (this project's own format, not ezTour's `.itm` SQLite).

- **`save <PROJECT> <LOG> <OUTPUT>`**: does not open `<LOG>`; refuses if `<PROJECT>` equals `<LOG>` or `<OUTPUT>`. Flags: `--format gpx|kml|csv|nmea|json` (default `gpx`), `--kmz` (requires `--photos`, conflicts with `--nmea`), `--nmea`, `--photos <DIR>`, `--utc-offset-days <DAYS>` (default `0`; `auto` is exit 2). Relative paths are resolved against cwd once and stored relative to `<PROJECT>`'s parent with `/` separators; absolute paths stay absolute.
- **`load <PROJECT>`**: validates all 8 required keys (`format`, `version`, `input`, `log`, `photos`, `utc_offset_days`, `output`, `export_format`), rejects unknown values/versions or KMZ without photos (naming the file), and runs `log read` or `log kmz`.

## `m241 log edit`

Applies exactly one edit to a raw dump (mutually exclusive; 0 or ≥2 is exit 2) and exports via `--format` (default `gpx`) to required `-o/--output <FILE>`. Never modifies input dumps; does not accept `--utc-offset-days`.

- `--split-at <N>`: writes first N points to `-o`, remainder to `<stem>-2.<ext>` (refuses empty half with exit 1; refuses overwriting existing sibling with exit 5).
- `--merge <FILE>` (repeatable): joins dumps ordered by earliest timestamp (untimed dumps last).
- `--delete <INDICES>`: removes comma-separated 0-based indices (out-of-range is exit 1).

## `m241 log erase`

Erases the track log (**destructive**; erase client lives in `m241-flash`).

1. Downloads raw bytes to required `-o/--output <FILE>` first (unparsed; refuses 0-byte log with exit 5; refuses existing output unless `--force`).
2. Without `--yes`: saves backup and exits 5.
3. With `--yes`: passes `BackupProof` of saved bytes, pauses logging (`$PMTK182,5`), sends `$PMTK182,6,1` only if pause succeeded, and resumes (`$PMTK182,4`) in all cases.

- Flags: `--port <PORT|auto>`, `-o/--output <FILE>`, `--force`, `--yes`, `--json` (`port`, `output`, `bytes`, `erased`).

## `m241 log geotag`

Matches `<PHOTOS>...` (files or immediate regular files of directories, sorted) to `<LOG>` by capture time and writes GPS EXIF tags.

- Flags: `--max-gap <SECONDS>` (default `300`), `--utc-offset-days <DAYS|auto>` (default `auto`), `--tz-offset-minutes <MINUTES>` (default `0`; shifts a copy of the track into camera-local time without editing EXIF time), `--no-backup`, `--group-photo-distance <METRES>` (transitive grouping into `group_id` in JSON; positive finite; off by default), `--shift-days/hours/minutes/seconds <N>` (signed, summed; day = 86,400 s; overflow is exit 1 before reading photos), `--scope <all|selected|by-camera>` (default `all`), `--photo <PATH>` (repeatable, required for `selected`), `--camera-make <MAKE>` & `--camera-model <MODEL>` (required for `by-camera`), `--dry-run`, `--json`.
- **Shift & scope**: shifts rewrite `DateTimeOriginal` alongside GPS tags (except mtime-only photos, which warn and are not stamped). Out-of-scope photos remain eligible at their unshifted time. `--scope selected` matches expanded `<DIR>/<NAME>` paths (0 matches is exit 1). `--scope by-camera` matches exact UTF-8 IFD0 `Make`/`Model` with trailing NUL/space stripped (unconfirmed vs real cameras, `06-open-questions.md`). Unused scope flags warn on stderr and are ignored.
- **Backups**: **on by default** in `GeotagOptions` (affects CLI and GUI), copying original to `<photo>_original` without overwriting an existing `_original`. Pass `--no-backup` to disable.

## `m241 log kmz`

Bundles `<LOG>` and matched `[PHOTOS]...` into an uncompressed Store ZIP at `-o/--output <FILE>` (`doc.kml` + `photo/`).

- Flags: `-o/--output <FILE>`, `--max-gap <SECONDS>` (default `300`), `--utc-offset-days <DAYS|auto>` (default `auto`), `--tz-offset-minutes <MINUTES>` (default `0`), `--max-photo-edge <PIXELS>` (scales & re-encodes oversized JPEG, dropping EXIF; undecodable oversized photo is exit 1), `--icons [PIXELS]` (adds `icon/<name>` thumbnails; default 64 px when bare; same scaling/error rule; omitted by default).
- Skipped photos warn with the matcher's actual reason. **Google Earth and `unzip` have not been run on the output** (verified only with `m241-core` ZIP/CRC tests).

## `m241 nmea repair`

Rewrites dates in `$GPRMC` / `$GPZDA` lines and recomputes checksums; all other lines pass through unchanged.

- Args/flags: `[INPUT]` (default `-`, stdin), `-o/--output <FILE>` (default stdout), `--offset-days <DAYS|auto>` (default `auto`).

## `m241 nmea proxy`

Relays a live NMEA stream with corrected dates to stdout and/or TCP.

- Flags: `--port <PORT|auto>` (default `auto`), `--baud <BAUD>` (default `38400`), `--offset-days <DAYS|auto>` (default `auto`), `--stdout`, `--bind <HOST:PORT>` (refuses non-loopback with exit 5 unless `--allow-non-loopback`), `--allow-non-loopback`, `--max-lines <N>`.
- Defaults to stdout when `--bind` is omitted; with `--bind`, writes to TCP only unless `--stdout` is also passed. Drops >128-byte unterminated fragments.

## The GUI's ログ操作 console

`m241-gui`'s Log screen provides six panels sharing core logic with the CLI:

| GUI panel | CLI equivalent | Notable differences |
|---|---|---|
| 吸い出し | `log download` | Empty log writes no file (CLI writes 0-byte file). |
| 消去 | `log erase` | Saves backup and takes confirmation, then stops at refusal (actual erase is CLI-only). |
| プロジェクト | `log project save/load` | Same 8-key JSON and relative-path rule. |
| NMEA 修復 | `nmea repair` | File-to-file only (no stdin/stdout). |
| NMEA 中継 | `nmea proxy` | Default bind `127.0.0.1:10110` (clear field to match CLI's no-TCP default). |
| 読み書き | `log read` | Same JSON schema (`nmea`, `input: "gpx"`, `offset_days`, `position` in decimal degrees; byte-identical to CLI). Refuses overwrite unless 上書き is ticked. No `--simplify`/`--cache`/`--csv-report`. |

`--port auto` in GUI filters OS port names (`/dev/ttyUSB*`, `/dev/ttyACM*`, macOS `cu.*USB*`/`SLAB`/`wchusbserial`, Windows `COM*`) rather than USB IDs. GUI-only controls: KMZ `icon/` (opt-in, default off), photo group distance, display-only coordinate format, relay file source. Hardware operations from the GUI remain 未検証.
