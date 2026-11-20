# Download Agent protocol — **under construction**

**Status: empty on purpose.** This page is filled from the disassembly of
`DA_v3.8.4.bin`, and it is written here *before* the code that implements it.

## Why the page exists before the code

Two prior Python implementations of this repair disagree about the DA's command
semantics, and both were written against the same incomplete picture. Porting
either would carry an unknown number of wrong assumptions into a path that can
destroy the device.

The rule this project follows: **every constant and every command in
`crates/m241-core/src/protocol/da.rs` cites a function and an instruction range
on this page.** If the citation cannot be written, the code cannot be written.

## What is known from behaviour

Established on a retail unit and recorded in `docs/dev/03-hardware-facts.md`.
These are observations, not explanations:

- `0xD3` is answered with two bytes; the second is `0x00` on success.
- `0xD5` is answered with two `0x5A` bytes.
- Each `0x100`-byte packet is answered with `0x69`.
- A `0xA5` anywhere means the DA refused.
- `0xD9` restarts the chip.

## What the disassembly has to answer

| Question | Why it matters |
|---|---|
| What does the `0xD3` status byte mean for values other than `0x00`? | today a non-zero status is reported as a failure with no detail |
| Does `0xD3`'s range argument choose sectors, or bytes within one sector? | decides whether a range can straddle a block boundary safely |
| What is the maximum packet length, and does it change with baud rate? | `0x100` is what the vendor tool sends, not necessarily the maximum |
| Does the DA verify the sum-of-bytes itself, or only echo it? | decides whether a corrupt packet is silently accepted |
| What are the argument forms of `0xD2` (baud) and `0xD9` (restart)? | the vendor documentation is vaguer here than for the write path |
| What happens on a write to a protected or unmapped range? | this is how the `0xD3` status byte gets a meaning |

## Method

1. Load `DA_v3.8.4.bin` at `0x00100800` (ARM, little-endian, Thumb/ARM mixed).
2. Identify the serial receive loop and the command dispatch.
3. Map each opcode to a handler, and each handler to its reply.
4. Record the reply bytes and the offsets that produce them.
5. Write the constants into this page with the instruction addresses.
6. Only then implement.

The disassembly tooling and the analysis notes live under `_scratch/` and are not
committed: the binary is proprietary and the notes are about the binary.
