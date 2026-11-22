# When a write fails

**Stay calm. Most failures are recoverable.**

The chip has a **BootROM** in mask ROM: a small program that cannot be erased and
that runs before the firmware. A failed firmware write damages the firmware; the
BootROM is still there. Power-cycle, and the chip comes up ready to accept a
write again.

The two exceptions are a wrong image and a power loss at the wrong moment. Either
can leave the unit unusable even with the BootROM intact.

## First, check the device

| What you see | What it means | What to do |
|---|---|---|
| It boots normally, date still wrong | the write failed, the old firmware is intact | run the flash step again |
| Nothing on the screen | the write stopped part-way | the recovery steps below |
| Garbled or partial display | the image is incomplete | the recovery steps below |
| No port appears in `m241 device ports` | the cable or the driver | replug, try another USB port |

## Recovery

1. **Remove the battery for at least ten seconds.** This is what puts the chip
   back in BootROM waiting state. A short press of the power button is not the
   same thing.
2. Put the battery back, switch on, and reconnect.
3. Write your own backup back with the explicit recovery mode:

   ```bash
   m241 fw flash backup/stock.bin --da vendor/DA_v3.8.4.bin --backup backup/stock.bin --recover --yes
   ```

   `--recover` requires the image to be byte-identical to `--backup` and skips
   the whole-unit comparison, because a partial write has already changed the
   unit. It still checks the M-241 vector and family markers and keeps the
   confirmation, layout and DA gates. This is the only supported path for a
   partial unit: without `--recover`, `verify_device` refuses a unit that no
   longer matches the backup. If the vector or family marker is itself damaged,
   `--recover` refuses rather than writing blind; that state is not recoverable
   by this build. A successful recovery leaves the unit in the
   BootROM, so remove the battery for about ten seconds, put it back, and switch
   it on before using the logger.

4. If it still will not answer, try assuming the chip is already in BootROM:

   ```bash
   m241 fw dump --assume-brom -o backup/probe.bin
   ```

5. If that reads, go back to step 3. If nothing answers at all, the BootROM is not
   running and no software can reach it: remove the battery for thirty seconds,
   try another USB port, and if it is still silent the unit has a hardware fault.

## Why this works

```
  chip
  +---------------------------------------+
  |  BootROM  (mask ROM, cannot be erased)|  <- always survives
  +---------------------------------------+
  |  firmware (rewritable)                |  <- what a failed write damages
  +---------------------------------------+
```

Only the lower layer is ever written, so the code that accepts a write is never
at risk.

## After recovery

`m241 device info` should print the release, build and model. If it does, the
device is healthy; the date will be wrong again unless you reapply the patch.

## Filing a report

Open an issue with the output of `m241 device doctor` and `m241 fw patch
--dry-run`, plus which step failed and the full error text.

**Do not attach a track log.** It contains your location history. A firmware dump
has no coordinates but does carry per-unit fields, so send its size and SHA-256
rather than the file.
