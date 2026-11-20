# Hardware facts, measured once

Every constant in `crates/m241-core/src/protocol/` traces to a line on this page.
If you change a constant, change the line, and say what new measurement moved it.

## BootROM

| Fact | Value | How it was established |
|---|---|---|
| Firmware base, BootROM view | `0x00200000` | `0x20000000` reads back `42 20 02 02` (unmapped bus); `0x00200000` returns the vector table `18 f0 9f e5 …` |
| DA load address | `0x00100800` | `0x100000`–`0x1007FF` is the BootROM's own stack; writing there kills the chip. `push 0x100800` appears exactly once in `HoluxLoggerUtility.exe` at file offset `0xB7B0` |
| Sync sequence | `0xA0`→`0x5F`, `0x0A`→`0xF5`, `0x50`→`0xAF`, `0x05`→`0xFA` | 36 probes of `0xA0` before the first `0x5F` on a freshly reset unit |
| Probes must be back to back | — | inserting any delay drops the chip out of the handshake |
| Count units | words for `0xA1`/`0xA2`/`0xA4`, dwords for `0xAF` | sending a byte count returns the wrong amount of data |
| Reads carry no trailer | — | `0xA2`/`0xAF` return echo + data; consuming a checksum trailer desynchronises every read |

## Download Agent

| Fact | Value | How it was established |
|---|---|---|
| Info report | `>BHBLHHHHL`, 20 bytes, starts `0xC0` | DA v3.8.4 answers `C0 03 08 49 …` |
| DA version | `0x0308` | the report's second field |
| Flash on the retail unit | 512 KiB, MX29LV400CB | report: `flash_size = 0x80000`, id `0x49`, mfr `0xC2`, device `0x22BA` |
| `0xD3` reply | two bytes: ACK plus status | reading one leaves the status byte in the buffer and shifts every later frame |
| WRITE16 echo | trails by one byte | after N bytes only N−1 echoes exist; the final byte's echo never arrives |
| Write sequence | `0xD3` range, `0xD5` write, `0x100`-byte packets each + sum-of-bytes → `0x69`, then two `0x5A` | the only sequence that completed on hardware |

## Firmware image

| Fact | Value |
|---|---|
| V1.13 size | 406,620 bytes (`0x6345C`) |
| V1.13 SHA-256 | `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435` |
| V1.12 size | 406,312 bytes (`0x63328`) |
| V1.12 SHA-256 | `4103601a29f256ae5da771bbcbc3ca85a89deb6268ce74865dc84b2583ae98f7` |
| GPS epoch constant | LE32 `315964800` at `0x402DC` (inactive) and `0x402F4` (effective) |
| Patched value | `935280000` = `315964800 + 1024 × 604800` |
| Bytes that change | `0x402F6`, `0x402F7` (epoch), `0x59C2C` (version marker) |
| Version marker | ASCII `V1.13` at `0x59C28` (V1.12: `0x59B14`) |

## Serial

| Fact | Value |
|---|---|
| NMEA baud | 38400 8N1 |
| BootROM baud | 115200 |
| Sentences at 1 Hz | `$GPGGA`, `$GPGSA`, `$GPGSV`, `$GPRMC`, `$GPVTG` |
| Log chunk accepted by the device | `0x800` bytes |

## UI languages

English / French / German on the firmware; the release notes list TC / SC / DE /
FR / EN. No Japanese. The string table is byte-identical between V1.12 and V1.13.
