# Verification: what was measured, and what was not

This page is the only authority that may move a claim from 未検証 to verified.
`✅` measured on a retail M-241 · `⚠️` not measured on hardware · `🧪` simulator and tests only (§3).

## 1. The hardware sessions

### 1.1 Initial dump and write (2026-11-15, v1 / manual)

Device: `B-core_1.20`, build `0043`, model `01017-00F` (V1.13), MT3318. Host: Windows 11, CP210x (VID 10C4 / PID EA60).

| Step | Result | Status |
|---|---|---|
| `$PMTK705` identity | returned strings above | ✅ |
| BootROM entry (`$PMTK180*3B` + sync) | 36 probes before `0x5F` | ✅ |
| Firmware dump (406,620 B, ×2) | identical SHA-256 both passes | ✅ |
| Header decode | `HOLUX_M-241`, `01017-00F`, build 67 | ✅ |
| WNRO patch | 3 bytes changed (`0x402F6`, `0x402F7`, `0x59C2C`) | ✅ |
| Flash write via DA + boot | all packets accepted, booted; `$GPRMC` date `151126` (2026-11-15) | ✅ |
| Log download | 212,860 B, 10,399 records, 0 CRC errors | ✅ |

Dump SHA-256 differs per unit (per-unit runtime fields); compare against your own backup.

**Settlement row rule** (for emptying `UNRESOLVED_STARTS`): must record exact `fw probe-da` command with resolved `--port`/`--da`, `0xD6` output at both `0` and `0x00200000` (failed read = "not read"), whether first 4 bytes are `18 F0 9F E5`, adopted base & why other excluded, date, and explicit "no flash written". The 2026-11-15 manual flash above is **not** a settlement row.

### 1.2 DA-side base settlement (2026-11-21)

Device: same M-241. Host: Linux `/dev/ttyUSB0`.

```text
m241 fw probe-da --da <DA_v3.8.4.bin> --port /dev/ttyUSB0 --yes --assume-brom
0x00000000  : read, first bytes 18 F0 9F E5 (matches the vector 18 F0 9F E5)  first 16: 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5
0x00200000  : read, first bytes 02 02 20 42 (does not match the vector)  first 16: 02 02 20 42 00 00 AA 55 AA 55 AA 55 AA 55
verdict     : the image is at 0x00000000
flash write : no
```

- **Settled DA-side base: `0x00000000`** (first 64 B match vendor V1.13). `0x00200000` reads `02 02 20 42` (unmapped bus; BootROM view differs from DA view).
- **No flash written** (`0xA1` RAM load to `0x00100800`–`0x0010422F` + `0xA8` jump + `0xD6` read only).
- **Single-page `0xD6` measured**: `page_len = 64` emitted 64 data bytes + 2-byte BE checksum `0x17FF` (66 B). 4 unexplained bytes remained after the 20-byte banner (drained before `0xD6`). ⚠️ Multi-page reads and post-final-page advance remain unmeasured.

### 1.3 First `0xD3` banner residue (2026-11-22)

Host: Linux `/dev/ttyUSB0`. Two `fw flash` attempts failed before `0xD5` with `0xD3 answered 0xBB` (flash untouched) because the 4 post-banner bytes can arrive *after* a pre-command clear. Client now resynchronises up to 8 non-`0x5A`/non-`0xA5` bytes and retries once via `0xD9` if failure occurs before `0xD5`. ⚠️ Automatic retry has not been triggered on hardware.

### 1.4 Final `0xD5` packet failure (2026-11-22)

Two runs (`--allow-vendor` and `--recover`) programmed 1,588/1,589 packets (406,528/406,620 B, 99%) and failed on packet 1,588 with `0x01` (expected `0x69`). Cause: client zero-padded the final packet to 256 B + 2 B sum, whereas DA reads only `406620 mod 256 = 92` B + 2 B sum (`01-da-protocol.md` §4.3). ⚠️ Exact meaning of wire byte `0x01` remains unestablished.

### 1.5 Completed Rust writes and sessions (2026-11-22 / 2026-11-23)

Device: same M-241. Host: Linux `/dev/m241` → `/dev/ttyUSB0`. Binary: `fix/flash-d3-resync` (`b8d91ee`, merged to `main` as `877881c`), each write independently approved per `AGENTS.md` §2.5.

| Step | Command | Result | Status |
|---|---|---|---|
| identity | `m241 device info` | `B-core_1.20` / `0043` / `01017-00F` / sdk 1.0 | ✅ |
| dump | `m241 fw dump` | 406,620 B, 2 reads identical, SHA-256 `9f192786…` | ✅ |
| DA base | `m241 fw probe-da` | `0x00000000` = `18 F0 9F E5`, `0x00200000` = `02 02 20 42`, `flash write : no` | ✅ |
| vendor write | `m241 fw flash --allow-vendor` | 1589/1589 pkts written, booted stock V1.13 (`06-01-80`); read-back differed at `0x5C` (see below) | ⚠️ |
| recovery write | `m241 fw flash --recover` | 1589 pkts, `verified: 406620 bytes read back and identical` (`verify_device` skipped by design) | ✅ |
| patch | `m241 fw patch` on fresh dump | 3 bytes (`0x402F6`, `0x402F7`, `0x59C2C`), epoch `935280000`, marker V1.14 | ✅ |
| patched write | `m241 fw flash --yes` | 1589 pkts, `verified: 406620 bytes read back and identical` | ✅ |
| boot V1.14 | `device info` + NMEA | answered; default no-fix date `22-08-99` (epoch `935280000`; stock was `06-01-80`) | ✅ |

