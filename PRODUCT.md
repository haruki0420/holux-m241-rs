# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

(Outside the current schema's four values, which cover web/ios/android/adaptive.
This is one native desktop application shipped to three operating systems from
one egui codebase; it does not adapt its design language per OS beyond window
chrome and font rendering.)

## Stack

Rust. Three-crate workspace: `m241-core` (protocol, firmware, log), `m241-cli`
(`m241 device|fw|log|nmea <verb>`), `m241-gui` (`eframe`/`egui` + `walkers` for
OpenStreetMap tiles). The CLI and the GUI are one binary: the GUI calls the same
`m241-core` functions the CLI calls, never a subprocess.

## Users

Someone who owns a Holux M-241 GPS logger bought around 2007 and has just noticed
its dates are 19.6 years wrong. They can install a single binary and plug in a
USB cable. They are not necessarily developers and do not want to read a protocol
specification.

Two pressures shape every screen:

- **The device can be lost.** A wrong write turns a working logger into scrap.
  The user knows this, or is about to learn it, and that anxiety is present at
  every destructive step.
- **The device is old and undocumented.** The port may not appear, sync may fail,
  the firmware may be a version nobody has profiled. Recovery paths matter as
  much as success paths.

## Product Purpose

Repair the GPS week-number-rollover damage on an M-241, and get the track log off
the device and onto a map. Success is: the logger reports today's date, the track
is visible on a map with photos attached, and the user never had to read a
protocol document to get there.

## Positioning

The only M-241 tool that ships a working simulator for its own protocol, so the
entire flow, including the destructive write, can be exercised and verified
before a real device is touched. In v2 the flash path is additionally rebuilt
from a disassembly of the vendor download agent rather than ported from other
implementations.

## Operating Context

- A desk, a USB cable, one M-241 with a charged battery.
- Windows was the only platform the previous version was hardware-verified on.
  The rewrite is developed on Arch Linux and will be validated on Windows again
  before flash is declared safe.
- The user alternates between the device (OLED, five-way button) and the app, so
  the app must tell them **when to go press a button on the device**. The
  power-cycle between the two dump passes is part of the workflow, not an error.
- Track logs are personal location data and never leave the machine.

## Capabilities and Constraints

Confirmed for v2:

- BootROM and Download Agent protocol; firmware dump, patch, flash.
- PMTK182 log download and erase.
- Binary log parsing; GPX, KML, KMZ, CSV export.
- Photo matching by capture time and EXIF GPS write-back.
- NMEA date repair and live relay.
- OpenStreetMap tiles through `walkers`, with an on-disk cache.
- CLI subcommands grouped as `m241 device|fw|log|nmea <verb>`.

Constraints:

- Flash safety gates carried over: NMEA pre-probe tri-state, backup diff check,
  typed `WRITE` confirmation, hard-coded load addresses.
- `clippy -D warnings`, plus `unwrap` / `expect` / `unsafe` denied outside tests.
- Flash behaviour is rebuilt from a disassembly of `DA_v3.8.4.bin`; the previous
  version and the upstream project are hypotheses, not sources of truth.

Undecided:

- Whether social upload returns. Out of scope for v2.

## Brand Commitments

The device is depicted truthfully or not at all: no invented product shots, no
claimed unit counts, no invented testimonials.

## Evidence on Hand

- `DA_v3.8.4.bin`, `brom.dll`, `brom.ini`, V1.12 and V1.13 stock images, the
  Japanese device manual, and the ezTour installer. All local, none
  redistributable.
- The 470-test v1 suite and its measured hardware facts, used as an oracle.
- One retail M-241, V1.13, CP210x, hardware-verified on Windows 11 in 2026-11.

Absent and not to be fabricated: OLED screenshots after flashing, Linux or macOS
hardware runs, Bluetooth SPP runs, 4 MiB and GR-245 units, any timing figure.

## Product Principles

1. **Name the risk at the moment it applies.** A warning in a preamble is not
   read; a warning on the control that destroys data is.
2. **Say what the device is doing, not what the code is doing.** "Waiting for you
   to power-cycle the logger" beats "second pass pending".
3. **Unverified is a first-class state**, labelled the same way everywhere.
4. **The safe path is the default path.** Read-only actions are one click; the
   destructive one is behind a typed word.

## Accessibility & Inclusion

Keyboard reachable end to end; the repair flow must be operable without a mouse.
Contrast at least 4.5:1 for body text. No colour-only status encoding.
