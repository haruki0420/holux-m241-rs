# Download Agent protocol

**Status: the write path and the command dispatch are fully determined; the
read-back and baud paths are determined in shape but not measured on hardware.**
This page is the authority that `crates/m241-core/src/protocol/da.rs` cites.
Every constant in the driver must point at a row of the opcode table below,
which in turn gives the address and the instruction range that produce the
behaviour.

## Why this page exists before the code

Two prior implementations of this repair disagree about the DA's command
semantics, and both were written against the same incomplete picture. Porting
either would carry an unknown number of wrong assumptions into a path that can
destroy the device.

The rule this project follows: **every constant and every command in the driver
cites a function and an instruction range on this page.** If the citation cannot
be written, the code cannot be written.

## 1. Transport

The DA is a plain byte stream over the same UART the BootROM used: **no framing,
no length prefix, no escaping, no framing checksum**. A command is an opcode
followed by a fixed number of argument bytes whose width is implied by the
opcode; a reply is a fixed number of bytes whose width is implied by the opcode
that was sent — with two documented exceptions (`0xD3` and `0xD5`, which send a
different number of bytes on the failure path).

Consequently **a client must consume exactly the reply width for each opcode.**
Reading one byte too few leaves the remainder in the buffer and shifts every
later frame by that many bytes; reading one byte too many consumes the start of
the next reply. This is the failure mode that made `0xD3` look like a broken
`0xD5`.

**All integers on the wire are big-endian**, matching the BootROM. This is
established at instruction level in `FUN_00102EAC` (receive `u32`) and
`FUN_00102EE4` (transmit `u32`):

```
0x00102EB4   mov  r12, #24
0x00102EC4   add  r1, r12, (-r2) << 3     ; shift = 24 - 8*n
0x00102EC8   orr  r3, r3, r0, lsl r1      ; first byte received becomes bits 31..24
```

and symmetrically `0x00102EF4`–`0x00102EFC` shifts by `24 - 8*n` on the way out,
so the most significant byte is sent first. The 16-bit pair
(`FUN_00102E34` / `FUN_00102E74`) uses shift base `8` (`0x00102E3C`,
`0x00102E78`). **Every multi-byte field in this protocol, including the payload
checksum trailer, is big-endian.**

Bytes used as text markers:

| Byte | Meaning |
|---|---|
| `0x5A` | `ACK` — the command was accepted |
| `0xA5` | `NAK` — the command was refused, or a checksum failed |
| `0x69` | `CONT` — one write packet was received and programmed |
| `0xBB` | unknown opcode |
| `0xC0` | start of the DA banner; also the `0xD2` handshake terminator |
| `0x7D` | sent once while the DA is waiting to be started |
| `0x04` | progress frame prefix (followed by a percentage byte) |

## 2. How the DA starts

Reset vector `0x00100800`–`0x00100828`: sets the stack pointer below the image
(`0x001007FC`), loads a word from the literal pool and branches to it. The
target is `0x00101718`, which configures the UART, calls the start-up handshake
`FUN_001015CC`, and falls through into `FUN_00102B38`, the command loop.

The banner is emitted by `FUN_00101C00`, called from the command-loop prologue
at `0x00102C50`. It is the `0xC0`-prefixed 20-byte record already recorded in
`docs/dev/03-hardware-facts.md`:

```
C0  03 08  49         three putc calls in the loop prologue, then the region id
00 08 00 00           total bytes, BE u32, from FUN_00100B70
00 C2                 BE u16, region entry +0x0E   (manufacturer id)
22 BA                 BE u16, region entry +0x10   (device id)
00 00                 BE u16, region entry +0x12
00 00                 BE u16, region entry +0x14
?? ?? ?? ??           BE u32, runtime cell at 0x00104330 + 0x10
```

That is 20 bytes, matching the `>BHBLHHHHL` layout recorded in
`docs/dev/03-hardware-facts.md` (`B` = `0xC0`, `H` = `0x0308`, `B` = region id,
`L` = total bytes, four `H`, `L` = the runtime word).

The four u16 words are **not** geometry despite the layout name: the retail
part's entry yields `0x00C2` (Macronix manufacturer id) and `0x22BA` (the
MX29LV400CB device id), then two zeros. This is the same manufacturer/device
pair the hardware table records for the retail unit, so the earlier reading of
these four fields as size/block geometry was wrong.

| Field | Source | Statically determined value |
|---|---|---|
| `0xC0`, `0x03`, `0x08` | `0x00102C38`, `0x00102C40`, `0x00102C48` | `C0 03 08` |
| region id | `0x00101C0C`–`0x00101C18`, region entry `+0x00` | `49` |
| total bytes | `FUN_00100B70` then `FUN_00102EE4` at `0x00101C38`–`0x00101C3C` | `0x00080000` |
| four u16 | `FUN_00102E74` ×4, `0x00101C40`–`0x00101C8C`, entry `+0x0E/+0x10/+0x12/+0x14` | `0x00C2 0x22BA 0x0000 0x0000` |
| final u32 | `FUN_00102EE4`, `0x00101C90`–`0x00101C98`, from RAM `0x00104330 + 0x10` | **not statically determinable** — the cell is outside the 14,896-byte image and is filled at run time |

