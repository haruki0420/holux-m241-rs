# Fixing the date

Four steps. Only the last one can damage the device.

```bash
m241 device ports                      # 1. find the cable
m241 fw dump -o backup/stock.bin       # 2. save your firmware (read-only)
m241 fw patch backup/stock.bin -o backup/fixed.bin --dry-run
m241 fw patch backup/stock.bin -o backup/fixed.bin
m241 fw flash --da vendor/DA_v3.8.4.bin --fw backup/fixed.bin --backup backup/stock.bin
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

> **Status: 未検証 / 未実装.** `m241 fw flash` is not implemented in this build
> yet; `crates/m241-flash/src/da.rs` still refuses to operate while the write
> path is reviewed against `docs/reference/01-da-protocol.md` and
> `docs/dev/11-safety-review.md`.

When implemented, the tool refuses unless all three of these hold:

1. the image matches a known firmware profile, or every byte that differs from
   `--backup` falls inside the patch offsets;
2. the image fits the flash size the device reports;
3. you typed `WRITE`.

Per `docs/dev/11-safety-review.md` and `docs/dev/04-prior-art.md`, the v2 flash
design also re-enters the BootROM to read back and verify the written bytes, and
you should confirm the live NMEA date in Step 5 below.

## Step 5: check the date

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
