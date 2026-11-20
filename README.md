# Holux M-241 — Rust rewrite

Tools for the **Holux M-241** GPS logger (MediaTek MT3318). The device records
dates 19.6 years in the past because its 2006 firmware hard-codes the GPS epoch
offset and never accounts for the 2019 week-number rollover. This project repairs
it, and reads the track log off the device onto a map.

**Status: skeleton.** The shell, the protocol frame layer and the token set exist
and are tested; the screens and the flash path do not. Nothing here has been run
against a real device yet — see [`docs/dev/05-verification.md`](docs/dev/05-verification.md).

## What it will do

| Job | Command |
|---|---|
| Find the logger's port | `m241 device ports` |
| Read its identity | `m241 device info` |
| Dump the firmware (read-only, safe) | `m241 fw dump` |
| Apply the date fix to the dump, on the PC | `m241 fw patch` |
| Write the fixed image back | `m241 fw flash` |
| Pull the track log off the device | `m241 log download` |
| Convert it to GPX / KML / CSV / NMEA / JSON | `m241 log read` |
| Put the track's coordinates into photos | `m241 log geotag` |
| Correct a live NMEA stream without flashing | `m241 nmea proxy` |

The GUI (`m241-gui`) offers the same jobs as four screens: 接続, ログ, ファーム,
設定. It calls the same core functions the CLI does.

## Building

```bash
cargo build --release
cargo test --workspace
./tools/check.sh          # fmt + clippy + tests + docs links + design invariants
```

Rust 1.85 or newer. The only runtime dependency is `serialport`, and the offline
commands never open a port.

## What this repository does not contain

The Holux firmware images, the download agent, the vendor DLLs and the vendor
executables are proprietary and are **not** distributed here. `tools/` reads them
from a local folder you supply; `.gitignore` blocks the common filenames so an
accidental `git add .` cannot publish them.

## The flash path is being rebuilt from the binary

The two existing Python implementations disagree about the download agent's
command semantics, and both were written against the same incomplete picture.
Rather than port either, this project disassembles `DA_v3.8.4.bin` and records the
command set in [`docs/reference/01-da-protocol.md`](docs/reference/01-da-protocol.md)
before implementing it. `crates/m241-core/src/protocol/da.rs` currently refuses to
operate and says why.

[`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) records what was taken from
where, and which prior claims are still hypotheses.

## Safety

Firmware modification of a discontinued device with no JTAG. Read
[`DISCLAIMER.md`](DISCLAIMER.md) before flashing anything. The short version:
take your own dump first, keep it, and only ever flash an image you built from it.

## Documentation

| Page | What it answers |
|---|---|
| [`docs/dev/03-hardware-facts.md`](docs/dev/03-hardware-facts.md) | every measured constant, and how it was measured |
| [`docs/dev/05-verification.md`](docs/dev/05-verification.md) | what was verified, and what was not |
| [`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) | what came from where |
| [`docs/dev/06-open-questions.md`](docs/dev/06-open-questions.md) | the questions with no answer yet |
| [`docs/reference/02-cli.md`](docs/reference/02-cli.md) | the command surface and exit codes |
| [`DESIGN.md`](DESIGN.md) | the interface: direction, tokens, screens |
| [`PRODUCT.md`](PRODUCT.md) | who this is for and what it must not do |
| [`AGENTS.md`](AGENTS.md) | working notes for automated contributors |

## Licence

MIT for the code and documentation. The proprietary binaries remain the property
of their owners.