The entry offsets are `+14`, `+16`, `+18`, `+20` in the `ldrh` at
`0x00101C4C`, `0x00101C60`, `0x00101C74`, `0x00101C88`, addressing
`region_table + index × 0x1C` — the region **entry**, not the sector descriptor
pointer at `+0x04`.

## 3. Command dispatch

The loop is `FUN_00102B38`, address range `0x00102B38`–`0x00102D18`.

The command byte is read at `0x00102C60` (`bl 0x00102D64`, the blocking
receive-byte primitive), then normalised and range-checked:

```
0x00102C64   sub  r0, r0, #0xD0
0x00102C68   cmp  r0, #0x0B
0x00102C6C   addls pc, pc, r0, lsl #2      ; 12-entry jump table
```

The table is the twelve words `0x00102C74`–`0x00102CA0`; each is a `b` to the
handler. Any opcode below `0xD0` or above `0xDB` falls through to `0x00102D10`,
which sends `0xBB` (`mov r0,#0xbb` at `0x00102D10`).

The loop prologue sets the DA's defaults at `0x00102B48`–`0x00102BA4`, on the
command-state block described in section 3.1: `+0` = `2`, `+1` = `4`, `+2` = `8`,
`+3` = `0x0E`, `+0x0C` (packet length) = `0x400`.

### 3.1 The DA's two state blocks

Two separate blocks sit in the DA's RAM, and mixing them up produces a
specification that looks self-consistent and is wrong. Every handler reaches
them through the literal pool, and the literal words make the split explicit:

| Literal word | Points to | Name in this page | What lives there |
|---|---|---|---|
| `0x0010282C` | `0x00104208` | **command state** | `+0` mode byte, `+1` baud index, `+2` status code, `+3` driver status, `+4` 16-bit running checksum, `+6` retry counter, `+0x0C` packet length, `+0x14` UART handle |
| `0x00102830` | `0x00104348` | **range state** | `+0` range count, `+4 + 8·i` the recorded `(start, end)` pairs, `+0x194` `0xD3` status byte, `+0x1A4`…`+0x1C8` the receive ring and its cursors |

The two blocks are never the same address, and each handler loads the one it
means. `0xD3` writes the range state (`0x00101FA0` loads `0x00102830`);
`0xD5` writes the command state for its status and length (`0x001024BC` loads
`0x0010282C`) while reading the range state for the recorded pairs and the ring
(`0x0010250C` loads `0x00102830`). The running checksum that `FUN_001019F0`
accumulates lands in the command state at `+4`, and `FUN_00101F08` compares
against exactly that cell (`0x00101F80`–`0x00101F84`).

The `0xD3` status byte at range-state `+0x194` is a different cell from the
`+2`/`+3` codes the write path fills in, which is why the `0xD3` reply can be a
constant while the write path has real error codes to hide.

| Opcode | Handler entry | Handler range | Argument bytes | Reply bytes |
|---|---|---|---|---|
| `0xD0` | `0x00102CA4` → `FUN_00102A18` | `0x00102A18`–`0x00102A48` | 1 | 1 |
| `0xD1` | `0x00102CAC` (inline) | `0x00102CAC`–`0x00102CB8` | 1 | 1 |
| `0xD2` | `0x00102CCC` → `FUN_00101B38` | `0x00101B38`–`0x00101BFC` | 2 + handshake | 1 + handshake |
| `0xD3` | `0x00102CD4` → `FUN_00101F98` | `0x00101F98`–`0x00102034` | 1 + 8·n | **2 on success, 1 on failure** |
| `0xD4` | `0x00102CDC` → `FUN_001029EC` | `0x001029EC`–`0x00102A14` | 8 | 1 (+ progress frames) |
| `0xD5` | `0x00102CEC` → `FUN_001024B8` | `0x001024B8`–`0x001028E4` | 4 + payload | 2, then 1 per packet, then 2 |
| `0xD6` | `0x00102CE4` → `FUN_00101E78` | `0x00101E78`–`0x00101F04` | 12 | one data page per page, host answers 1 byte |
| `0xD7` | `0x00102CBC` → `FUN_00102A4C` | `0x00102A4C`–`0x00102A68` | 4 | 1 |
| `0xD8` | `0x00102CC4` → `FUN_00102A6C` | `0x00102A6C`–`0x00102A88` | 4 | 3 |
| `0xD9` | `0x00102C98` → `0x00102C5C` | `0x00101920`–`0x00101944` | **0** | **none** |
| `0xDA` | `0x00102CF4` | `0x00102CF4`–`0x00102D04` | 0 | 3 |
| `0xDB` | `0x00102D08` → `FUN_00102A9C` | `0x00102A9C`–`0x00102B08` | 2 | 1, then silence |
| other | `0x00102D10` | `0x00102D10`–`0x00102D14` | 0 | 1 (`0xBB`) |

