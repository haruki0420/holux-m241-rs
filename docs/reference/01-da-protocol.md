# Download Agent protocol

**Status: write path and command dispatch are determined from `DA_v3.8.4.bin` (14,896 bytes, `0x00100800`–`0x0010422F`); read-back and baud paths are determined in shape but not fully measured on hardware.**

Authority for `crates/m241-flash/src/da.rs` (kept out of read-only `m241-core`). Every constant and command in the driver must cite a function and instruction range on this page.

## 1. Transport

Plain byte stream over the BootROM UART: **no framing, no length prefix, no escaping, no framing checksum**. Argument and reply widths are implied by the opcode, except `0xD3` and `0xD5` which vary reply width on failure. Consuming the exact reply width is mandatory; off-by-one reads desynchronise all later commands.

**All multi-byte integers (including payload checksum trailers) are big-endian**, established in `FUN_00102EAC` / `FUN_00102EE4` (`u32`, shifts `24 - 8*n` at `0x00102EB4`–`0x00102EC8`, `0x00102EF4`–`0x00102EFC`) and `FUN_00102E34` / `FUN_00102E74` (`u16`, shift base `8` at `0x00102E3C`, `0x00102E78`).

| Byte | Meaning |
|---|---|
| `0x5A` | `ACK` — command accepted |
| `0xA5` | `NAK` — refused or checksum failed |
| `0x69` | `CONT` — write packet received and programmed |
| `0xBB` | unknown opcode |
| `0xC0` | DA banner start; `0xD2` handshake terminator |
| `0x7D` | sent once while waiting to start |
| `0x04` | progress frame prefix (followed by percentage byte) |

## 2. How the DA starts

Reset vector `0x00100800`–`0x00100828` sets SP to `0x001007FC` and branches to `0x00101718` (UART init → handshake `FUN_001015CC` → command loop `FUN_00102B38`).

Command-loop prologue (`0x00102C50`) calls `FUN_00101C00` to emit the 20-byte `>BHBLHHHHL` banner (`docs/dev/03-hardware-facts.md`):

| Field | Source | Retail value |
|---|---|---|
| `0xC0 0x03 0x08` | `0x00102C38`, `0x00102C40`, `0x00102C48` | `C0 03 08` |
| region id (`u8`) | `0x00101C0C`–`0x00101C18`, entry `+0x00` | `0x49` |
| total bytes (`u32`) | `FUN_00100B70` → `0x00101C38`–`0x00101C3C` | `0x00080000` |
| four `u16` | `FUN_00102E74` ×4, `0x00101C40`–`0x00101C8C`, entry `+0x0E/+0x10/+0x12/+0x14` (`ldrh` at `0x00101C4C`/`0x00101C60`/`0x00101C74`/`0x00101C88`) | `0x00C2` (mfr), `0x22BA` (MX29LV400CB), `0x0000`, `0x0000` (ids, not geometry) |
| final `u32` | `0x00101C90`–`0x00101C98`, RAM `0x00104330 + 0x10` | runtime value (outside image) |

## 3. Command dispatch

`FUN_00102B38` (`0x00102B38`–`0x00102D18`) sets defaults (`0x00102B48`–`0x00102BA4`: `+0=2`, `+1=4`, `+2=8`, `+3=0x0E`, `+0x0C=0x400`), reads an opcode (`0x00102C60` `bl 0x00102D64`), checks `0xD0..=0xDB` (`0x00102C64`–`0x00102C6C`), and jumps via the 12-word table `0x00102C74`–`0x00102CA0`. Any other opcode falls through to `0x00102D10` and sends `0xBB`.

### 3.1 The DA's two state blocks

| Literal | Address | Block | Fields |
|---|---|---|---|
| `0x0010282C` | `0x00104208` | **command state** | `+0` mode, `+1` baud index, `+2` status, `+3` driver status, `+4` 16-bit running checksum (`FUN_001019F0`/`FUN_00101F08`), `+6` retry count, `+0x0C` packet length, `+0x14` UART handle |
| `0x00102830` | `0x00104348` | **range state** | `+0` range count, `+4 + 8·i` `(start, end)` pairs, `+0x194` `0xD3` status byte, `+0x1A4`…`+0x1C8` receive ring & cursors |

