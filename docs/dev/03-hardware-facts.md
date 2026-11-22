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

The BootROM and the DA address the same flash differently. `0x00200000` is the
firmware base **in the BootROM's view**; in the DA's view that address does not
return the image, and the image begins at `0`. Do not carry an address from one
view to the other without converting it; the measured DA-view row is under
"Download Agent".

## Download Agent

| Fact | Value | How it was established |
|---|---|---|
| Info report | `>BHBLHHHHL`, 20 bytes, starts `0xC0` | DA v3.8.4 answers `C0 03 08 49 …` |
| DA version | `0x0308` | the report's second field |
| Flash on the retail unit | 512 KiB, MX29LV400CB | report: total-bytes field `0x80000`, region id `0x49`, mfr `0xC2`, device `0x22BA` |
| Firmware base, DA view | `0x00000000` | `fw probe-da --assume-brom` on 2026-11-21: a `0xD6` read at DA address `0` returns `18 F0 9F E5`, and its first 64 bytes equal the vendor V1.13 image's first 64. DA address `0x00200000` returns `02 02 20 42`, consistent with an unmapped bus. `docs/dev/05-verification.md` §1.2 |
| `0xD6` page, 64-byte argument | 64 data bytes + 2-byte big-endian checksum (66 bytes) | manual read on 2026-11-21: the trailer was `0x17FF`, equal to the sum of the 64 bytes. ⚠️ multi-page reads and the advance after the final page of a multi-page read are unmeasured |
| `0xD3` reply, accepted | two bytes: `5A 00` | reading one leaves the status byte in the buffer and shifts every later frame |
| `0xD3` reply, refused | **one** byte: `A5` | the failure path at `0x0010202C` skips both sends of the success path and reaches the shared tail call once; instruction sequence only, ⚠️ 未検証 — no refusal was ever seen on hardware |
| `0xD3` on refusal | records nothing from the refused command, but **keeps the ranges recorded by earlier ones** | the handler returns before overwriting the range table |
| WRITE16 echo | trails by one byte | after N bytes only N−1 echoes exist; the final byte's echo never arrives |
| Write sequence | `0xD3` records the range, `0xD5` erases and writes: `0x100`-byte packets each + sum-of-bytes → `0x69`, then two `0x5A` | the only sequence that completed on hardware |
| `0xD5` final packet's receive length | `(end - start + 1) mod length` — **92 bytes** for the 406,620-byte image; every earlier packet is `length` | `0x00102474` in `DA_v3.8.4.bin`: `r0` range index, `r1` packet index, `r2` packet count; returns the remainder only when `r1 == r2 - 1`. ⚠️ 未検証 on hardware — the 2026-11-22 runs failed on this packet — the instruction sequence is the source (`docs/reference/01-da-protocol.md` §4.3) |
| 2026-11-22 write failure | 1,588 of 1,589 packets accepted, then the final zero-padded packet answered `0x01` at 99% (`406,528` of `406,620` bytes) | `fw flash --allow-vendor` and `fw flash --recover` on Linux over USB serial, `docs/dev/05-verification.md` §1.4. The client then padded the final packet to `0x100`; ⚠️ what `0x01` means inside the DA is not established |
| DA jump (`0xA8`) | address `0x00100800`, **count `0`** | the only client that ran it on a unit sent zero (`python/src/m241/protocol/brom.py`, `BootRom.jump`). ⚠️ 未検証 whether the BootROM reads the count at all; a non-zero value has never been sent to a unit |

The 20-byte info report is `>BHBLHHHHL`, but the field name "block geometry" that
layout suggests is wrong: the four middle `H` fields are **not** flash geometry.