`0x00102C5C` is the shared "restart the chip" tail; `0xD9` branches to it
directly, which is why `0xD9` has no handler of its own.

## 4. The six questions

### 4.1 What does the `0xD3` status byte mean?

**On the wire the status byte is always `0x00`, and it is only sent when the
command succeeded. There is no non-zero status value.** The "status" is a fixed
success marker, not a result code.

`FUN_00101F98` builds both replies and they are **not the same width**:

```
0x00102004   mov   r0, #0x00
0x00102008-18        reset the range/packet iterators
0x00102018   strb  r0, [r7, #0x194]      ; status cell := 0
0x0010201C   mov   r0, #0x5A
0x00102020   bl    0x00102DE0            ; byte 1: ACK
0x00102024   ldrb  r0, [r7, #0x194]      ; byte 2: the cell just cleared -> 0x00
0x00102028   b     0x00102030            ; -> pop, tail-call the sender
0x0010202C   mov   r0, #0xA5             ; failure path lands here instead
0x00102030   pop   {r4-r8,lr}
0x00102034   b     0x00102DE0            ; send one byte
```

So:

| Outcome | Bytes on the wire |
|---|---|
| accepted | `5A 00` (two bytes) |
| refused | `A5` (**one byte**) |

The failure path skips the second send, so a client that always reads two bytes
after `0xD3` will consume the first byte of the next reply after a refusal.
`comm + 0x194` is written only here, and only with zero, so the value is
provably constant.

Internally the DA does keep richer codes — `comm + 2` holds `2`, `5` or `6` and
`comm + 3` holds the flash driver's return value — but those cells are never
transmitted. A client can only distinguish "accepted" from "refused", and it
must use the reply *length* to do so.

This closes `docs/dev/06-open-questions.md` §3: the question was what a status
value other than `0x00` means; the answer is that no such value exists.

### 4.2 Is `0xD3`'s range in sectors or in bytes?

**Bytes, inclusive at both ends, big-endian, with a parity constraint.**

`FUN_00101F98` reads a count byte through `FUN_00102D64`, then for each entry two
`u32` values through `FUN_00102EAC`:

```
0x00101F9C   bl   0x00102D64            ; count n
0x00101FA8   strb r0, [r7, #0]          ; comm+0 := n
0x00101FB0   bl   0x00102EAC            ; start      (BE u32)
0x00101FB8   bl   0x00102EAC            ; end        (BE u32)
0x00101FC0   sub  r0, r5, r5            ; (start, end-start+1)
0x00101FCC   bl   0x00100A34            ; whole span mapped?
0x00101FD8   tst  r5, #1
0x00101FDC   bne  fail                 ; start must be even
0x00101FE0   tst  r6, #1
0x00101FE4   beq  fail                 ; end must be odd
```

The three acceptance conditions are:

1. `FUN_00100A34(start, end - start + 1)` is non-zero, i.e. the whole span lies
   inside regions listed in the region table at `0x00103B38`;
2. `start` is even;
3. `end` is odd.

The parity rule makes the range a whole number of 16-bit half-words, which is
what the flash driver programs. A range of one half-word at `0x00100000` is
`start = 0x00100000`, `end = 0x00100001`; the whole 512 KiB part is
`start = 0x00000000`, `end = 0x0007FFFF`. **The unit is bytes** — the parity
check is a constraint on a byte-valued pair, not a switch to sector or
half-word units. `FUN_00100A34` walks the region table entry by entry and
subtracts the covered span, so a range may legitimately cross a region boundary
as long as the next region continues it.

The count byte is masked with `and r4, r4, #0xFF` at `0x00101FF4`, so up to 255
ranges may be recorded in one command, each occupying 8 bytes at `comm + 4`.

`0xD3` records the ranges and resets the iterators. **It does not touch the
flash.** See 4.6.

### 4.3 What is the maximum packet length?

**There is no maximum in the DA. `0x100` is a host-side convention, not a
protocol limit, and it does not change with baud rate.**

`0xD5` reads its length as a BE `u32` (`0x001024D8`, `bl 0x00102EAC`) and stores
it at `comm + 0x0C` (`0x001024DC`). Nothing between `0x001024B8` and
`0x001028E4` compares that value against a bound. The receive loop counts down
from the length argument (`FUN_001019F0`, loop at `0x00101A78`:
`cmp r4, r10` with `r10` = length), and the only limit is free space in the DA's
own ring.

The ring bookkeeping is at `comm + 0x1A4` (packet start), `+0x1A8` (packet end),
`+0x1B4` (head), `+0x1B8` (tail), `+0x1BC` (buffer end). `0xD5` declines a
packet only when the ring cannot hold it:

```
0x001025B4-0x001025C8   if (length - 1) > (head + free) -> status 2, do not read
```

The default length the DA itself sets is `0x400` (`0x00102BA4`), which is an
internal initialisation value, not a contract. A client cannot derive a safe
maximum from the protocol and has to pick one.

Two things a client does depend on: a **short final packet is accepted** — the
length argument drives the per-packet checksum and the `0x69` reply, and the
hardware-tested write in `docs/dev/03-hardware-facts.md` ends on a 92-byte
packet — and the length must be even, because the payload is checksummed as
16-bit pairs.

### 4.4 Does the DA verify the sum-of-bytes, or only echo it?

**It verifies. Nothing is merely echoed.** There are two independent checks.

1. **Per packet, at receive time.** `FUN_001019F0` (`0x001019F0`–`0x00101B08`)
   accumulates the 16-bit byte sum of everything it received and compares it
   with the two-byte trailer that follows the payload:

   ```
   0x00101AE4   orr  r0, r0, r1, lsl #0      ; the trailer, assembled
   0x00101AE8   cmp  r0, r5                  ; against the accumulated sum
   0x00101AEC   moveq r0, #0x01              ; match
   0x00101AF8   movne r0, #0x01              ; mismatch: ring reset, no 0x69
   ```

   A mismatch never produces `0x69`; it falls into the write handler's retry
   logic, which ends in `0xA5` and a chip restart.

2. **Per transfer, before the final acknowledgement.** `FUN_00101F08`
   (`0x00101F08`–`0x00101F90`) re-reads the recorded ranges from the flash
   through the driver's byte-read entry (`vtable + 0x30` = `0x0010100C`), sums
   them, and compares against the accumulated value:

   ```
   0x00101F80   ldr   r0, [pc, #0x8A4]       ; accumulated sum
   0x00101F84   ldrh  r0, [r0]
   0x00101F88   cmp   r4, r0
   0x00101F8C   movne r0, #0
   0x00101F90   moveq r0, #1
   ```

   A zero result makes `0xD5` send `0xA5` (`0x001028A0`–`0x001028B8`).

The sum is a **16-bit sum of individual bytes, transmitted high byte first**
(that is, big-endian like every other field). `FUN_00101944`
(`0x00101944`–`0x001019EC`) is both the write path's page serialiser and the
read path's page reader, and it emits, in order:

1. the range's bytes, two at a time, taken from flash as a 16-bit word and
   emitted **low byte first** — so the data reproduces the image byte order
   (`0x0010198C` sends `value & 0xFF`, `0x00101990`–`0x00101994` sends
   `value >> 8`);
2. zero bytes until the page length reaches `comm + 0x0C`
   (`0x001019C0`–`0x001019D8`: pad while `comm + 0x0C - length > i`), so a short
   final page is **zero-padded to the full packet length**, not left short;
3. the 16-bit sum, **high byte first** (`0x001019DC` sends `sum >> 8`,
   `0x001019EC` tail-calls the sender with `sum & 0xFF`).

The sum itself is accumulated at `0x0010199C`–`0x001019A4` and folded to 16 bits
by clearing bits 16 and 17 (`bic r6, r0, #196608`, `0x001019A4`), which is
arithmetic modulo 65536 because the maximum possible value for a 1024-byte page
is below `0x40000`.

### 4.5 What are the argument forms of `0xD2` and `0xD9`?

**`0xD2` takes two bytes and is followed by a byte-level handshake; `0xD9` takes
no argument at all.**

`0xD2` — `FUN_00101B38`, `0x00101B38`–`0x00101BFC`:

```
0x00101B38   bl   0x00102D64        ; byte 1: baud index -> comm+1
0x00101B50   bl   0x00102D64        ; byte 2: echo count
0x00101B54   cmp  bVar1, #0
0x00101B58   cmpne bVar1, #0x0D     ; index must be 1..13
```

- Byte 1 selects a divider through `FUN_00102F20` from the table at
  `0x00102FF8`. **Indices 1 through 13 are accepted**; `0` and `14` or more are
  refused with `A5` and the link stays at the old rate.
- On acceptance the DA sends `0x5A`, reprograms the UART, then **echoes every
  byte the host sends until it sees `0xC0`**, then reads bytes until it sees
  `0x5A`, and then sends `0x5A`:

  ```
  0x00101B80   bl   0x00102D64        ; read
  0x00101B84   bl   0x00102DE0        ; echo it
  0x00101B8C   cmp  iVar5, #0xC0
  0x00101B9C   cmp  iVar5, #0x5A
  0x00101BA0   bl   0x00102DE0        ; final 0x5A
  ```

  Both terminators are mandatory: the host must send `0xC0` to end the echo
  phase and then a `0x5A` to confirm, after the rate has already changed.