| Opcode | Handler | Range | Args | Reply |
|---|---|---|---|---|
| `0xD0` | `0x00102CA4` → `FUN_00102A18` | `0x00102A18`–`0x00102A48` | 1 | 1 |
| `0xD1` | `0x00102CAC` (inline) | `0x00102CAC`–`0x00102CB8` | 1 | 1 |
| `0xD2` | `0x00102CCC` → `FUN_00101B38` | `0x00101B38`–`0x00101BFC` | 2 + handshake | 1 + handshake |
| `0xD3` | `0x00102CD4` → `FUN_00101F98` | `0x00101F98`–`0x00102034` | 1 + 8·n | **2 on success (`5A 00`), 1 on failure (`A5`)** |
| `0xD4` | `0x00102CDC` → `FUN_001029EC` | `0x001029EC`–`0x00102A14` | 8 | 1 (+ `0x04 <pct>` progress frames) |
| `0xD5` | `0x00102CEC` → `FUN_001024B8` | `0x001024B8`–`0x001028E4` | 4 + payload | 2, then 1/pkt (`0x69`), then 2 (`5A 5A`) |
| `0xD6` | `0x00102CE4` → `FUN_00101E78` | `0x00101E78`–`0x00101F04` | 12 | page + 2 B sum per page; host replies 1 B |
| `0xD7` | `0x00102CBC` → `FUN_00102A4C` | `0x00102A4C`–`0x00102A68` | 4 | 1 |
| `0xD8` | `0x00102CC4` → `FUN_00102A6C` | `0x00102A6C`–`0x00102A88` | 4 | 3 |
| `0xD9` | `0x00102C98` → `0x00102C5C` | `0x00101920`–`0x00101944` | **0** | **none** |
| `0xDA` | `0x00102CF4` (inline) | `0x00102CF4`–`0x00102D04` | 0 | 3 |
| `0xDB` | `0x00102D08` → `FUN_00102A9C` | `0x00102A9C`–`0x00102B08` | 2 | 1, then infinite loop |
| other | `0x00102D10` | `0x00102D10`–`0x00102D14` | 0 | 1 (`0xBB`) |

### 3.2 The BootROM's `0xA8` jump frame

BootROM command (9 bytes): opcode `0xA8`, BE `u32` address (`0x00100800`), BE `u32` count. The count's meaning is unestablished; `crates/m241-flash/src/da.rs` sends `DA_JUMP_COUNT = 0` matching the prior hardware-tested client (`docs/dev/03-hardware-facts.md`). ⚠️ 未検証 whether the BootROM reads the field.

## 4. The six questions

### 4.1 What does the `0xD3` status byte mean?

**Always `0x00`, sent only on success.** `FUN_00101F98` clears `range_state + 0x194` (`0x00102004`, `0x00102018`), sends `0x5A` (`0x0010201C`–`0x00102020`), then sends `[r7, #0x194]` = `0x00` (`0x00102024`–`0x00102034`). Failure jumps to `0x0010202C` and sends **a single `0xA5`**. Internal error codes at `comm + 2`/`+3` are never transmitted. Closes `docs/dev/06-open-questions.md` §3.

### 4.2 Is `0xD3`'s range in sectors or in bytes?

**Bytes, inclusive `(start, end)`, big-endian, parity-constrained.** `FUN_00101F98` reads count `n` (`0x00101F9C`, masked `& 0xFF` at `0x00101FF4`, max 255 ranges) and `n` BE `u32` pairs (`0x00101FB0`, `0x00101FB8`). Acceptance requires:

1. `FUN_00100A34(start, end - start + 1) != 0` (`0x00101FCC`, entire span inside region table `0x00103B38`);
2. `start` even (`0x00101FD8`–`0x00101FDC`);
3. `end` odd (`0x00101FE0`–`0x00101FE4`).

`0xD3` only records ranges and resets iterators; **it does not touch flash** (§4.6).

### 4.3 What is the maximum packet length?

**No hard limit in the DA** (`0x100` is a host convention; DA default `0x400` at `0x00102BA4` is only an init value). `0xD5` stores BE `u32` length at `comm + 0x0C` (`0x001024D8`–`0x001024DC`) and only declines when the ring (`comm + 0x1A4..+0x1BC`) cannot hold `length - 1` (`0x001025B4`–`0x001025C8`). Length must be even.

