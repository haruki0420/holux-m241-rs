# Prior art: what was taken from where

Two implementations of this repair exist, and this one is the third. Neither of
the first two is a source of truth; both are inputs that were checked against the
vendor binaries where it mattered.

| Project | Language | Licence | What it established | What this project does differently |
|---|---|---|---|---|
| `NekobaTechnology/holux-m241-wnro-fix` | Python | MIT | The `0x402F4` epoch patch, the DA load address, the `$PMTK180*3B` entry route, the failure of the "patch the +1024 constant" approach | Rebuilds the flash path from the DA disassembly rather than its framing; adds the NMEA pre-probe, the backup diff check and a simulator |
| `holux-m241-wnro-toolkit` (v1, Python) | Python | MIT | A full CLI, a 470-test suite, the measured protocol constants (echo lags, two-byte `0xD3` reply, no read trailer), the log format quirks | Rust; flash rebuilt from the binary; the log parser and exporters are re-derived from the format description, not ported |
| this project | Rust | MIT | — | — |

## What is a hypothesis and what is verified

A fact is **verified** when it was measured on a retail M-241 and is recorded in
`docs/dev/05-verification.md`. Everything else, including facts both prior
projects agree on, is a **hypothesis** until this project reproduces it.

The distinction matters most for the flash path. The two prior implementations
disagree in places, and both were written against the same incomplete picture of
the DA command set. That is why the command semantics come from disassembling
`DA_v3.8.4.bin` and are written into `docs/reference/01-da-protocol.md` before any
code implements them.

## Prior-art hypotheses to check against the disassembly

Recorded here so they are checked rather than copied. Each becomes either a
citation in the reference document or a correction.

| Hypothesis | Source | Status |
|---|---|---|
| `0xD3` replies with two bytes: ACK plus a status byte | v1, measured on hardware | to re-check |
| A non-zero `0xD3` status means failure | v1 | to re-check |
| WRITE16's echo trails by one byte, so only `sent[..n-1]` is echoed | v1, measured on hardware | to re-check |
| Reads carry no checksum trailer; `0xA4` is separate | v1, measured on hardware | to re-check |
| `0xD3` is one ten-byte frame, not split into header/begin/end | v1 | to re-check |
| The DA accepts a short final packet | v1 | to re-check |
| `0xD9` restarts the chip and the firmware boots | v1, measured on hardware | to re-check |
| Load address `0x00100800` | both, and `push 0x100800` in the vendor utility | corroborated by two sources |

## What is deliberately not carried over

- **The Python implementation's structure.** The crate split here follows the
  device's own software layers (wire, image, log), not the v1 module tree.
- **The upstream project's safety posture.** It pulses the reset lines and
  continues after a failed verification where this project refuses. Those
  differences are defects here, not features.
- **Any binary.** No firmware image, download agent, DLL or vendor executable is
  redistributed. `tools/` reads them from a local drop the user supplies.