- **Vendor write `0x5C` discrepancy**: firmware restores its 4 per-unit bytes `0x5C`–`0x5F` (`91 1B D4 CF`) on boot, so post-reboot read-back differs from vendor `0xFF` at `0x5C`. The other 10 per-unit bytes (`0xF8`–`0xFD`, `0x63458`–`0x6345B`) held vendor values. Source of restored bytes and read-back policy remain open ([`06-open-questions.md`](06-open-questions.md) §8).
- **V1.12 refused before port open**: dry runs exited 5 for both normal and `--allow-vendor` (`32fe5269…` pin).
- **No-fix clock & `nmea proxy` (2026-11-22/23)**: later with no fix (`GGA` quality 0, `RMC` `V`), unit emitted `$GPRMC` date `221126` (2026-11-22) matching host clock; `nmea proxy` relayed to stdout and `tcp://127.0.0.1:10110` (9/9 valid checksums; `--offset-days 1` → `231126`). ⚠️ Live GPS fix date, RTC vs GPS time origin, long TCP runs, multi-client, and non-loopback bind remain 未検証.
- **Post-write log download (2026-11-22)**: 3,228 B then 3,260 B (no framing errors; GUI read 3,292 B). Offline analysis showed 0 track points — settings journal only (141×16 B + 23×20 B). Re-parsing with PR #34 yields `journal_only: true` (164/166 items) and fixes the 2026-11-21 225,028 B dump to 10,942 points / 0 bad checksums / 248 journal items. ⚠️ Recording track points after flash remains 未検証.
- **GUI on real unit (2026-11-22, X11)**: Connect screen listed ports and 「接続して確認」 read `B-core_1.20` / `0043` / `01017-00F` / SDK `1.0` and log usage `3,292 B` (有効) ✅. ⚠️ Log open, follow, geotag, KMZ, shift, edit, device download/erase/relay, and GUI firmware write were not exercised on hardware.
- **GUI Log + OSM tiles on dev host (2026-11-23, NO M-241 attached)**: X11 drew 10,942-point dump + OSM tiles + attribution ✅ (dev host); Wayland cached 8 tiles ✅ (fetch only; appearance 未検証); headless/`--screenshot` fetched 0 tiles 🧪.

## 1a. Linux log-erase hardware session (2026-11-21)

Device: M-241 over CP210x. Binary: `bf41c2e` (`feat/log-erase-wire`, predates empty-log refusal `ea53d1a` and lost-stop-ACK resume).

| Step | Result | Status |
|---|---|---|
| Bare `$PMTK182,6` (pre-fix) | backup 225,028 B saved, failed; log untouched (225,060 B) | ✅ (negative) |
| `$PMTK182,5` → `$PMTK182,6,1*3E` → `$PMTK182,4` | backup 225,092 B saved, `erased: yes`, exit 0 | ✅ |
| Post-erase `log download` | 528 B (`0x210`), `records: 0`, `fixes: 0`, exit 0 | ✅ |
| Empty-log refusal & lost-stop-ACK resume | not re-run on hardware | ⚠️ |

## 2. Verification summary (open vs closed)

| Item | Status / Detail |
|---|---|
| Rust `fw dump` (chunked), `fw probe-da`, `fw flash` (strict & `--recover`), `0xD5` short final pkt (92+2), `0xD9` → `$PMTK180*3B` read-back, `fw da-restart` (from DA RAM), Linux USB | ✅ Measured 2026-11-21/22 (§1.2, §1.5) |
| `nmea proxy` stdout & loopback TCP | ✅ 9 sentences valid (§1.5); ⚠️ long runs, multi-client, non-loopback, live fix 未検証 |
| `--allow-vendor` write & boot | ⚠️ Written & booted stock V1.13, but read-back failed at `0x5C` (firmware restores `0x5C`–`0x5F` ✅) |
| OLED after flash / post-flash log timestamps / live GPS fix date | ⚠️ 未検証 |
| GUI device paths (download, erase backup, relay, firmware write), geotag, KMZ, shift, edit | ⚠️ 未検証 on hardware (Connect read-only probe ✅; Log/OSM X11/Wayland dev-host only) |
| `device doctor` on unit / `log geotag` on real camera JPEG | ⚠️ 未検証 (synthetic JPEGs only) |
| macOS / Bluetooth SPP / 4 MiB / GR-245 / V1.11 / V1.12 on hardware | ⚠️ 未検証 (V1.12 refused pre-port ✅; V1.12 offsets derived from vendor pkg only) |
| Automatic `0xD9` + 1 retry on `0xD3` failure / `0xD6` multi-page or non-64 page len / `0xA8` count / `0x01` meaning / 4 post-banner bytes origin / stock V1.13 or 2nd unit `probe-da` | ⚠️ 未検証 (fake or single-page only) |
| External viewers (Google Earth, `unzip`, GPX/NMEA tools) | ⚠️ 未検証 (KML 2.0 element order checked with `xmllint --schema` on 2026-11-23) |
| Timing figures | ⚠️ No stopwatch measurements |

## 3. The simulator

`m241-core` and `m241-flash` (`crates/m241-flash/src/simulator.rs`) include fake devices for **regression pinning** (`🧪`), not proof of hardware correctness (shared helpers like `swap_pairs` can be symmetrically wrong). Correctness rests on: (1) citations to `03-hardware-facts.md` / `01-da-protocol.md`, (2) hardware sessions in §1, and (3) `11-safety-review.md` + `tools/check_safety.py`.

## 4. How to close a gap

Run against real hardware → record command, verbatim output and date in §1 → update `⚠️` at the claim site. Never mark `✅` without all three.
