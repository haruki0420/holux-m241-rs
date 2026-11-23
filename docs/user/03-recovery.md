# When a write fails

**Most failures are recoverable.** The MT3318 BootROM is mask ROM and cannot be erased; power-cycling returns the chip to a state ready to accept a backup flash (except a wrong image or power loss at a fatal moment).

## Quick triage

| Symptom | Meaning | Action |
|---|---|---|
| Boots normally, date still wrong | Old firmware intact | Re-run flash step |
| Blank or garbled screen | Partial write | Follow Recovery below |
| No port in `m241 device ports` | Cable or driver | Replug / try another USB port |

## Recovery steps

1. **Remove the battery for ≥10 seconds** (a power-button press is not enough), reinsert, switch on, and reconnect.
2. Flash your own backup back with `--recover`:

   ```bash
   m241 fw flash backup/stock.bin --da vendor/DA_v3.8.4.bin --backup backup/stock.bin --recover --yes
   ```

   `--recover` requires an image byte-identical to `--backup` and skips `verify_device` while keeping M-241 vector/family checks, layout, and DA gates. (If vector/family markers are damaged, `--recover` refuses.) After success, remove the battery for ~10 seconds and power on.
3. If the unit does not answer, test BootROM directly:

   ```bash
   m241 fw dump --assume-brom -o backup/probe.bin
   ```

   If it reads, return to step 2. If completely silent after a 30-second battery pull and another USB port, the unit has a hardware fault.

## After recovery & reporting

Run `m241 device info` to confirm firmware identity (dates remain unpatched until you re-apply the patch). If filing an issue, include `m241 device doctor` and `m241 fw patch --dry-run` output plus the full error — **never attach track logs or firmware dumps** (share size and SHA-256 only).
