# Product

<!-- impeccable:product-schema 1 -->

## Platform & Stack

Cross-platform desktop (Linux / Windows / macOS) in Rust: `m241-core` (read-only protocol, firmware, log, EXIF/geotag), `m241-flash` (destructive write/erase), `m241-cli` (`m241 device|fw|log|nmea <verb>`), `m241-gui` (`eframe`/`egui` + `ureq` for OSM tiles; shares `m241_flash::flow` with CLI, user decision 2026-11-23), plus Tauri v2 `m241-app`.

## Users & Purpose

For owners of a ~2007 Holux M-241 GPS logger whose dates are 19.6 years behind and whose vendor app (HOLUX ezTour v3.0) is dead (`docs/dev/07-eztour-spec.ja.md`):
- Safely repair the GPS week-number rollover without reading a protocol spec.
- Download, view, edit, and export track logs (GPX/KML/KMZ/CSV/HTML/RTF/NMEA/JSON) and geotag photos with no activation servers or external tools.
- Ship a protocol simulator and rebuild the flash path from `DA_v3.8.4.bin` disassembly rather than porting prior scripts.

## Operating Context & Privacy

- Desk, USB cable, charged M-241; user alternates between device hardware (power-cycle between dump passes) and app.
- Track logs stay local and are never uploaded. The only internet traffic is background OpenStreetMap tiles (on by default; viewing area follows the track; switchable off in Settings/toolbar so nothing leaves the machine).

## Constraints

- Safety gates: NMEA pre-probe tri-state, backup-diff check, explicit `WRITE`/`--yes` confirmation, hard-coded ranges.
- `clippy -D warnings`; `unsafe`, `unwrap`, `expect`, `panic` denied outside tests.
- Out of scope: SNS upload, activation, AGPS, proprietary binary redistribution.
- Truthful reporting only: never fabricate hardware evidence, screenshots, or timing figures (`docs/dev/05-verification.md`).

## Product Principles & Accessibility

1. **Name the risk at the moment it applies** (on the destructive control, not buried in a preamble).
2. **State what the device is doing** ("power-cycle the logger", not internal state names).
3. **Unverified is a first-class state**, labelled consistently everywhere.
4. **Safe path by default**: read-only is one click; destructive requires a typed word.
5. **Accessibility**: full keyboard reachability, ≥4.5:1 body contrast, no colour-only status.