- Byte 2 is an echo count for a link test. If it is non-zero the DA then runs
  that many passes of "receive a byte, expect it to equal the pass counter, echo
  it", restarting the chip (`0x00101BC0` → `0x00101BCC`) if a byte does not
  match. It is not part of the rate change.

`0xD9` — `0x00102C98` `b 0x00102C5C` → `FUN_00101920`
(`0x00101920`–`0x00101944`):

```
0x00101920   mov  r0, #0x4000
0x00101924   mvn  r0, r0                    ; 0xFFFFBFFF
0x00101928   strh r0, [control, #0x28]      ; watchdog / reset control
0x0010192C   strh ..., [control, #0x50]     ; 0x1234
0x00101930   strh ..., [control, #0x54]     ; 0x5678
0x00101938   b    .                         ; never returns
```

**It reads no argument bytes and sends no reply.** Any bytes the host sends
after `0xD9` go to a chip that has already restarted. A client must not wait for
an acknowledgement.

### 4.6 What happens on a write to a protected or unmapped range?

Four distinct outcomes, in the order the DA reaches them.

1. **`0xD3` refuses a range that is not wholly inside a mapped region.** The
   check is `FUN_00100A34(start, end-start+1)` at `0x00101FCC`, which walks the
   region table at `0x00103B38` (stride `0x1C`, terminated by a region id of
   `0xFF`) and returns 1 only if the entire span is covered. Unmapped or
   protected spans are refused with a single `A5` and nothing is recorded.
   **A refused `0xD3` leaves the previously recorded ranges in place**, so a
   client must not treat it as "cleared".

2. **`0xD5` erases the sector before it writes.** This is on the `0xD5` path, not
   the `0xD3` path: `0x0010251C` resolves the address, `0x0010252C` calls the
   driver's sector erase (`FUN_00100D5C`), and a non-zero return sends `0xA5`,
   delays and restarts the chip (`0x0010253C`–`0x0010254C`). The second `0x5A`
   is sent only after that erase succeeded (`0x00102560`). Every later packet
   that crosses into a new sector triggers the same erase before its data is
   programmed (`0x0010268C`–`0x001026BC`).

3. **A sector the chip will not erase or program** reaches the driver, which
   reports a status the DA stores at `comm + 3` and turns into `0xA5`. The
   driver's own codes are `0` = OK, `3` = timeout / no toggle, `4` = retry, `7`
   = program failure after five attempts (status poll at `0x0010123C`; the
   retry counter giving `7` is in `0x00100D2C`). None of these reach the wire.

4. **The DA's own image and the BootROM scratch are unreachable through `0xD3`.**
   The region table lists only flash regions, so `0x00100800` (the DA) and
   `0x00100000`–`0x001007FF` (BootROM stack) have no entry and any range
   covering them fails check 1.

**This corrects a claim repeated in earlier notes.** `docs/dev/03-hardware-facts.md`
and `AGENTS.md` §2.2 say `0xD3` erases the range it covers. It does not. The
`0xD3` handler contains no call to the flash driver at all — its only calls are
`0x00102D64`, `0x00102EAC` (twice), `0x00100A34` and `0x00102DE0`. The erase is
in `0xD5`, per sector, immediately before that sector is written. The safety
warning stands unchanged, because `0xD3` followed by `0xD5` still erases every
sector the range touches, and a range covering `0x00000000` still destroys the
reset vector — but nothing is erased at `0xD3` time, and a `0xD3` that is never
followed by a `0xD5` changes no flash.

## 5. Opcode reference

### 5.1 `0xD0` — set the range count

`FUN_00102A18`, `0x00102A18`–`0x00102A48`. Reads one byte; accepts `1`, `2`,
`3`, `4`; stores it at `comm + 0` and replies `0x5A`. Anything else replies
`0xA5` and leaves the value unchanged. `comm + 0` is the loop bound `0xD5` uses
to walk the range table (`0x00102884`: `ldrb r0,[r10,#0]; cmp r9,r0`), so this
sets how many recorded ranges `0xD5` will write. `0xD3` overwrites the same cell
with its own count, and the DA's default is `2` (`0x00102B54`).

### 5.2 `0xD1` — consume one byte, acknowledge

`0x00102CAC`–`0x00102CB8`: consume one byte, `mov r0,#0x5A`, send it, loop. One
argument byte, one reply byte, no other effect. Its purpose is not proven; it
behaves as a deliberate one-byte ping that lets the host confirm the link is
byte-aligned.

### 5.3 `0xD3` — record the write range

`FUN_00101F98`, `0x00101F98`–`0x00102034`. Arguments: one count byte `n`, then
`n` pairs of big-endian `u32` `(start, end)`. Reply: `5A 00` on success, `A5`
(single byte) on failure. See 4.1, 4.2 and 4.6.

On success the range count is stored at `comm + 0`, each pair at `comm + 4 + 8·i`,
and the packet iterator (`comm + 0x1B0`), the range cursor (`comm + 0x1C4`) and
the range iterator (`comm + 0x1C8`) are reset (`0x00102008`–`0x00102018`) so the
following `0xD5` starts from the first range.

