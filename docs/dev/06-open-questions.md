# Open questions

Real gaps recorded so nobody fabricates answers. (Former §3 — `0xD3` status byte — is closed in `docs/reference/01-da-protocol.md` §4.1: always `0x00` on accept, omitted on refusal.)

## 1. Where does the log actually live?

The DA reports a 512 KiB flash (MX29LV400CB), yet a unit held 212,860 B of log alongside a 406,620 B firmware image. Post-write downloads on 2026-11-22 (3,228 / 3,260 B) contained only the settings journal (PR #34 fixed the parser misread; pre-write 2026-11-21 dump parses to 10,942 points / 0 bad checksums). Still open: physical location of log storage, whether post-write units record track points (with or without fix), and layouts on V1.12 / 4 MiB / GR-245. Needs full-chip read past `0x80000` or schematic.

## 2. Does a Japanese-language firmware SKU exist?

No Japanese strings exist in inspected V1.12 / V1.13 packages (release notes list TC/SC/DE/FR/EN; 41-entry string table identical). Treat as closed unless a Japanese image surfaces.

## 3. Was Bluetooth SPP ever exercised?

No. All hardware sessions used USB (`docs/dev/05-verification.md` §1.5).

## 4. Can the reset-line entry be made safe?

`MtkBootRom` refuses DTR/RTS when NMEA state is unknown; a completely silent unit answering neither NMEA nor `0xA0` cannot be recovered in software without an external signal (current draw / GPIO).

## 5. Raw device speed vs. recomputed speed (ezTour parity)

ezTour recomputed speed/distance from point sequences (`docs/dev/07-eztour-spec.ja.md` §4.2). Undecided whether summaries/charts should prefer raw device speed when present or recompute uniformly.

## 6. Exact rules for ezTour's `byCamera` and `selPhotos` scopes

`07-eztour-spec.ja.md` §6.1-7 does not specify matching details. `PhotoShiftScope` currently uses exact matches: `ByCamera` compares valid-UTF-8 IFD0 `Make`/`Model` with trailing NUL/space stripped (case-sensitive); `SelectedPhotos` compares exact paths; 1 day = 86,400 s. Real-camera IFD0 layouts are unverified (synthetic JPEGs only).

## 7. How did ezTour's KMZ carry `icon/` thumbnails?

`07-eztour-spec.ja.md` §6.3 mentions `icon/` without specifying dimensions, naming, or KML references. `export_kmz_with_icons` takes caller size and leaves KML unchanged; unverified against real ezTour KMZ output.

## 8. Should `--allow-vendor` read-back skip firmware-managed bytes (`0x5C`–`0x5F`)?

Waiting on user decision (current merged behaviour is **C**). On 2026-11-22 (`05-verification.md` §1.5), `--allow-vendor` wrote all 1,589 packets and booted V1.13 (`06-01-80`), but post-reboot read-back differed at `0x5C` because the booted firmware restores its 4 per-unit bytes `0x5C`–`0x5F` (`91 1B D4 CF`; origin unknown). The other 10 per-unit bytes (`0xF8`–`0xFD`, `0x63458`–`0x6345B`) kept vendor values.

- **A (Recommended — needs user approval & `AGENTS.md` §2.3 update)**: exclude constant `0x5C`–`0x5F` only in `write_vendor` read-back (never strict or `--recover`), verifying the remaining 406,616 bytes. Narrow relaxation; cannot catch a write fault confined to those 4 bytes.
- **B (Rejected)**: treat read-back as informational only. Drops post-write proof (and leaves `--recover` with no verification).
- **C (Current)**: keep strict read-back and document the expected `0x5C`–`0x5F` mismatch; restore unit bytes via `fw flash --recover` with pre-vendor backup (untested after vendor write).
- *(Unready 4th option: `0xD6` read before `0xD9` — multi-page `0xD6` is unmeasured, `01-da-protocol.md` §7 item 4.)*

## 9. Where does the no-fix clock get its value?

On 2026-11-22 (`05-verification.md` §1.5), patched V1.14 emitted `22-08-99` right after flash, then later reported current UTC `221126` with no fix. Unknown whether this comes from a free-running RTC or an earlier fix, and live-fix date after patching remains unobserved.

## 10. Unexercised GUI paths on real hardware

Connect's read-only probe ran on hardware (2026-11-22, `05-verification.md` §1.5), and Log + OSM tiles ran on dev-host X11/Wayland (2026-11-23, no device). Unexercised on hardware: Log file open, follow render, geotag JPEG write, KMZ/shift/edit UI, device download/erase/relay, and GUI firmware write.
