# Fixing the date

Four steps — only step 4 can damage the device:

```bash
m241 device ports                      # 1. find the cable
m241 fw dump -o backup/stock.bin       # 2. save your firmware (read-only)
m241 fw patch backup/stock.bin -o backup/fixed.bin --dry-run
m241 fw patch backup/stock.bin -o backup/fixed.bin
m241 fw flash backup/fixed.bin --da vendor/DA_v3.8.4.bin --backup backup/stock.bin --yes
```

## Step 2: Dump twice across a power-cycle (read-only)

`fw dump` (and GUI 吸い出す) reads firmware twice and saves only when both SHA-256 hashes match. Between passes, remove the battery for ~10 seconds, reinsert, switch on, and press Enter.

## Step 3: Check the patch plan

`--dry-run` writes nothing and should list exactly three changed bytes: `0x402F6`, `0x402F7` (epoch constant) and `0x59C2C` (version digit). Stop if any other offset appears.

## Step 4: Flash (destructive)

Read [`docs/dev/05-verification.md`](../dev/05-verification.md), [`docs/reference/01-da-protocol.md`](../reference/01-da-protocol.md), and [`docs/dev/11-safety-review.md`](../dev/11-safety-review.md) first.

Before erasing, `fw flash` compares the unit's full firmware against `--backup` (`verify_device`), verifies the image differs only at profile patch offsets (strict path; `--allow-vendor` is separate), validates the pinned layout (`0x00000000`, `406620` B) and pinned `DA_v3.8.4.bin` SHA-256, and requires `--yes` (`AGENTS.md` §2.5). After writing, it sends `0xD9`, re-enters BootROM via NMEA/`$PMTK180*3B`, and reads back the full range.

## Step 5: Verify the date

Read-back leaves the unit in BootROM. Remove the battery for ~10 seconds, reinsert, power on, reconnect, then check:

```bash
m241 device info
m241 nmea proxy --stdout
```

Check `$GPRMC` field 9 (`ddmmyy`). If the date is still 19.6 years behind, re-flash your backup via [recovery](03-recovery.md). Note: post-flash OLED appearance and live GPS-fix dates remain unverified (`docs/dev/05-verification.md` §2).