| Offset | Bytes | Meaning |
|---|---|---|
| `0x00` | `C0` | banner marker |
| `0x01`–`0x02` | `03 08` | DA version `0x0308` |
| `0x03` | `49` | region id |
| `0x04`–`0x07` | `00 08 00 00` | total bytes the DA reports, `0x80000` (512 KiB) — the only geometry in the banner |
| `0x08`–`0x09` | `00 C2` | region entry `+0x0E`: manufacturer id `0xC2` (MX29LV400CB) |
| `0x0A`–`0x0B` | `22 BA` | region entry `+0x10`: device id `0x22BA` |
| `0x0C`–`0x0D` | `00 00` | region entry `+0x12`: `0x0000` on the retail part, meaning not established |
| `0x0E`–`0x0F` | `00 00` | region entry `+0x14`: `0x0000` on the retail part, meaning not established |
| `0x10`–`0x13` | `?? ?? ?? ??` | u32 copied at run time from a RAM cell outside the DA image; **not statically determinable** |

The block geometry (`0x2000`, `0x10000`, `0x3F`…) lives in the sector descriptor
the region table entry points at and is never transmitted — see
`docs/reference/01-da-protocol.md` §2 and §6. Reading the four `H` fields as
size / block count was an error in earlier notes.

Because the trailing u32 comes from RAM, those four bytes cannot be predicted
from the image and must not be compared against a recorded constant. ⚠️ 未検証
what fills that cell. On the measured unit the report began
`C0 03 08 49 00 08 00 00 00 C2 22 BA 00 00 00 00`.

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
| Log erase command | `$PMTK182,6,1*3E` (after `$PMTK182,5`, before `$PMTK182,4`), ACK `$PMTK001,182,6,3` |
| Post-erase log usage (`$PMTK182,2,8`) | `528` bytes (`0x210`: fresh `0x200` block header + 16 B, 0 records / 0 fixes); bare `$PMTK182,6` without `,1` leaves the log untouched |

## Log format

A dump is a flat copy of the log area in 64 KiB blocks. Each block starts with a
`0x200`-byte header whose bitmask says which fields a record carries, followed by
the records and `0xFF` padding. The area is **not** a pure array of records: the
unit also writes a settings/state journal between record runs.

| Fact | Value | How it was established |
|---|---|---|
| Settings/state journal entry | 16 bytes: `AA`×7, type (1 byte), value (`u32` little-endian), `BB`×4 | runs of this exact template between record runs in the 2026-11-21 and both 2026-11-22 dumps |
| Journal name separator | 20 bytes of ASCII `HOLUXGR241LOGGER    ` (four trailing spaces) | 2026-11-21 blocks 0/2/3 and both 2026-11-22 dumps |
| Journal cluster | a name separator followed by six entries; one cluster is 116 bytes, and a block can start part-way into a cluster | 2026-11-21 block 2: two record runs (2,742 and 456 records) separated by a journal region holding nine name separators |
| Journal types seen | 3, 4 and 7 | the values look like the block header's period (`50` = 5.0 s) and policy (`0x0104` / `0x0106`) words; the mapping is **inference from the values, not firmware-confirmed** |
| Journal type 2 | ⚠️ 未検証 — suggested to overwrite the record mask, but never observed doing anything | not acted on anywhere in the parser |
| Header mask vs data | the mask can disagree with the actual record length in the block | 2026-11-21 block 0: header mask `0x1D` says 16-byte records, while all 3,244 records are 20 bytes and only the 20-byte grid checksums |
| Journal-only dump | a configured unit can hold a block header plus journal and no records at all (3,228 and 3,260 bytes) | both 2026-11-22 dumps; read as 20-byte records they produced 135 / 137 checksum-failing points |

Journal bytes are not track records. Reading them as records is what produced the
checksum-failing garbage points in the earlier parser. The parser recognises only
the exact shapes above and skips them; anything unrecognised fails the per-record
checksum and is dropped rather than guessed at. The type meanings are inference
from the observed values and must not be used as a configuration source.

## UI languages

English / French / German on the firmware; the release notes list TC / SC / DE /
FR / EN. No Japanese. The string table is byte-identical between V1.12 and V1.13.