### 5.4 `0xD4` — erase a range, with progress

`FUN_001029EC`, `0x001029EC`–`0x00102A14`. Arguments: big-endian `u32 address`,
`u32 length` (`0x001029F0`, `0x001029F8`). Calls `FUN_001023AC(address, length)`
and replies `0x5A` when it returns zero, otherwise that error code as a single
byte (`0x00102A08`–`0x00102A14`).

`FUN_001023AC` (`0x001023AC`–`0x00102474`) is a range **erase**, not a checksum:
it resolves the address, walks the range sector by sector calling the driver
erase `FUN_00100D5C`, and emits progress on the same byte stream as two-byte
`0x04 <percent>` frames (`0x00102370`–`0x0010237C`). Its error codes are `9`
(range could not be resolved) and `10` (range not inside a mapped region), plus
whatever the driver returns.

**A client must treat `0xD4` as destructive, and must tolerate progress frames
arriving between the command and its final byte.**

### 5.5 `0xD6` — read a range

`FUN_00101E78`, `0x00101E78`–`0x00101F04`. Arguments: three big-endian `u32` —
page length (stored at `comm + 0x0C`), start address, total length:

```
0x00101E7C   bl   0x00102EAC        ; page length
0x00101E88   bl   0x00102EAC        ; start address
0x00101E90   bl   0x00102EAC        ; total length
0x00101E9C   bl   0x00103508        ; udiv(total + page - 1, page) = page count
0x00101EB8   bl   0x00101944        ; send one page
0x00101ED4-0x00101EF0               ; wait: 0x5A advances, 0xA5 retries
```

Each page is sent with the same `FUN_00101944` serialiser the write path uses,
so **a read page carries the zero padding and the 16-bit big-endian checksum
trailer** described in 4.4. After each page the host must send one byte: `0x5A`
to advance, `0xA5` to retry the same page. Any other byte is ignored and the DA
keeps waiting. Nothing is sent when the range is complete.

### 5.6 `0xD5` — write

`FUN_001024B8`, `0x001024B8`–`0x001028E4`. Argument: one big-endian `u32`
packet length (`0x001024D8`), stored at `comm + 0x0C`.

The command is a small protocol of its own:

| Step | Address | Direction | Bytes |
|---|---|---|---|
| length argument consumed | `0x001024D8` | host → DA | 4 |
| driver initialise (`vtable + 0x10`) | `0x00102500` | internal | — |
| first `ACK` | `0x00102504` | DA → host | `0x5A` |
| erase the sector holding the first range's start | `0x0010251C`–`0x00102540` | internal | — |
| erase failed | `0x0010253C`–`0x0010254C` | DA → host | `0xA5`, then restart |
| erase succeeded | `0x00102560` | DA → host | `0x5A` |
| per packet: payload, then 16-bit sum | `0x001025E0`, `0x001019F0` | host → DA | `length` + 2 |
| per packet accepted | `0x00102734`–`0x00102738` | DA → host | `0x69` |
| transfer complete | `0x00102898` | DA → host | `0x5A` |
| final sum check failed | `0x001028A0`–`0x001028B8` | DA → host | `0xA5` |

Per packet the DA also polls the flash driver for completion
(`0x001025E8`–`0x00102728`); the driver's return value decides whether the `0x69`
is sent, whether the packet is retried, or whether the write is abandoned with
`0xA5` and a restart.

**Only the ranges recorded by `0xD3` are written.** The handler walks the range
table from `comm + 4`, programming `end - start + 1` bytes per range
(`0x00102570`–`0x0010257C`), and `FUN_00101B0C` computes the packet count as
`ceil((end - start + 1) / length)` (`0x00101B0C`–`0x00101B34`). A client that
sends more packets than the ranges need is feeding the ring buffer with data
that has nowhere to go, and is answered with `0xA5`.

**The final acknowledgement is two `0x5A` bytes on the normal path**
(`0x001027F8` and `0x00102898`), with the host's checksum verification by the DA
in between. Both send sites are identified; what is not proven is whether the
host must interleave a byte between them — see section 7, item 1. The
hardware-tested client sends nothing there and the write succeeds, and hardware
observed exactly two `0x5A` bytes, so the contract a client may rely on is:
after the last packet's trailer, read two bytes and require both to be `0x5A`.

### 5.7 `0xD7` — read flash into DA memory

`FUN_00102A4C`, `0x00102A4C`–`0x00102A68`. Argument: big-endian `u32` address
(`0x00102A50`). Reads the half-word there through the driver helper
`FUN_00100DB0`, **stores it at the host-supplied address in the DA's own address
space** (`0x00102A5C`: `strh r0, [r4]`), and replies `0x5A`. No data is
returned to the host.

