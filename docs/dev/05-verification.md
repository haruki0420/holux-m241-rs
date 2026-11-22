# Verification: what was measured, and what was not

This page is the only place a claim may move from 未検証 to verified. Every other
document points here.

`✅` measured on a retail M-241 · `⚠️` not measured on hardware · `🧪` covered by
the simulator and tests only — see §3 for what that is and is not worth.

## 1. The hardware sessions

### 1.1 The write and dump (2026-11-15)

Device: `B-core_1.20`, build `0043`, model `01017-00F` (V1.13), MT3318.
Host: Windows 11, CP210x bridge (VID 10C4 / PID EA60) as a COM port.

| Step | Result | Status |
|---|---|---|
| `$PMTK705` identity read | returned the strings above | ✅ |
| BootROM entry via `$PMTK180*3B`, then `0xA0`/`0x0A`/`0x50`/`0x05` | 36 probes before `0x5F` | ✅ |
| Firmware dump, 406,620 bytes, read twice | SHA-256 identical both passes | ✅ |
| Header decode | family `HOLUX_M-241`, device `01017-00F`, build 67 | ✅ |
| WNRO patch | 3 bytes changed: `0x402F6`, `0x402F7`, `0x59C2C` | ✅ |
| Flash write through the DA | every packet accepted, DA restarted | ✅ |
| Boot after flash | the logger booted | ✅ |
| `$GPRMC` after flash | date `151126` = 2026-11-15 | ✅ |
| Log download | 212,860 bytes, 10,399 records, 0 CRC errors | ✅ |

The measured dump's SHA-256 differs per unit because the image contains
per-unit runtime fields; compare against your own backup, never against a
figure on this page.

A **settlement row** for the DA-side firmware base is the only row that may
justify emptying `UNRESOLVED_STARTS`. It must contain at least:

* the exact `fw probe-da` command line that ran, including the resolved
  `--port` and the `--da` path;
* what `0xD6` returned at **both** candidates, `0` and `0x00200000` — a failed
  read is recorded as "not read", never as a mismatch;
* whether the first four bytes at the adopted base are `18 F0 9F E5`;
* the base adopted, and why the other candidate was excluded;
* the date;
* an explicit statement that the run wrote no flash (DA load into RAM only).

The `Flash write through the DA … ✅` row above is the earlier manual flash.
It is **not** a settlement row: it records no `fw probe-da` run and says
nothing about which base the DA uses, so it cannot justify emptying
`UNRESOLVED_STARTS`.

### 1.2 The DA-side base read (2026-11-21)

Device: the same retail M-241. Host: Linux, `/dev/ttyUSB0` (USB serial).

Command, after re-entering the BootROM with `--assume-brom`:

```text
m241 fw probe-da --da <DA_v3.8.4.bin> --port /dev/ttyUSB0 --yes --assume-brom
```

Output:

```text
loading the DA into RAM on /dev/ttyUSB0 (this writes no flash), then reading 64 bytes at 0x00000000 and 0x00200000
BootROM sync answered; writing the DA into the chip's RAM
DA v3.8 answered (region 0x49, flash 00C2:22BA, 524288 bytes); reading
0x00000000  : read, first bytes 18 F0 9F E5 (matches the vector 18 F0 9F E5)  first 16: 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5
0x00200000  : read, first bytes 02 02 20 42 (does not match the vector)  first 16: 02 02 20 42 00 00 AA 55 AA 55 AA 55 AA 55
verdict     : the image is at 0x00000000
flash write : no
```

* **DA-side base: `0x00000000`.** The first four bytes are the firmware vector
  `18 F0 9F E5`, and the first 64 bytes match the vendor V1.13 image's first 64
  bytes exactly.
* **`0x00200000` in the DA's view is not firmware.** It reads `02 02 20 42`,
  consistent with the unmapped-bus pattern `42 20 02 02` byte-reversed. This is
  a different address view from the BootROM's: the BootROM reads the firmware
  vector at `0x00200000`, while the DA reads it at `0`. The same number means
  different things in the two views.