**Final packet receive length is the remainder, not `length`.** `0x00102474(range_idx, pkt_idx, pkt_count)` returns `(end - start + 1) mod length` when `pkt_idx == pkt_count - 1`, and `length` otherwise. For 406,620 bytes at `0x100`, packet 1,589 is **92 bytes + 2-byte sum** (must **not** be zero-padded; zero-padding caused the 2026-11-22 failure returning `0x01`, `docs/dev/05-verification.md` §1.4). ⚠️ Behaviour of `0x00102474` when remainder is 0 and meaning of `0x01` are unestablished.

### 4.4 Does the DA verify the sum-of-bytes, or only echo it?

**Verifies twice:**

1. **Per packet (`FUN_001019F0`, `0x001019F0`–`0x00101B08`)**: sums the bytes received for that packet (using `0x00102474`'s length) and compares with the BE `u16` trailer (`0x00101AE4`–`0x00101AF8`). Mismatch resets ring and never sends `0x69` (falls into retry → `0xA5` + restart; ⚠️ hardware returned `0x01` on padded final packet, §7 item 8).
2. **Per transfer (`FUN_00101F08`, `0x00101F08`–`0x00101F90`)**: re-reads flash via driver `vtable + 0x30` (`0x0010100C`), sums, and compares with `command_state + 4` (`0x00101F80`–`0x00101F90`). Mismatch sends `0xA5` (`0x001028A0`–`0x001028B8`).

`FUN_00101944` (`0x00101944`–`0x001019EC`) serialises read pages: half-words emitted low byte first (`0x0010198C`–`0x00101994`), zero-padded to `comm + 0x0C` (`0x001019C0`–`0x001019D8`), then 16-bit byte sum BE (`0x0010199C`–`0x001019A4`, `0x001019DC`–`0x001019EC`).

### 4.5 What are the argument forms of `0xD2` and `0xD9`?

- **`0xD2` (`FUN_00101B38`, `0x00101B38`–`0x00101BFC`)**: 2 bytes — baud index `1..=13` (`0x00101B54`–`0x00101B58`, table `0x00102FF8`; others return `0xA5`) and echo count. On accept: sends `0x5A`, changes UART rate, echoes host bytes until `0xC0` (`0x00101B80`–`0x00101B8C`), waits for `0x5A` (`0x00101B9C`), sends final `0x5A` (`0x00101BA0`). Non-zero echo count runs link-test passes (mismatch restarts at `0x00101BC0`–`0x00101BCC`).
- **`0xD9` (`0x00102C98` → `0x00102C5C` → `FUN_00101920`, `0x00101920`–`0x00101944`)**: **0 args, 0 reply**. Writes watchdog/reset control (`0xFFFFBFFF`, `0x1234`, `0x5678`) and loops forever (`0x00101938`).

### 4.6 What happens on a write to a protected or unmapped range?

1. **`0xD3` refuses unmapped spans** via `FUN_00100A34` (`0x00101FCC`) with a single `0xA5`, **leaving previously recorded ranges intact**. DA RAM (`0x00100800`) and BootROM stack (`0x00100000`–`0x001007FF`) are not in the region table and fail here.
2. **`0xD5` erases each sector before writing it**: `0x0010251C`–`0x00102540` calls driver sector erase (`FUN_00100D5C`); failure sends `0xA5` and restarts (`0x0010253C`–`0x0010254C`); success sends the second `0x5A` (`0x00102560`). Subsequent sector crossings erase at `0x0010268C`–`0x001026BC`.
3. **Driver failures** (`0`=OK, `3`=timeout, `4`=retry, `7`=program failure after 5 tries; `0x0010123C`, `0x00100D2C`) are stored at `comm + 3` and surface as `0xA5`.

Note: `0xD3` calls no flash driver; erasing happens solely in `0xD5` (or `0xD4`).

## 5. Opcode reference

### 5.1 `0xD0` — set range count
`FUN_00102A18` (`0x00102A18`–`0x00102A48`). Reads 1 byte; accepts `1..=4` into `comm + 0` (used by `0xD5` at `0x00102884`, default `2` at `0x00102B54`, overwritten by `0xD3`) and replies `0x5A`; else `0xA5`.

### 5.2 `0xD1` — consume one byte, acknowledge
`0x00102CAC`–`0x00102CB8`. Consumes 1 byte and replies `0x5A`. Purpose unproven (acts as alignment ping).

### 5.3 `0xD3` — record write range
`FUN_00101F98` (`0x00101F98`–`0x00102034`). Args: `n` (u8) + `n` BE `(start, end)` pairs. Reply: `5A 00` or single `A5`. Resets packet/range iterators at `comm + 0x1B0 / 0x1C4 / 0x1C8` (`0x00102008`–`0x00102018`). See §4.1, §4.2, §4.6.

### 5.4 `0xD4` — erase range with progress (**destructive**)
`FUN_001029EC` (`0x001029EC`–`0x00102A14`). Args: BE `u32 address`, `u32 length`. Calls `FUN_001023AC` (`0x001023AC`–`0x00102474`), erasing sector by sector (`FUN_00100D5C`), emitting `0x04 <percent>` frames (`0x00102370`–`0x0010237C`), and ending with `0x5A` or a 1-byte error code (`9` unresolved, `10` unmapped, or driver code). Not a checksum.

### 5.5 `0xD6` — read range
`FUN_00101E78` (`0x00101E78`–`0x00101F04`). Args: three BE `u32` — `page_len` (`comm + 0x0C`), `start`, `total_len`. Page count = `udiv(total + page - 1, page)` (`0x00101E9C`). Sends each page via `FUN_00101944` (zero-padded + BE `u16` sum), then waits for host `0x5A` (advance) or `0xA5` (retry) (`0x00101ED4`–`0x00101EF0`). Nothing sent after completion.

**Hardware measurement (2026-11-21, 1 page, `05-verification.md` §1.2):** `page_len = 64` at DA address `0` returned 64 data bytes + checksum `0x17FF` (66 bytes). 4 unexplained bytes appeared after the 20-byte banner before the first page (client drains before `0xD6`). ⚠️ Multi-page reads and post-final-page advance remain unmeasured.

### 5.6 `0xD5` — write
`FUN_001024B8` (`0x001024B8`–`0x001028E4`). Arg: BE `u32` packet length (`0x001024D8`).

| Step | Address | Direction | Bytes |
|---|---|---|---|
| length arg + driver init | `0x001024D8`, `0x00102500` | host → DA | 4 |
| first `ACK` | `0x00102504` | DA → host | `0x5A` |
| erase first sector | `0x0010251C`–`0x00102540` | internal | fail → `0xA5` + restart (`0x0010253C`–`0x0010254C`) |
| erase succeeded | `0x00102560` | DA → host | `0x5A` |
| per packet (`ceil((end-start+1)/len)`, `FUN_00101B0C`) | `0x001025E0`, `0x001019F0` | host → DA | `length` + 2 B sum (**last pkt**: remainder + 2, §4.3) |
| packet programmed (poll `0x001025E8`–`0x00102728`) | `0x00102734`–`0x00102738` | DA → host | `0x69` |
| transfer complete / final sum | `0x001027F8`, `0x00102898` | DA → host | `5A 5A` (fail → `0xA5` at `0x001028A0`–`0x001028B8`) |

Only ranges recorded by `0xD3` are written; extra packets get `0xA5`. After the last packet's trailer, read two `0x5A` bytes and send nothing in between (§7 item 1).

### 5.7 `0xD7` / 5.8 `0xD8` — DA RAM half-word access (not flash)
- **`0xD7`** (`FUN_00102A4C`, `0x00102A4C`–`0x00102A68`): BE `u32` addr → reads flash half-word (`FUN_00100DB0`), stores to `[addr]` in DA RAM (`0x00102A5C`), replies `0x5A`. Corrupts DA if given a flash address.
- **`0xD8`** (`FUN_00102A6C`, `0x00102A6C`–`0x00102A88`): BE `u32` addr → sends `[addr]` from DA RAM as BE `u16` + `0x5A` (3 bytes).

### 5.9 `0xDA` and `0xDB`
- **`0xDA`** (`0x00102CF4`–`0x00102D04`): sends `0x5A` + stale `r4` half-word (3 bytes). Undocumented.
- **`0xDB`** (`FUN_00102A9C`, `0x00102A9C`–`0x00102B08`): BE `u16`, compares baud divisor, sends `0x5A`, writes UART dividers (`DAT_0010281C`), enters infinite loop (`0x00102B08`).

## 6. Flash geometry

Region table at `0x00103B38`: 16 entries × `0x1C` bytes, terminated by `0xFF`. Entry fields: `+0x00` region id, `+0x04` sector descriptor ptr, `+0x08` ops ptr, `+0x0C` u32 (low u16 = descriptor count, high u16 = mfr id), `+0x0E..+0x14` four banner u16s (mfr, device, 0, 0), `+0x18` driver vtable ptr.

Retail entry (`0x49`, `0x00103B8C`): desc `0x00103EA0` (`0x00080000` = 512 KiB, sector sizes `0x4000`/`0x2000`/`0x8000`…), ops `0x00103CF8`, `0x00C20001`, device `0x22BA`, vtable `0x00103D64` (`+0x08` erase, `+0x18` program half-word, `+0x30` byte read, `+0x38` firmware base `0x00200000`). Erase uses JEDEC `AA 55 80 AA 55 30` (`0x001011D0`–`0x00101238`); program uses `AA 55 A0` (`0x001013E4`+).

## 7. Unresolved

1. **Inter-`0x5A` byte at end of `0xD5`**: `0x001027F8` is followed by a `0x69` receive (`0x00102800`–`0x00102808`), unreached on single-range hardware runs (client sends nothing, reads two `0x5A`). Branch rule unreduced.
2. **`0xD1` purpose**: handler decoded, protocol reason unknown.
3. **`0xDA` half-word**: stale `r4`; no setter found.
4. **`0xD6` multi-page / other page lengths**: only 1×64 B page measured (`05-verification.md` §1.2).
5. **Per-range host checksum**: none found (`0xD4` is erase).
6. **Non-`0x49` region selection**: 16 entries (`0x46`–`0x54`) exist; only `0x49` observed.
7. **`0xD5` per-packet retry**: unmeasured whether a retry can emit `0x69` twice.
8. **Meaning of `0x01` in `0xD5` reply**: observed on zero-padded final packet (`05-verification.md` §1.4) where §4.4 predicts `0xA5`. Must be reported as failure, never retried.

## 8. Hazards for the implementation

1. `0xD5` erases per sector before writing — a range covering `0x00000000` erases the reset vector.
2. `0xD3` reply is 2 bytes (`5A 00`) on accept, **1 byte (`A5`)** on refusal (`AGENTS.md` §4.1).
3. Failed `0xD3` leaves prior ranges intact.
4. `0xD9` never replies.
5. `0xD4` is destructive and interleaves `0x04 <pct>` progress frames.
6. `0xD7`/`0xD8` access DA RAM, not flash.
7. Payload checksum is BE 16-bit byte sum.
8. `0xD3` `end` is an inclusive address, not a length.
9. Final `0xD5` packet must be sent short (`(end-start+1) mod length`, 92 B for retail V1.13), never zero-padded.

## 9. Method

Disassembled `DA_v3.8.4.bin` (14,896 bytes, `0x00100800`–`0x0010422F`, ARM LE), traced `0x00102C74` jump table and handlers, derived endianness from `FUN_00102E34`/`0x00102E74`/`0x00102EAC`/`0x00102EE4`, and cross-checked against `docs/dev/03-hardware-facts.md`. Working notes stay uncommitted in `_scratch/`.

## 10. Corrections to earlier notes

- Entire DA protocol is big-endian; `0xD3` does not erase (`0xD5` does); `0xD3` status is always `0x00` on accept and omitted on `A5`; `0xD3` `end` is an address, not a length.
- `0xD4` is erase with progress, not checksum; `0xD6` takes 3 args; `0xD8` replies 3 bytes; `0xD2` takes baud index + echo count followed by `0xC0`/`0x5A` handshake.
- Banner's 4 middle `u16`s are mfr/device ids (`0x00C2`/`0x22BA`), not geometry; entry `+0x0C` low half is descriptor count.
- Final `0xD5` packet is read at remainder length (`0x00102474`), never zero-padded.
