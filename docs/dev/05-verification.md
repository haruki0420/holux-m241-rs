# Verification: what was measured, and what was not

This page is the only place a claim may move from 未検証 to verified. Every other
document points here.

`✅` measured on a retail M-241 · `⚠️` not measured on hardware · `🧪` covered by
the simulator and tests only — see §3 for what that is and is not worth.

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

A **settlement row** for the DA-side firmware base is the only row that may
justify emptying `UNRESOLVED_STARTS`. It must contain at least:

* the exact `fw probe-da` command line that ran, including the resolved
  `--port` and the `--da` path;
* what `0xD6` returned at **both** candidates, `0` and `0x00200000` — a failed
  read is recorded as "not read", never as a mismatch;
* whether the first four bytes at the adopted base are `18 F0 9F E5`;
* the base adopted, and why the other candidate was excluded;
* the date;
* an explicit statement that the run wrote no flash (DA load into RAM only).

The `Flash write through the DA … ✅` row above is the earlier manual flash.
It is **not** a settlement row: it records no `fw probe-da` run and says
nothing about which base the DA uses, so it cannot justify emptying
`UNRESOLVED_STARTS`.

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
| The DA-side firmware base (`0` or `0x00200000`) | `fw probe-da` reads both and reports; no settlement row has been recorded in §1 (which defines what that row must contain), so `FLASH_WRITE_START` stays refused |
| The DA's `0xD6` read itself | page length it emits for a given page-length argument, whether it sends anything before the first page, and whether the host's advance after the **last** page is sent or omitted (`docs/reference/01-da-protocol.md` §5.5, §7 item 4); the client sends one advance per page including the last |
| The firmware vector `18 F0 9F E5` through the DA | measured in the BootROM's view only; `fw probe-da`'s test of the first four bytes has never been run against a unit, so its `Settled` verdict is unverified |
| `fw probe-da` on a stock V1.13 and on a V1.14 unit | neither has been run: the image measured so far is a patched (V1.14-reporting) unit, and the probe's output is what a unit run has to fill in |
| The `0xA8` jump frame's count word | the protocol page gives it no meaning; the only client that ran the jump on a unit sent `0`, and this crate does too. Whether the BootROM reads the field at all is not established (`docs/dev/03-hardware-facts.md` "DA jump") |

## 3. The simulator

`crates/m241-core` is exercised by a fake device that models the chip's own
behaviour: echo, the checksums, the 16-bit pair swap, the two read widths, and
the PMTK command/reply framing. It is not a mock: it drives the real client code
paths. The Download Agent is no longer modelled there — it belongs to the write
crate, and its fake lives in `crates/m241-flash/src/simulator.rs`.

**It is not a proof of correctness, and 🧪 never means "verified".**

The reason is a circular reference. The simulator and the client share the same
functions: the fake swaps 16-bit pairs with the crate's own `swap_pairs`, and the
client undoes it with the same function. If that function's notion of the byte
order is wrong, both halves are wrong in the same direction and the round trip
still passes. The same holds for the checksum, the frame layout and every
constant that is read from the crate rather than written out twice. A test that
passes therefore shows the two halves agree; it does not show either half agrees
with the device.

What the simulator *is* good for is a **regression test**: it catches the
refactor that quietly changes behaviour, because the behaviour it compares
against was recorded from a version that was checked another way. That is a real
job — most of the protocol's costliest bugs were regressions, not first attempts
— but it is a different job from establishing that the first attempt was right.

Correctness is carried by three things instead:

1. **The citation.** Every protocol constant points at a measurement in
   [`03-hardware-facts.md`](03-hardware-facts.md) or at a function and
   instruction range in [`../reference/01-da-protocol.md`](../reference/01-da-protocol.md).
   A constant that cannot be cited is a guess and is labelled as one.
2. **The hardware run.** Section 1 is the only place a claim becomes ✅. A
   command that never ran against a unit stays ⚠️ no matter how green the suite
   is.
3. **The review.** [`11-safety-review.md`](11-safety-review.md) fixes the
   questions a diff is read against, and `tools/check_safety.py` turns the ones
   that can be mechanical — the read/write crate split — into a gate.

🧪 marks a claim covered only by the simulator and tests. It is weaker than ✅ and
the documents say so at the point of use, not only here. Treat it as "this is
pinned, so it will not drift unnoticed", never as "this is known to be right".

## 4. How to close a gap

1. Run the flow against a real device.
2. Record the command, the output, and the date in section 1.
3. Update the `⚠️` at the point the claim is made.

If you cannot do all three, the claim stays `⚠️`. Writing `✅` for an untested
platform is a defect, not optimism.
