# Verification: what was measured, and what was not

This page is the only place a claim may move from 未検証 to verified. Every other
document points here.

`✅` measured on a retail M-241 · `⚠️` not measured on hardware · `🧪` covered by
the simulator and tests only.

## 1. The hardware session (2026-11-15)

Device: `B-core_1.20`, build `0043`, model `01017-00F` (V1.13), MT3318.
Host: Windows 11, CP210x bridge (VID 10C4 / PID EA60) as a COM port.

| Step | Result | Status |
|---|---|---|
| `$PMTK705` identity read | returned the strings above | ✅ |
| BootROM entry via `$PMTK180*3B`, then `0xA0`/`0x0A`/`0x50`/`0x05` | 36 probes before `0x5F` | ✅ |
| Firmware dump, 406,620 bytes, read twice | SHA-256 identical both passes | ✅ |
| Header decode | family `HOLUX_M-241`, device `01017-00F`, build 67 | ✅ |
| WNRO patch | 3 bytes changed: `0x402F6`, `0x402F7`, `0x59C2C` | ✅ |
| Flash write through the DA | every packet accepted, DA restarted | ✅ |
| Boot after flash | the logger booted | ✅ |
| `$GPRMC` after flash | date `151126` = 2026-11-15 | ✅ |
| Log download | 212,860 bytes, 10,399 records, 0 CRC errors | ✅ |

The measured dump's SHA-256 differs per unit because the image contains
per-unit runtime fields; compare against your own backup, never against a
figure on this page.

## 2. What was never measured

| Claim | Why it is still open |
|---|---|
| The OLED after flashing | never observed; the display was not photographed |
| The binary log's timestamps after flashing | inferred from the shared NMEA conversion path; no post-flash log was downloaded |
| Linux with a real logger | code runs in CI; no device was connected |
| macOS with a real logger | same |
| Bluetooth SPP | all hardware work went over USB |
| 4 MiB units and GR-245 variants | geometry maths is implemented and unit-tested; no device was available |
| V1.11 and V1.12 on hardware | no V1.11 image to profile, V1.12 was never flashed |
| Any timing figure | nothing in this repository is a stopwatch measurement |

## 3. The simulator

`crates/m241-core` is exercised by a fake device that models the chip's own
behaviour: echo, the checksums, the 16-bit pair swap, the 20-byte DA report, and
the delay before a read's data arrives. It is not a mock: it drives the real
client code paths, which is what makes a protocol regression fail loudly instead
of corrupting a unit.

🧪 marks a claim that is covered this way. It is weaker than ✅ and the documents
say so at the point of use, not only here.

## 4. How to close a gap

1. Run the flow against a real device.
2. Record the command, the output, and the date in section 1.
3. Update the `⚠️` at the point the claim is made.

If you cannot do all three, the claim stays `⚠️`. Writing `✅` for an untested
platform is a defect, not optimism.
