# Working notes for automated contributors

Read this before touching `crates/m241-core/src/protocol/`. It records what was
expensive to learn so it does not have to be rediscovered.

## 0. How to work here

**One check command, run before every commit:**

```bash
./tools/check.sh
```

It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --workspace` and the docs link check, and prints one line per gate.

**Long commands go in the background.** A first `cargo build` of the GUI pulls
in `wgpu` and takes several minutes. Do not block a turn on it.

**Never manage files with a script.** Moving or renaming: `git mv`. Editing:
the editor tools, one change at a time. A bulk find-and-replace once corrupted
Python identifiers in the predecessor project (`True` became `hrue`) and had to
be reverted from git.

**Do not claim what has not been verified.** If something has not been run on a
real M-241, the docs say 未検証 and so does your report. The hardware session in
`docs/dev/05-verification.md` is the only file allowed to move a claim from
unverified to verified.

## 1. What this project is

A cross-platform (Windows / Linux / macOS) tool for the **Holux M-241** GPS
logger (MediaTek **MT3318**, firmware `B-core_1.20`, model `01017-00F`).

Three jobs:

1. **Repair the GPS week-number rollover.** The 2006-era firmware hard-codes the
   GPS-epoch Unix offset `315964800` and never accounts for the second rollover
   (2019-04-06), so every timestamp is 7168 days in the past. The 4-byte constant
   at file offset `0x402F4` is replaced (`315964800` → `935280000`); **3 bytes
   actually change**.
2. **Download and convert the track log**, including matching photos by capture
   time.
3. **Repair or relay a live NMEA stream** for hosts that cannot be patched.

The only runtime dependency is `pyserial`… no: in v2 the only dependency at all
is the Rust standard library plus `serialport`, and the offline commands never
open a port.

## 2. Non-negotiable safety rules

### 2.1 Never pulse DTR/RTS at a logger that is streaming NMEA

`MtkBootRom::sync()` on its own is safe. Asserting the reset lines is not: at a
running firmware it leaves the unit unresponsive until the battery is removed.

The entry routine therefore checks for NMEA first and returns a tri-state:

* `true` — firmware is running, use `$PMTK180*3B`;
* `false` — definitively silent, the reset route is safe;
* `None` — cannot tell, **refuse**.

Do not "simplify" that tri-state away.

### 2.2 `0xD3` erases the range it covers

The DA erases whole sectors when you set a memory range. A range starting at `0`
erases the start of the firmware image, including the reset vector, and the unit
stops booting. The range must be a hard-coded constant checked against an
expected value, never derived from user input.

### 2.3 Take a dump before writing anything

Any image whose differing bytes fall outside the patch offsets of the user's own
backup is refused. An image identical to the backup is accepted. Do not weaken
that check.

### 2.4 Never commit personal data

See §6.

## 3. Verified hardware facts — do not re-derive

| Fact | Value | Established by |
|---|---|---|
| Firmware base (BootROM view) | `0x00200000` | `0x20000000` returns `42 20 02 02` (unmapped bus); `0x00200000` returns `18 f0 9f e5 …` |
| DA load address | `0x00100800` | `0x100000`–`0x1007FF` is BootROM scratch; writing there kills the chip |
| DA info report | `>BHBLHHHHL`, 20 bytes, starts `0xC0` | DA v3.8.4 answers `C0 03 08 49 …` |
| Flash on the retail unit | 512 KiB, MX29LV400CB, mfr `0xC2`, device `0x22BA`, DA id `0x49` | DA info report |
| Firmware image size | 406,620 bytes (`0x6345C`) | read back and hashed |
| NMEA | 38400 8N1, `$GPGGA/$GPGSA/$GPGSV/$GPRMC/$GPVTG` at 1 Hz | |
| UI languages | English / French / German — no Japanese | release notes list TC/SC/DE/FR/EN |

The DA's `flash_size` describes the chip it can program. A unit was measured
holding 212,860 bytes of log **alongside** a 406,620-byte firmware image, so the
log area is not that chip. Unresolved; see `docs/dev/06-open-questions.md`.

## 4. The protocol gotchas that cost the most time

### 4.1 `0xD3` replies with two bytes

ACK plus a status byte. Reading only the ACK leaves the status byte in the
buffer, shifting every later frame by one, so the next command times out and it
looks like the `0xD5` step is broken.

### 4.2 WRITE16's echo trails by one byte

After N bytes only N−1 echo bytes are available: the echo of the final byte
never arrives because nothing follows it. Writing one byte and waiting for its
echo hangs; requiring an exact echo for every chunk fails on the last chunk.
Accumulate and compare against `sent[..len-1]`, then drain the tail.

### 4.3 Reads carry no checksum trailer

`0xA2` / `0xAF` return echo + data only. The checksum is a separate `0xA4`
command. Consuming a two-byte trailer desynchronises every read.

### 4.4 The two widths order bytes differently

| Opcode | Wire order | Conversion |
|---|---|---|
| `0xA2` / `0xA1` | 16-bit pairs swapped | `swap_pairs` |
| `0xAF` | each 32-bit word byte-reversed | `reverse_words32` |

Using the pair swap for `0xAF` yields plausible garbage, not an error.

### 4.5 Count fields are in units, not bytes

`0xAF` takes dwords, `0xA2`/`0xA1`/`0xA4` take words. `Frame::for_bytes` encodes
that, and refuses a count that is not a whole number of units.

### 4.6 `$PMTK180*3B` needs a live NMEA stream

If the stream has already stopped the sentence is ignored, so the client checks
for the stream first and reports a precise error instead of a bare sync timeout.
Send at the NMEA baud, then switch to 115200.

### 4.7 The three sync probes must be back to back

`0xA0` → `0x5F`, then `0x0A`→`0xF5`, `0x50`→`0xAF`, `0x05`→`0xFA` with no pause.
Inserting delays drops the chip out of the handshake.

## 5. Log download (PMTK182)

```
$PMTK182,2,8            -> $PMTK182,3,8,<hex bytes used>
$PMTK182,5              -> logging disabled
$PMTK182,7,<addr>,<len> -> $PMTK182,8,<addr>,<hex data>
$PMTK182,4              -> logging enabled
$PMTK182,6              -> erase the log
```

Two things are easy to get wrong:

* **Address and length are hexadecimal without a `0x` prefix.** `$PMTK182,7,64,64`
  reads from `0x64` and returns `0x64` bytes.
* **Logging must be paused for the transfer** and resumed in a `finally` block,
  or a failure leaves the device not recording.

Chunks of `0x800` bytes are accepted. A 200 KB log is about 100 round trips.

### Record size comes from the block header

Each 64 KiB block starts with a `0x200` header whose bitmask says which fields
each record carries. A V1.13 unit can therefore produce 16-byte records (UTC +
lat + lon + height) or 20-byte records (with speed). Do not assume a size from
the firmware version.

Lat/lon are 4-byte floats, height is a 3-byte `u24`, and the per-record checksum
is one byte with no `'*'` separator.

## 6. Personal data

This repository is public. Before any commit that touches docs, tests or
examples:

* **No real coordinates.** Use `35.6812360, 139.7671250` or the no-fix
  placeholder `8960.0000,N,00000.0000,E`.
* **No local paths.** No home directories from any OS.
* **No serial-port instance names from a specific machine.** `COM3` in a usage
  example is fine; an enumeration of someone's ports is not.
* **Never commit** `holux/`, `vendor/`, `backup/` or device captures. All are
  gitignored; keep it that way.

## 7. Open questions

Recorded in `docs/dev/06-open-questions.md`. Do not fabricate answers:

* Where the log actually lives, given the DA reports a 512 KiB part while a unit
  held 212 KB of log plus a 406 KB image.
* Whether a Japanese-language firmware SKU exists.
* Bluetooth SPP was never exercised.
* The `0xD3` status byte's meaning beyond `0x00` = success.
