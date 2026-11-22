# Verification: what was measured, and what was not

This page is the only place a claim may move from 未検証 to verified. Every other
document points here.

`✅` measured on a retail M-241 · `⚠️` not measured on hardware · `🧪` covered by
the simulator and tests only — see §3 for what that is and is not worth.

## 1. The hardware sessions

### 1.1 The write and dump (2026-11-15)

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

### 1.2 The DA-side base read (2026-11-21)

Device: the same retail M-241. Host: Linux, `/dev/ttyUSB0` (USB serial).

Command, after re-entering the BootROM with `--assume-brom`:

```text
m241 fw probe-da --da <DA_v3.8.4.bin> --port /dev/ttyUSB0 --yes --assume-brom
```

Output:

```text
loading the DA into RAM on /dev/ttyUSB0 (this writes no flash), then reading 64 bytes at 0x00000000 and 0x00200000
BootROM sync answered; writing the DA into the chip's RAM
DA v3.8 answered (region 0x49, flash 00C2:22BA, 524288 bytes); reading
0x00000000  : read, first bytes 18 F0 9F E5 (matches the vector 18 F0 9F E5)  first 16: 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5 18 F0 9F E5
0x00200000  : read, first bytes 02 02 20 42 (does not match the vector)  first 16: 02 02 20 42 00 00 AA 55 AA 55 AA 55 AA 55
verdict     : the image is at 0x00000000
flash write : no
```

* **DA-side base: `0x00000000`.** The first four bytes are the firmware vector
  `18 F0 9F E5`, and the first 64 bytes match the vendor V1.13 image's first 64
  bytes exactly.
* **`0x00200000` in the DA's view is not firmware.** It reads `02 02 20 42`,
  consistent with the unmapped-bus pattern `42 20 02 02` byte-reversed. This is
  a different address view from the BootROM's: the BootROM reads the firmware
  vector at `0x00200000`, while the DA reads it at `0`. The same number means
  different things in the two views.
* **No flash was written.** The only writes were the BootROM's `0xA1` frames
  into the DA's RAM at `0x00100800`–`0x0010422F`, and the `0xA8` jump. No
  `0xD3`, `0xD4`, `0xD5` or `0xD9` was sent.
* **Manual check in the same session:** one `0xD6` page with a 64-byte
  page-length argument is **64 data bytes + a 2-byte big-endian checksum** (66
  bytes total). The trailer of the page read at address `0` was `0x17FF`, equal
  to the sum of its 64 bytes.
* The first attempt read four bytes off (trailer `0x7035`, sum `0x1870`); after
  the drain fix the read succeeded. **Four bytes remained after the banner**;
  their origin is not established.
* Only **one page** was read at each address. Multi-page reads, the advance
  after the final page of a multi-page read, and bytes between pages were not
  exercised.

## 1a. The Linux log-erase hardware session (2026-11-21)

Device: retail Holux M-241 (MT3318) over USB serial (CP210x bridge).
Host: Linux, `m241` built from commit `bf41c2e`, the then-head of
`feat/log-erase-wire`. That commit is **not** an ancestor of this branch, and no
binary from this branch's final head has been run on hardware.

The rows below were measured with that binary only. The log-erase work has
gained changes since: the empty-log refusal (`ea53d1a`) and the lost-stop-ACK
resume with its tests. **Those changes have not been run on hardware**, so
nothing on this page makes the final head verified.

| Step | Command / detail | Result | Status |
|---|---|---|---|
| Bare `$PMTK182,6` (negative result) | `m241 log erase --port <port> --output <backup> --yes` before fix | backup saved (`225,028` bytes), failed on trailing `$PMTK001,182,4,3*23`; follow-up `m241 log download` showed `225,060` bytes / `7,846` records (log untouched) | ✅ (negative) |
| Log download & erase (`$PMTK182,5` → `$PMTK182,6,1*3E` → `$PMTK182,4`) | `m241 log erase --port <port> --output <backup> --yes` | backup saved (`225,092` bytes), `erased: yes`, exit `0` | ✅ |
| Post-erase verification | `m241 log download --port <port> --output <check> --force` | `528` bytes (`0x210`), `records: 0`, `fixes: 0`, exit `0` | ✅ |
| Final-head re-run (empty-log refusal, lost-stop-ACK resume) | not run: the binary above predates both changes | — | ⚠️ |

## 2. What was never measured

| Claim | Why it is still open |
|---|---|
| The OLED after flashing | never observed; the display was not photographed |
| The binary log's timestamps after flashing | inferred from the shared NMEA conversion path; no post-flash log was downloaded |
| Linux BootROM / firmware dump & flash with a real logger | `log download` and `log erase` ran on Linux on 2026-11-21 (§1a); BootROM / DA paths have not been recorded on Linux yet |
| macOS with a real logger | same |
| Bluetooth SPP | all hardware work went over USB |
| 4 MiB units and GR-245 variants | geometry maths is implemented and unit-tested; no device was available |
| V1.11 and V1.12 on hardware | no V1.11 image to profile, V1.12 was never flashed |
| The V1.12 patch offsets (`0x402F6`, `0x402F7`, `0x59B18`) | derived from the V1.12 vendor package's layout (`0x59B14` marker, size and SHA-256 in `03-hardware-facts.md`), not from a V1.12 unit; V1.12 was never dumped or flashed here |
| Any timing figure | nothing in this repository is a stopwatch measurement |
| The Rust `fw flash` command on a unit | the 2026-11-15 flash was an earlier manual/v1 run; this Rust client has never opened a port for a write. The write, erase, restart and backup-diff gates are covered only by the simulator |
| The `0xD9` → NMEA probe → `$PMTK180*3B` re-entry and read-back | implemented in the CLI and exercised against the fake only; no unit has been asked to boot its firmware and hand the BootROM back after a write. The 2026-11-15 row records that the unit booted, not that this read-back ran |
| The 406,620-byte BootROM read split into `DUMP_CHUNK` frames | the frame count and per-frame byte limits are fixed against the fake; no unit has been read at this size through the chunked path. A single 406,620-byte `0xA2` would exceed the five-second deadline at 115,200 8N1 |
| The DA's `0xD6` read across multiple pages | one page per address was read on 2026-11-21; the advance between pages, the advance after the **last** page of a multi-page read, and any bytes the DA sends between pages are unmeasured (`docs/reference/01-da-protocol.md` §5.5, §7 item 4) |
| The four bytes after the DA banner | observed once on 2026-11-21; their origin is not established. The client drains them before `0xD6` |
| `0xD6`'s page length for arguments other than 64 | only a 64-byte page-length argument was measured (64 data bytes + 2-byte trailer) |
| `fw probe-da` on stock V1.13 and on a second unit | the 2026-11-21 run read the unit from §1.1, which already carries the WNRO patch (the version marker is patched to V1.14); its first 64 bytes match the vendor V1.13 image's, but no stock unit and no second unit were probed |
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