* **No flash was written.** The only writes were the BootROM's `0xA1` frames
  into the DA's RAM at `0x00100800`–`0x0010422F`, and the `0xA8` jump. No
  `0xD3`, `0xD4`, `0xD5` or `0xD9` was sent.
* **Manual check in the same session:** one `0xD6` page with a 64-byte
  page-length argument is **64 data bytes + a 2-byte big-endian checksum** (66
  bytes total). The trailer of the page read at address `0` was `0x17FF`, equal
  to the sum of its 64 bytes.
* The first attempt read four bytes off (trailer `0x7035`, sum `0x1870`); after
  the drain fix the read succeeded. **Four bytes remained after the banner**;
  their origin is not established.
* Only **one page** was read at each address. Multi-page reads, the advance
  after the final page of a multi-page read, and bytes between pages were not
  exercised.

### 1.3 The first `0xD3` after the DA banner (2026-11-22)

Device: the same retail M-241. Host: Linux, `/dev/ttyUSB0`.

A real `fw flash` attempt reached the write sequence and then stopped at the
first range command:

```text
BootROM sync answered; comparing the whole unit with the backup before any erase
the unit matches the backup; loading the DA into the chip's RAM (no flash write yet)
DA v3.8 answered; programming 406620 bytes in 1589 packets
m241: error: framing error: 0xD3 answered 0xBB; expected 0x5A or 0xA5
```

* **The flash was not changed.** `0xD3` only records a range; the erase is
  inside `0xD5`, which was never sent (`docs/reference/01-da-protocol.md` §4.1,
  §5.3).
* `0xBB` is consistent with one of the **four bytes after the DA banner**
  recorded in §1.2 being read as the `0xD3` ACK. The previous client drained
  those bytes before `0xD6`; it did not drain before the first `0xD3`.
* The client now clears the input at the start of `record_range`, so both the
  first `0xD3` and `0xD6` drain the banner residue. ⚠️ The fix has **not** been
  re-run on hardware; this row records the failure that motivated it.
* A second hardware run after that clear-only fix still failed with the same
  `0xD3 answered 0xBB`. The four bytes can arrive **after** the pre-command
  clear, so a clear alone is not enough. The reply reader now resynchronises:
  it skips non-`0x5A` / non-`0xA5` bytes up to 8 within the same reply timeout,
  then reads the real `5A 00` or `A5`. On a `0xD3` failure before `0xD5`, the
  CLI also sends `0xD9`, re-enters the BootROM, reloads the DA and retries
  once; after `0xD5` starts it sends `0xD9` only. ⚠️ The automatic retry has
  not been re-run on a unit. The `0xD3` phase did succeed in the 2026-11-22 runs
  of §1.4 (they reached the `0xD5` phase), so the `0xBB` failure did not recur;
  those logs do not record which commit the binary came from, so this page does
  not promote the resynchronised read to ✅ on their strength alone.

### 1.4 The final `0xD5` packet (2026-11-22)

Device: the same retail M-241. Host: Linux over USB serial.

Two write attempts — the official V1.13 vendor image (`--allow-vendor`) and the
recovery write of the unit's own backup (`--recover`) — both reached the write
sequence and stopped on the last packet:

```text
DA v3.8 answered; programming 406620 bytes in 1589 packets
m241: error: framing error: 0xD5 packet 1588 answered 0x01, expected 0x69. packets up to DA address 0x00063400 were programmed (406528 of 406620 bytes, 99%); the range is now only partly erased and programmed.
```

* **Where it stopped:** 1,588 packets were answered `0x69`, 406,528 of 406,620
  bytes were programmed, and the failure is packet index 1,588 — the 1,589th and
  last. The run reached 99%; it did not fail early or on the range command.
* **What the client sent:** every packet was zero-filled to `PACKET_SIZE`
  (`0x100`) and the two-byte trailer was the sum of the padded payload. The
  final packet therefore went out as 256 payload bytes + 2 checksum bytes,
  where the DA reads only 92 payload bytes + 2 (`0x00102474`,
  `docs/reference/01-da-protocol.md` §4.3).
