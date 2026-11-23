# Disclaimer — read before you flash anything

**You can permanently destroy your Holux M-241. Do not flash unless you accept that risk.**

## 1. Abandoned hardware, no JTAG

Holux left the GPS business in 2019; there is no support, replacement board, or **JTAG header**. A hard brick requires BGA-level rework and effectively scraps the unit.

## 2. Failure modes

| Failure | Consequence |
|---|---|
| Power loss / cable pull mid-write | Soft brick (usually recoverable by re-flashing your backup; BootROM is mask ROM) |
| Wrong start address / wrong image | Unbootable; possibly hard brick |
| Writing `0x00100000`–`0x001007FF` | Destroys BootROM stack instantly (DA loads at `0x00100800`) |
| Interrupted DA load / log download | Power-cycle and retry (log data may be lost; unit fine) |

## 3. Pre-flight checklist

1. **Dump your own firmware first** (two matching hashes) and keep it read-only as your rollback.
2. Confirm a known profile with `m241 fw patch --dry-run`; stop if unrecognised.
3. Use a reliable USB cable, fresh battery, AC power on laptops (disable sleep), and close other serial apps.
4. **Flash only an image generated from your own dump** — never third-party images from the internet.

## 4. Warranty, law & resale

Provided **as is** under MIT with no warranty or liability. You are responsible for local laws on personal firmware repair. A patched unit is **modified** (marked `V1.14`); selling it as unmodified is fraud.

## 5. Privacy & non-destructive alternatives

No proprietary binaries are shipped; no device is modified without explicit confirmation; logs stay local. Only optional `nmea proxy` (loopback default) and background OpenStreetMap tiles (on by default, switchable off in Settings) use the network.

If you prefer zero brick risk, skip flashing and use `m241 log read --utc-offset-days 7168` or `m241 nmea proxy`.