### 5.8 `0xD8` — write one half-word from DA memory

`FUN_00102A6C`, `0x00102A6C`–`0x00102A88`. Argument: big-endian `u32` address
(`0x00102A70`). Loads the half-word at that address in DA memory, sends it to
the host as a big-endian `u16` (`0x00102A74`, `FUN_00102E74`), and replies
`0x5A`. The reply is therefore **three bytes**: the half-word, then the ACK.

`0xD7` and `0xD8` are a matched pair over the DA's own memory: `0xD7` writes a
half-word into DA memory and `0xD8` reads one back. Neither touches the flash.
A client must not use them as flash primitives.

### 5.9 `0xDA` and `0xDB`

`0xDA` is inline at `0x00102CF4`–`0x00102D04`: send `0x5A`, then send the
half-word in `r4` (`0x00102CFC`–`0x00102D00`), then loop. `r4` is a leftover
from the previous handler and is not loaded for this case, so the value is
whatever the last command left behind. Treat it as undocumented.

`0xDB` is `FUN_00102A9C`, `0x00102A9C`–`0x00102B08`. Arguments: one big-endian
`u16` (`0x00102AC4`), compared against the baud divisor
(`0x00102ADC`–`0x00102AE8`), then `0x5A`, a delay, a write of the UART divider
registers from `DAT_0010281C`, and an infinite loop at `0x00102B08`. It is the
last command in a session: after it the DA never returns to the command loop.

## 6. Flash geometry

The region table is at `0x00103B38`: sixteen entries of `0x1C` bytes, terminated
by a region id of `0xFF`. Each entry:

| Offset | Meaning |
|---|---|
| `+0x00` | region id, echoed as the fourth banner byte |
| `+0x04` | pointer to the sector descriptor |
| `+0x08` | pointer to the driver operations table |
| `+0x0C` | u32: low u16 = number of descriptor entries to sum (a count, not a sector count), high u16 = the manufacturer id that the banner also sends at `+0x0E` |
| `+0x0E` | u16 manufacturer id, sent as banner word 1 |
| `+0x10` | u16 device id, sent as banner word 2 (`0x22BA` for the retail part) |
| `+0x12` | u16, sent as banner word 3 (`0x0000` for the retail part) |
| `+0x14` | u16, sent as banner word 4 (`0x0000` for the retail part) |
| `+0x18` | pointer to the driver vtable |

Note that `+0x0E`–`+0x14` are four adjacent u16 *inside the entry*, overlapping
what would be the high half of the `+0x0C` word and the whole of `+0x10`. The
banner reads them with `ldrh` at `0x00101C4C`, `0x00101C60`, `0x00101C74` and
`0x00101C88`.

For the retail part (region id `0x49`, entry at `0x00103B8C`, descriptor pointer
`0x00103EA0`, driver vtable `0x00103D64`), the entry is:

```
00000049  00103ea0  00103cf8  00c20001  000022ba  00000000  00103d64
 id       desc      ops       count|mfr device    zero     vtable
```

and the descriptor at `0x00103EA0` begins `0x00080000` (total size, 512 KiB),
`0x00000000`, `0x00000001`, `0x00004000`, `0x00004000`, `0x00000002`,
`0x00002000`, `0x00008000` — a sector-size/block-count descriptor. **The banner
does not read this descriptor**; it reads the entry's own `+0x0E`–`+0x14`. The
`0x2000`/`0x10000`/`0x3F` values that appear here and in the neighbouring
entries are block geometry, not banner fields.

The driver vtable maps: `+0x00` reset/unlock, `+0x04` unlock check, `+0x08`
sector erase, `+0x0C` status poll, `+0x10` no-op initialise, `+0x14` no-op
return, `+0x18` program half-word, `+0x20` status, `+0x24` device reset, `+0x30`
byte read, `+0x34` read with post-check, `+0x38` firmware base `0x00200000`.

The erase is the JEDEC `AA 55 80 AA 55 30` sequence
(`0x001011D0`–`0x00101238`); the program path issues `AA 55 A0` and then the
half-word (`0x001013E4` onwards). Nothing in the protocol exposes these
sequences to the host.

## 7. Unresolved

Recorded honestly rather than guessed.

1. **Whether the host must send a byte between the two final `0x5A` bytes of
   `0xD5`.** Two send sites exist (`0x001027F8`, `0x00102898`) and one of them
   is followed by a receive that expects `0x69` (`0x00102800`–`0x00102808`).
   The hardware-tested client sends nothing and works, so the receive is not
   reached on the single-range path — but the branch condition that decides
   this has not been reduced to a rule. A client should read exactly two `0x5A`
   bytes and send nothing.
2. **`0xD1`'s purpose.** The handler is fully decoded (consume one byte, reply
   `0x5A`); why the protocol needs it is not.
3. **`0xDA`'s half-word.** The value sent is a stale register. No caller was
   found that sets it deliberately.
