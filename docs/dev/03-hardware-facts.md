# Hardware facts, measured once

Authority for constants in `crates/m241-core/src/protocol/`. Update here whenever a new measurement moves a constant.

## BootROM

| Fact | Value | Established by |
|---|---|---|
| Firmware base (BootROM view) | `0x00200000` | `0x20000000` → `42 20 02 02` (unmapped); `0x00200000` → `18 f0 9f e5 …` (DA view is `0`; convert between views) |
| DA load address | `0x00100800` | `0x100000`–`0x1007FF` is BootROM stack (writing kills chip); `push 0x100800` at `HoluxLoggerUtility.exe+0xB7B0` |
| Sync sequence (back-to-back, no pause) | `0xA0`→`0x5F`, `0x0A`→`0xF5`, `0x50`→`0xAF`, `0x05`→`0xFA` | 36 `0xA0` probes before first `0x5F` |
| Count units / reads | words (`0xA1`/`0xA2`/`0xA4`), dwords (`0xAF`); no checksum trailer | `0xA2`/`0xAF` return echo + data only; `0xA4` is separate |

## Download Agent

| Fact | Value | Established by |
|---|---|---|
| Info report | `>BHBLHHHHL` (20 B): `C0 03 08 49 00 08 00 00 00 C2 22 BA 00 00 00 00 ?? ?? ?? ??` | DA v3.8.4 (`0x0308`), region `0x49`, 512 KiB MX29LV400CB (`0x00C2`/`0x22BA`, ids not geometry), trailing `u32` from RAM (not predictable, ⚠️ 未検証 what fills it). See `01-da-protocol.md` §2/§6 |
| Firmware base (DA view) | `0x00000000` | `fw probe-da` 2026-11-21 (`05-verification.md` §1.2): `0` → `18 F0 9F E5` (matches V1.13 first 64 B); `0x00200000` → `02 02 20 42` |
| `0xD6` page (64 B arg) | 64 data B + 2 B BE sum (66 B; `0x17FF` at addr `0`) | 2026-11-21 single page; ⚠️ multi-page / advance unmeasured |
| `0xD3` reply | `5A 00` (accept, 2 B) / `A5` (refuse, **1 B**, keeps prior ranges) | Accept measured; refusal from `0x0010202C` only (⚠️ 未検証 on hardware) |
| WRITE16 echo | trails by 1 byte (`N−1` echoes for `N` bytes) | measured |
| Write sequence (`0xD3` + `0xD5`) | `0x100` B pkts + BE u16 sum → `0x69`, final pkt `(end-start+1) mod len` (**92 B** + 2 B sum for V1.13), then `5A 5A` | Disassembly `0x00102474` (`01-da-protocol.md` §4.3) & 2026-11-22 runs (`05-verification.md` §1.4–§1.5). Zero-padded final pkt returned `0x01` at pkt 1,588 (⚠️ meaning of `0x01` unknown) |
| DA jump (`0xA8`) | addr `0x00100800`, count `0` | Prior client sent `0` (⚠️ 未検証 whether BootROM reads count) |

## Firmware image & serial

| Fact | Value |
|---|---|
| V1.13 / V1.12 size & SHA-256 | V1.13: 406,620 B (`0x6345C`), `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435` · V1.12: 406,312 B (`0x63328`), `4103601a29f256ae5da771bbcbc3ca85a89deb6268ce74865dc84b2583ae98f7` |
| GPS epoch & patch | LE32 `315964800` at `0x402DC` (unused) & `0x402F4` (effective) → `935280000` (`+1024×604800`); changes `0x402F6`, `0x402F7`, `0x59C2C` (V1.13 marker `0x59C28`; V1.12 `0x59B14`) |
| Serial & NMEA | NMEA 38400 8N1 (`$GPGGA/$GPGSA/$GPGSV/$GPRMC/$GPVTG` @ 1 Hz); BootROM 115200 |
| Log PMTK182 | `0x800` B chunks; erase = `$PMTK182,5` → `$PMTK182,6,1*3E` (ACK `$PMTK001,182,6,3`) → `$PMTK182,4`; post-erase usage = `528` B (`0x210`, 0 records; bare `$PMTK182,6` does nothing) |
| UI languages | EN / FR / DE (release notes TC/SC/DE/FR/EN; no Japanese; V1.12/V1.13 string table identical) |

## Log format

64 KiB blocks: `0x200`-byte header (bitmask selects record fields) + records + `0xFF` padding, interleaved with a **settings/state journal**:
- **Journal entry (16 B)**: `AA`×7, 1 B type, LE `u32` value, `BB`×4. **Name separator (20 B)**: ASCII `HOLUXGR241LOGGER    ` (4 trailing spaces). Cluster = 1 separator + 6 entries (116 B).
- **Observed types** (2026-11-21 225,028 B dump: 214 entries + 34 separators = 248 items): types 3 (×68), 4 (×68), 7 (×75), 6 (×2: `0x0104`/`0x0106`), 2 (×1 at `0x220`: `0x8800_003D`). Meanings are inference only, not firmware-confirmed.
- **Stale header mask**: block 0 header `0x1D` declared 16 B records while its 3,244 records were 20 B. Parser takes boundaries from checksum chains (2 consecutive at known start, 4 for resync, or 1 before journal/padding; 50 random 64 KiB seeds yield 0 points) and field meanings from the mask (fields outside detected length are skipped; 7,698 points in blocks 1–3 carry speed).
- **Journal-only dump**: configured unit with no points (2026-11-22: 3,228 / 3,260 B, 0 points).
- **Satellite / DOP fields**: `crates/m241-core/src/log/block.rs` defines `SID`/`NSAT`/`PDOP`/`HDOP`/`VDOP`, but measured masks (`0x1D`/`0x3D`) select none and `crates/m241-core/src/log/record.rs` stores only UTC/lat/lon/height/speed/rcr. Exported `$GPGGA`/`$GPGSA` leave satellite count, satellite list, DOPs, and geoid separation **empty** (never `00`), and GSA fix type (2/3) is inferred from altitude presence.
