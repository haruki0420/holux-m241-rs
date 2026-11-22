# Fixing the date

Four steps. Only the last one can damage the device.

```bash
m241 device ports                      # 1. find the cable
m241 fw dump -o backup/stock.bin       # 2. save your firmware (read-only)
m241 fw patch backup/stock.bin -o backup/fixed.bin --dry-run
m241 fw patch backup/stock.bin -o backup/fixed.bin
m241 fw flash backup/fixed.bin --da vendor/DA_v3.8.4.bin --backup backup/stock.bin --yes
```

## Step 2 asks you to power-cycle

The dump reads your firmware twice and compares the hashes, because a single read
that silently truncated would look like success. Between the passes the tool stops
and asks you to remove the battery for about ten seconds, put it back, switch on,
and press Enter. A second read from a fresh boot is what makes the comparison
mean something.

**This step never writes to the device.** It can be repeated as often as you like.

## Step 3 shows what will change

`--dry-run` prints the plan and writes nothing. Expect three changed bytes:

```
0x402F6, 0x402F7   the two high bytes of the date constant
0x59C2C            one digit of the version string
```

A fourth offset means the image is not the version this tool expects. Stop.

## Step 4 is the only dangerous step

> **Status: implemented, not verified on hardware.** `m241 fw flash` is wired
> through the DA write and read-back, but this build has not flashed a unit.
> Read `docs/dev/05-verification.md` before running it, and review the sequence
> against `docs/reference/01-da-protocol.md` and
> `docs/dev/11-safety-review.md`.

The command first compares the unit's whole firmware span with `--backup`
(`DownloadAgent::verify_device`). The library's `DownloadAgent::write` does not
call that check itself: it compares the image with `backup`, not with the unit.
The device check belongs to the CLI route, not to the library `write`. After
that it refuses unless all three of these hold:

1. the image matches a known firmware profile and is either identical to
   `--backup` or differs from it only at that profile's patch offsets. The
   strict path has no vendor-package or cross-model exception; the explicit
   `--allow-vendor` gate (fixed V1.13 SHA-256) is separate and is not used here;
2. the layout gate accepts the pinned start `0x00000000` and length `406620`;
3. you passed `--yes` (this build has no typed `WRITE` prompt; `AGENTS.md` §2.5
   governs who may pass it).

The write sends `0xD9`, then re-enters the BootROM with the same NMEA probe and
`$PMTK180*3B` hand-over the first entry used, and reads the range back. If the
re-entry fails, the command reports that the read-back was not performed; it
never reports success. Confirm the live NMEA date in Step 5 as well.

## Step 5: check the date

The read-back leaves the unit in the BootROM. Remove the battery for about ten
seconds, put it back, switch the unit on, and reconnect before the commands
below; otherwise `device info` and the NMEA proxy will find no firmware.

```bash
m241 device info
m241 nmea proxy --stdout
```

The ninth field of `$GPRMC` is the date as `ddmmyy`. It should be today. If it is
still nineteen years ago, the write did not take; re-flash your backup and read
[recovery](03-recovery.md).

## What is not verified

The display on the device is expected to show the corrected date, because it
reads the same conversion result, but nobody has photographed it after a flash.
`docs/dev/05-verification.md` keeps that list.