4. **`0xD6`'s page length on hardware.** The argument is passed to an unsigned
   division that mixes it with the total length; the page size the DA actually
   emits for a given argument triple has not been measured.
5. **Whether any command computes a per-range checksum for the host to compare.**
   The only checksums found are the packet trailer and the DA's own read-back
   verification in 4.4. `0xD4`, which earlier notes called a checksum, is an
   erase.
6. **Whether the region table can select a part other than `0x49`.** Sixteen
   entries exist with ids `0x46`–`0x54`; only `0x49` was observed on hardware.
   The selector is the byte at `comm + 0`, set by `0xD0` and by `0xD3`.
7. **Whether `0xD5`'s per-packet retry yields `0x69` twice for one packet.** The
   retry path re-enters the receive loop, and the `0x69` is emitted on the
   completion branch; a client should be prepared to see more `0x69` bytes than
   packets it sent only if it also sees a retry, and no hardware measurement
   exists for that case.

## 8. Hazards for the implementation

Every one of these is a way to destroy the device or to produce a failure that
points somewhere else.

1. **`0xD5` erases before it writes, per sector.** A range recorded by `0xD3`
   and then used by `0xD5` is erased sector by sector as the write advances. A
   range that includes the reset vector (`0x00000000`) erases the start of the
   firmware image and the unit stops booting.
2. **`0xD3`'s reply length is not fixed.** Success is `5A 00`, refusal is a
   single `A5`. A client that always reads two bytes desynchronises after a
   refusal; a client that always reads one leaves a `0x00` in the buffer after
   success. This is the exact regression `AGENTS.md` §4.1 records, now with the
   failure case attached to it.
3. **A failed `0xD3` leaves the previous ranges in the table.** Never treat
   `A5` as "nothing recorded".
4. **`0xD9` never replies.** Waiting for an acknowledgement after `0xD9` hangs
   forever.
5. **`0xD4` is destructive and emits progress frames.** It erases the range and
   can send `0x04 nn` before its final byte.
6. **`0xD7` and `0xD8` are not flash primitives.** They read and write the DA's
   own memory at an address the host supplies, so `0xD7` with a flash address
   corrupts the running DA.
7. **The payload checksum is a 16-bit sum of bytes, big-endian**, like every
   other field — confirm the byte order from the wire, not from the BootROM's
   reputation, because the two halves of this protocol family differ in other
   places.
8. **`0xD3`'s `end` is an address, not a length.** A client that passes a length
   records a range of `end - start + 1` bytes, and the following `0xD5` erases
   all of it.
9. **A short final packet is normal.** The hardware-verified image is 406,620
   bytes and ends on a 92-byte packet; the DA accepts it and the length argument
   is what drives the `0x69` accounting.

## 9. Method

1. Load the DA image at `0x00100800` as ARM, little-endian instruction encoding.
2. Follow the reset vector to the initialiser and the command loop.
3. Read the 12-entry jump table at `0x00102C74` to map opcodes to handlers.
4. Read each handler end to end for its argument widths, its reply bytes and its
   calls to the flash driver.
5. Re-derive each multi-byte field's byte order from the shift arithmetic in
   `FUN_00102E34` / `FUN_00102E74` / `FUN_00102EAC` / `FUN_00102EE4` rather than
   assuming it.
6. Cross-check every reply against the hardware observations in
   `docs/dev/03-hardware-facts.md`.
7. Record only what the instructions show; list the rest in section 7.

The image this page describes is `DA_v3.8.4.bin`, 14,896 bytes, covering
`0x00100800`–`0x0010422F`. Only behaviour and addresses belong in the
repository; the working notes behind this page live under `_scratch/` and are
not committed.

## 10. Corrections this page makes to earlier notes

- **The protocol is big-endian throughout.** The BootROM is big-endian and so is
  the DA.
- `0xD3` does **not** erase. The erase is inside `0xD5`, per sector, before that
  sector is written (section 4.6).
- The `0xD3` status byte has no value other than `0x00`, and it is not sent at
  all on failure — the failure reply is a single `0xA5` (section 4.1).
- `0xD4` is an erase with progress reporting, not a checksum command
  (section 5.4).
- `0xD6` takes three arguments (page length, address, total length), not two
  (section 5.5).
- `0xD8` sends three bytes, not one (section 5.8).
- `0xD3`'s `end` is an address, not a length (section 4.2).
- `0xD2`'s second argument is an echo count, and the command requires a further
  `0xC0` / `0x5A` handshake before the link is usable again (section 4.5).
- The banner's four middle u16 fields are the manufacturer and device ids, not
  block geometry: they read `0x00C2` and `0x22BA` on the retail part, which is
  exactly the pair the hardware table records (section 2). The block geometry
  lives in the sector descriptor that the region entry points at and is never
  transmitted.
- The region entry's `+0x0C` word is a descriptor count in its low half and the
  manufacturer id in its high half, not a sector count (section 6).