* **Why that cannot pass the packet check:** the DA reads the first 92 bytes as
  payload and the next two — the client's zero padding — as the trailer, so the
  sums disagree (§4.4 of the same page). For the 406,620-byte image the
  remainder is `406620 mod 256 = 92`.
* **`0x01` is not explained.** §4.4's mismatch path ends in `0xA5` and a
  restart, but the wire carried `0x01`; which DA status it is, is **not**
  established. The client reports it as a framing failure and does not retry
  on it.
* **The `0xD3` phase succeeded in both runs.** Both reached the `0xD5` phase, so
  the `0xBB` failure of §1.3 did not recur here; the failure is later, in
  `0xD5`. Which commit the binary came from is not in these logs, so this does
  not promote the resynchronised read to ✅.
* **Both runs ended with a `0xD9` restart**, which has no reply, so the restart
  is not confirmed by these logs. The flash is partly erased and programmed at
  this point; the recovery path (`m241 fw dump`, then `fw flash --recover`) is
  the documented next step.
* **The fix was re-run on a unit later the same day and completed**; §1.5
  records the runs, the read-back and the boot. This section is the failure
  that motivated the change.

### 1.5 The writes that completed (2026-11-22)

Device: the same retail M-241 (`B-core_1.20`, build `0043`, model `01017-00F`,
flash `00C2:22BA`). Host: Linux, `/dev/m241` (udev symlink to `/dev/ttyUSB0`).
Binary: `fix/flash-d3-resync` at `b8d91ee` — the client that sends the final
`0xD5` packet at the DA's receive length (§1.4); that branch is PR #19, merged
into `main` as `877881c`, and this page records the runs, not the merge. Every
write below was approved per execution by an independent reviewer
(`AGENTS.md` §2.5). The two strict writes compared the unit with the backup
before the DA was loaded (`verify_device`); the **recovery write did not** —
`--recover` skips that comparison by design, because a partly written unit
cannot match its backup, and its log says so (`recovery mode: … the whole-unit
comparison is skipped; checking only the M-241 vector and family markers`).

| Step | Command | Result | Status |
|---|---|---|---|
| identity | `m241 device info` | `B-core_1.20` / `0043` / `01017-00F` / sdk 1.0 | ✅ |
| dump | `m241 fw dump` | 406,620 bytes, two reads identical, SHA-256 `9f192786…` | ✅ |
| DA base | `m241 fw probe-da` | DA v3.8, `0x00000000` = `18 F0 9F E5`, `0x00200000` = `02 02 20 42`, `flash write : no` | ✅ |
| vendor write | `m241 fw flash --allow-vendor` | 1589/1589 packets, 406,620 bytes programmed; read-back differed at `0x5C` (see below) | ⚠️ |
| recovery write | `m241 fw flash --recover` (the unit's own dump) | 1589 packets, `verified: 406620 bytes read back and identical` | ✅ |
| patch | `m241 fw patch` on a fresh V1.13 dump | 3 bytes changed (`0x402F6`, `0x402F7`, `0x59C2C`), epoch `935280000`, marker V1.14 | ✅ |
| patched write | `m241 fw flash --yes` (patched image, backup = the fresh dump) | 1589 packets, `verified: 406620 bytes read back and identical` | ✅ |
| boot of V1.14 | `m241 device info` + NMEA | `device info` answered; the NMEA default date was `22-08-99` (1999-08-22, the patched epoch `935280000`; unpatched would be `06-01-80`) | ✅ |

The raw command output quoted on this page was captured in the session that ran
the commands and is not committed here: the captures are device transcripts and
the repository does not take them (`AGENTS.md` §6). The quoted lines are
verbatim; each is the line the client printed.

* **The write path completed on a unit for the first time.** Both successful
  runs reached the final packet, were acknowledged, restarted the chip with
  `0xD9`, re-entered the BootROM through the NMEA / `$PMTK180*3B` route and read
  the whole 406,620-byte range back; the read-back matched the written image
  exactly. The fake and the unit tests are no longer the only evidence.
* **The patched image came from a unit dump, not from the vendor file.**
  `fw patch` was run on the fresh V1.13 dump, so the patched image carries the
  unit's current per-unit bytes; the strict write compared it with that dump and
  only the three patch bytes differ.
* **The patched firmware boots, and the running firmware carries the patched
  epoch.** After the strict write the unit was power-cycled by the user and the
  client answered: `device info` returned `B-core_1.20` / `0043` /
  `01017-00F`, and the NMEA stream reported the default date `22-08-99`
  (1999-08-22) — the value the patched constant `935280000` produces when there
  is no fix. The stock image reported `06-01-80` in the same session, so this is a
  measured difference between the two images, not a guess from the write.
* **未検証: the date with a real fix.** With no satellites indoors the unit
  reported its default time right after the write, not a GPS-derived one; that a
  live fix now produces today's date has not been observed. Later on 2026-11-22
  the same unit, still with no fix, reported the current UTC date — see
  "The no-fix date follows the host clock" below — so the no-fix clock does move
  past the constant, but how it gets set is not established. Recording a track
  and reading it back with the WNRO-corrected timestamp is the next step, and it
  needs sky, not a cable.

#### The no-fix date follows the host clock (2026-11-22)

Device: the same retail M-241, running the patched V1.14 image written above.
Host: Linux, the unit over USB serial. The host clock stood at
`2026-11-22T15:23:11Z`–`2026-11-22T15:23:22Z` across the captures.

| Step | Result | Status |
|---|---|---|
| `m241 device info` | `B-core_1.20` / `0043` / `01017-00F` / sdk 1.0 | ✅ |
| raw NMEA, first sentence of the window | `$GPRMC,152314.586,V,8960.0000,N,00000.0000,E,0.00,0.00,221126,,,N*7E` | ✅ |
| raw NMEA, the `$GPGGA` of the same second | `$GPGGA,152314.586,8960.0000,N,00000.0000,E,0,0,,137.0,M,13.0,M,,*48` | ✅ |
| the same stream through `m241 nmea proxy` | `$GPRMC,152322.586,V,…221126,,,N*7B` at host `15:23:22Z`; the proxy applied no offset | ✅ |
| fix state | `$GPGGA` quality `0`, `0` satellites; `$GPRMC` status `V` | ✅ |
| date and time against the host | both dates `221126` (2026-11-22); sentence time within a few seconds of the host clock | ✅ |

* **Without a fix the unit reports the current UTC date and time.** Eight
  `$GPRMC` and eight `$GPGGA` sentences were captured between host
  `15:23:11Z` and `15:23:19Z`; every date field was `221126`, and the time
  fields ran from `152314.586` to `152321.586`, i.e. with the host clock. A
  second capture through `m241 nmea proxy` at `15:23:22Z` printed
  `$GPRMC,152322.586,V,…221126…`. A separate pass minutes earlier
  (`$GPRMC,152224.586,…,221126,…` and `$GPGGA,…,0,0,…` at `15:22:25Z`) agrees.
* **This is not the constant.** The stock image reported `06-01-80` (its epoch
  `315964800`) and the patched image reported `22-08-99` (its epoch
  `935280000`) when the unit was started with no fix (above). Today's `221126`
  is neither, so the sentence is carrying a clock value, not just the constant.
* **What is still 未検証.** Whether the clock behind this date is a free-running
  RTC, a value set by an earlier GPS fix, or something else is not established;
  the unit was not observed from a known-unset clock to this state, and no
  unpatched image with a running clock was observed. How much of the correct
  date comes from the patch and how much from the clock's current value is
  therefore not separated. The date from a **live fix** remains unobserved, and
  the source of the time (RTC or GPS time) is not established.
* The raw capture and the command output were written to a temporary directory
  outside the repository and are not committed (`AGENTS.md` §6); the sentences
  above are verbatim.

#### The log read back after the writes (2026-11-22)

Device: the same unit, still recording. The read is
`m241 log download --port <port> --output <file>`, run twice about ten seconds
apart; nothing was erased or written.

| Step | Result | Status |
|---|---|---|
| first download | 3,228 bytes, 135 records, exit 0, no framing error | ✅ |
| second download | 3,260 bytes, 137 records | ✅ |
| block header | `1 of 1 blocks failed their header checksum` in both | ✅ |
| record checksums | every record failed (`bad checksums: 135`, then `137`) | ✅ |
| decoded span | `2060-09-25T04:18:50Z`–`2015-05-08T05:53:12Z`, then `1974-01-28T17:15:22Z` | ⚠️ not trustworthy |
| log usage through the GUI minutes later | `3,292 B`, recording 「有効」 | ✅ |

* The transport was clean: both runs exited 0 with no framing error, and the
  unit's log usage grew between them (3,228 → 3,260 bytes, 135 → 137 records),
  so the log was being written while it was read. The GUI then reported
  `3,292 B`.
* **The download cannot be used as evidence of a post-flash timestamp.** The
  unit's only block fails its header checksum and every record fails its
  per-record checksum, and the spans the client decodes are nonsense. A 64 KiB
  block that is still being written may not carry a final header, so the record
  boundaries derived from it may be wrong; whether that explains the failures or
  the log area is damaged is **未検証**. The `fixes` count and the decoded dates
  are not used anywhere.
* The raw dump is not committed: it is the user's track data (`AGENTS.md` §6).

#### The GUI on a real unit (2026-11-22)

Device: the same unit, running the patched V1.14 image. Host: Linux, X11.
Binary: `target/debug/m241-gui` built from this working tree; the exact commit
it was built from was not recorded.

| Step | Result | Status |
|---|---|---|
| start the window with the unit attached | the Connect screen listed the host's serial ports and had the unit's port selected by default; its diagnostic line named the CP210x / PL2303 / FTDI / CH340 cable types | ✅ |
| 「接続して確認」 | 「接続済み（読み取り専用）」; firmware `B-core_1.20`, build `0043`, model `01017-00F`, SDK `1.0`; log status 「既知リリース（日付修正対象）」 | ✅ |
| log usage through the same probe | `3,292 B`, recording 「有効」 | ✅ |
| open a downloaded log in the Log screen | not exercised: its file-path field needs keyboard input, and no keyboard could be injected into the X11 window in this session | ⚠️ |
| 追従 render on a real track | not exercised (it needs a log loaded in the previous row) | ⚠️ |
| photo geotag / JPEG write through the GUI | not exercised: needs a real camera JPEG and a photo folder; that path is still covered by synthetic JPEGs only | ⚠️ |

* The connect probe **reads** only: the screen says 読み取り専用, and the probe
  path never pulses DTR/RTS (`AGENTS.md` §2.1). It opened the port, read
  `$PMTK705`, then asked the unit for its log usage.
* The usage the GUI showed (`3,292 B`) agrees with the `log download` byte
  counts of the same minutes (3,228 then 3,260), so the GUI and the CLI read the
  same log.
* The button was reached through AT-SPI: the driver resolved the accessible
  under the clicked point and dispatched its `click` action, and the screen then
  changed as shown. This records how the screen was driven, not that the GUI is
  operable by keyboard or screen reader.
* The screenshots were taken but are not committed: they show this host's port
  list (`AGENTS.md` §6).

#### The vendor write completes but the read-back differs at `0x5C`

The stock V1.13 vendor image was written 100% (all 1589 packets acknowledged,
406,620 of 406,620 bytes), and the unit booted it: `device info` answered and
the NMEA default date was `06-01-80` (epoch `315964800`, unpatched). The
read-back still failed, at one byte: the unit held `0x91` at `0x5C` where the
vendor image has `0xFF`.

A fresh dump taken after that boot shows the unit restored exactly `0x5C`–`0x5F`
to the values of its own earlier dump. Those four bytes are inside the 14 bytes
of per-unit data the vendor gate warns about, and the booted firmware re-creates
them. The read-back is taken after a restart and a BootROM re-entry, so a vendor
write cannot compare equal at those offsets even when the write itself is
correct.

The other ten per-unit bytes (`0xF8`–`0xFD`, `0x63458`–`0x6345B`) are not
restored by the firmware: after the vendor write they held the vendor values.

未検証: where the restored four bytes come from (a second copy in the chip, or a
derivation), and whether the vendor read-back should ignore the
firmware-managed offsets — a design decision that has not been made.

#### V1.12 is refused before a port is opened

Measured, not assumed — two dry runs, so no port was opened:

```text
$ m241 fw flash <V1.12 image> --backup <406,620-byte dump> --dry-run
m241: error: refused: the backup is 406620 bytes; profile V1.12 is 406312 bytes

$ m241 fw flash <V1.12 image> --backup <406,620-byte dump> --allow-vendor --dry-run
m241: error: refused: the vendor gate is pinned to V1.13 (406620 bytes, SHA-256 32fe5269…); profile V1.12 is not that release. V1.12 and other vendor builds are not supported
```

Both exit 5.

## 1a. The Linux log-erase hardware session (2026-11-21)

Device: retail Holux M-241 (MT3318) over USB serial (CP210x bridge).
Host: Linux, `m241` built from commit `bf41c2e`, the then-head of
`feat/log-erase-wire`. That commit is **not** an ancestor of this branch, and no
binary from this branch's final head has been run on hardware.

The rows below were measured with that binary only. The log-erase work has
gained changes since: the empty-log refusal (`ea53d1a`) and the lost-stop-ACK
resume with its tests. **Those changes have not been run on hardware**, so
nothing on this page makes the final head verified.

| Step | Command / detail | Result | Status |
|---|---|---|---|
| Bare `$PMTK182,6` (negative result) | `m241 log erase --port <port> --output <backup> --yes` before fix | backup saved (`225,028` bytes), failed on trailing `$PMTK001,182,4,3*23`; follow-up `m241 log download` showed `225,060` bytes / `7,846` records (log untouched) | ✅ (negative) |
| Log download & erase (`$PMTK182,5` → `$PMTK182,6,1*3E` → `$PMTK182,4`) | `m241 log erase --port <port> --output <backup> --yes` | backup saved (`225,092` bytes), `erased: yes`, exit `0` | ✅ |
| Post-erase verification | `m241 log download --port <port> --output <check> --force` | `528` bytes (`0x210`), `records: 0`, `fixes: 0`, exit `0` | ✅ |
| Final-head re-run (empty-log refusal, lost-stop-ACK resume) | not run: the binary above predates both changes | — | ⚠️ |

## 2. What was never measured

| Claim | Why it is still open |
|---|---|
| The OLED after flashing | never observed; the display was not photographed |
| The binary log's timestamps after flashing | ⚠️ a post-flash log **was** downloaded on 2026-11-22 (§1.5), but the unit's only block failed its header checksum and every record failed its checksum, so the decoded dates were nonsense; the download therefore does not verify a timestamp. Whether a block that is still being written explains it, or the log area is damaged, is still 未検証 |
| The NMEA date from a live GPS fix after patching | ⚠️ with no fix the unit reported the then-current UTC date on 2026-11-22 (§1.5), but that is a clock value, not a fix-derived one; the patched default with a zero clock is `22-08-99` (epoch `935280000`). No fix was available indoors, so a GPS-derived date after patching is still unobserved |
| The GUI's Log screen, 追従 render and photo write on a real unit | ⚠️ not exercised. The Connect screen and 「接続して確認」 did run against the unit on 2026-11-22 (§1.5); the Log screen needs a log opened through its keyboard-driven path field (no keyboard could be injected into the X11 window) and the photo write needs a real camera JPEG |
| `m241 device doctor` against a unit | the command and its tests exist, but no hardware session in §1 ran the port probe, the drop scan or the DA extraction on a unit |
| `m241 log geotag` on a real camera JPEG | only synthetic JPEGs are written by the tests; no file from a real camera has been geotagged, and a real camera's IFD0 layout has not been read |
| `m241 nmea proxy` on a live stream | ✅ relayed a live stream from the unit to stdout on 2026-11-22 (§1.5): repaired `$GPRMC` / `$GPGGA` lines were printed, and with the dates already current the `auto` offset applied nothing. The TCP `--bind` relay was not exercised |
| Linux BootROM / firmware dump & flash with a real logger | ✅ measured 2026-11-22 (§1.5): `fw dump`, `fw probe-da`, three `fw flash` runs and their read-backs all ran on Linux over `/dev/ttyUSB0` |
| macOS with a real logger | ⚠️ never measured: every hardware session in §1 and §1a was Linux over USB; no macOS host was used and the binary was never launched on a Mac |
| Bluetooth SPP | all hardware work went over USB |
| 4 MiB units and GR-245 variants | geometry maths is implemented and unit-tested; no device was available |
| V1.11 and V1.12 on hardware | no V1.11 image to profile. The V1.12 release was measured as **refused before opening a port** on 2026-11-22 (§1.5), so it was never flashed by this client |
| The V1.12 patch offsets (`0x402F6`, `0x402F7`, `0x59B18`) | derived from the V1.12 vendor package's layout (`0x59B14` marker, size and SHA-256 in `03-hardware-facts.md`), not from a V1.12 unit; V1.12 was never dumped or flashed here |
| Any timing figure | nothing in this repository is a stopwatch measurement |
| The Rust `fw flash` command on a unit | ✅ the 2026-11-15 flash was an earlier manual/v1 run; on 2026-11-22 the Rust client completed two writes and read each written range back identical (§1.5). The write, erase, restart and backup-diff gates are no longer covered by the simulator only |
| The final `0xD5` packet sent short (92 + 2) | ✅ measured 2026-11-22 (§1.5): the zero-padded final packet failed on hardware (§1.4); the client that sends the remainder plus its sum completed two writes and read both back identical |
| What `0x01` means in a `0xD5` reply | observed 2026-11-22 (§1.4); the disassembly page explains why a padded final packet cannot pass the checksum, but not which DA status reaches the wire as `0x01` |
| Writing the official V1.13 vendor image (`--allow-vendor`) | ⚠️ written 2026-11-22 (§1.5): all 1589 packets acknowledged, 406,620 bytes programmed, and the unit booted the stock V1.13 (NMEA default date `06-01-80`). The read-back then differed at `0x5C`, because the booted firmware restores its four per-unit bytes; the 14-byte overwrite was therefore observed, but the read-back gate cannot confirm a vendor write as it stands |
| The firmware restoring per-unit bytes at boot | ✅ observed 2026-11-22 (§1.5): after the vendor write the unit held `91 1B D4 CF` at `0x5C`–`0x5F` — its own earlier values — where the image has `FF`. Where those bytes come from and whether the vendor read-back should skip them are 未検証 |
| The `0xD9` → NMEA probe → `$PMTK180*3B` re-entry and read-back | ✅ measured 2026-11-22 (§1.5): after both successful writes the client restarted the chip, re-entered the BootROM through the NMEA hand-over and read the whole range back. The 2026-11-15 row recorded that the unit booted, not that this read-back ran |
| The automatic `0xD9` restart and one retry after a `0xD3` failure | still not exercised on a unit: with the final-packet fix in place no `0xD3` failure occurred, so the restart-and-retry path ran only against the fake. The retry uses the same `FLASH_WRITE_START` / `FLASH_WRITE_LENGTH` and never recomputes the range |
| `m241 fw da-restart` | ✅ measured once 2026-11-22: after `fw probe-da` left the DA running in RAM, the standalone `0xD9` was sent and the next `device info` answered, so the unit had booted its firmware. The command sends one byte, waits for no reply and never touches DTR/RTS; a unit sitting in the BootROM is not restarted by it (measured the same day) |
| The 406,620-byte BootROM read split into `DUMP_CHUNK` frames | ✅ measured 2026-11-22 (§1.5): `fw dump` read 406,620 bytes in two independent passes that were identical, and the write read-back read the same range in one pass. A single 406,620-byte `0xA2` would exceed the five-second deadline at 115,200 8N1 |
| The DA's `0xD6` read across multiple pages | one page per address was read on 2026-11-21; the advance between pages, the advance after the **last** page of a multi-page read, and any bytes the DA sends between pages are unmeasured (`docs/reference/01-da-protocol.md` §5.5, §7 item 4) |
| The four bytes after the DA banner | origin is still not established. They were read as `0xBB` by the first `0xD3` on 2026-11-22 (§1.3); with the resynchronising read in place, every later run that day reached and passed the `0xD3` phase (§1.5). The client clears the input before `0xD6` and before the first `0xD3` and resynchronises the `0xD3` reply (skips up to 8 non-ACK bytes) |
| `0xD6`'s page length for arguments other than 64 | only a 64-byte page-length argument was measured (64 data bytes + 2-byte trailer) |
| `fw probe-da` on stock V1.13 and on a second unit | the 2026-11-21 run read the unit from §1.1, which already carries the WNRO patch (the version marker is patched to V1.14); its first 64 bytes match the vendor V1.13 image's, but no stock unit and no second unit were probed |
| The `0xA8` jump frame's count word | the protocol page gives it no meaning; the only client that ran the jump on a unit sent `0`, and this crate does too. Whether the BootROM reads the field at all is not established (`docs/dev/03-hardware-facts.md` "DA jump") |

## 3. The simulator

`crates/m241-core` is exercised by a fake device that models the chip's own
behaviour: echo, the checksums, the 16-bit pair swap, the two read widths, and
the PMTK command/reply framing. It is not a mock: it drives the real client code
paths. The Download Agent is no longer modelled there — it belongs to the write
crate, and its fake lives in `crates/m241-flash/src/simulator.rs`.

**It is not a proof of correctness, and 🧪 never means "verified".**

The reason is a circular reference. The simulator and the client share the same
functions: the fake swaps 16-bit pairs with the crate's own `swap_pairs`, and the
client undoes it with the same function. If that function's notion of the byte
order is wrong, both halves are wrong in the same direction and the round trip
still passes. The same holds for the checksum, the frame layout and every
constant that is read from the crate rather than written out twice. A test that
passes therefore shows the two halves agree; it does not show either half agrees
with the device.

What the simulator *is* good for is a **regression test**: it catches the
refactor that quietly changes behaviour, because the behaviour it compares
against was recorded from a version that was checked another way. That is a real
job — most of the protocol's costliest bugs were regressions, not first attempts
— but it is a different job from establishing that the first attempt was right.

Correctness is carried by three things instead:

1. **The citation.** Every protocol constant points at a measurement in
   [`03-hardware-facts.md`](03-hardware-facts.md) or at a function and
   instruction range in [`../reference/01-da-protocol.md`](../reference/01-da-protocol.md).
   A constant that cannot be cited is a guess and is labelled as one.
2. **The hardware run.** Section 1 is the only place a claim becomes ✅. A
   command that never ran against a unit stays ⚠️ no matter how green the suite
   is.
3. **The review.** [`11-safety-review.md`](11-safety-review.md) fixes the
   questions a diff is read against, and `tools/check_safety.py` turns the ones
   that can be mechanical — the read/write crate split — into a gate.

🧪 marks a claim covered only by the simulator and tests. It is weaker than ✅ and
the documents say so at the point of use, not only here. Treat it as "this is
pinned, so it will not drift unnoticed", never as "this is known to be right".

## 4. How to close a gap

1. Run the flow against a real device.
2. Record the command, the output, and the date in section 1.
3. Update the `⚠️` at the point the claim is made.

If you cannot do all three, the claim stays `⚠️`. Writing `✅` for an untested
platform is a defect, not optimism.
