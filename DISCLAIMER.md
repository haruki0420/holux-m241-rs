# Disclaimer — read before you flash anything

**Short version: you can permanently destroy your Holux M-241. Do not run the
flash step unless you accept that.**

## 1. This is firmware modification of an abandoned device

The M-241 was discontinued years ago and Holux left the GPS business in 2019.
There is no manufacturer support, no service centre, no replacement mainboard,
and **no JTAG header on the PCB**. If you hard-brick the unit, the only recovery
is chip-level work on the MT3318 BGA package. Practically, a hard brick means the
device is scrap.

## 2. What can go wrong

| Failure | Consequence |
|---|---|
| Power lost or cable unplugged mid-write | Flash left half-erased: **soft brick**, usually recoverable by re-flashing a full dump |
| Wrong start address | Firmware written to the wrong region: unbootable |
| Wrong image for your hardware | Unbootable; possibly a hard brick |
| Writing to `0x00100000`–`0x001007FF` | The BootROM's own stack. The chip dies instantly. This is why the download agent loads at `0x00100800` |
| Interrupting the download agent load | BootROM session dies; power-cycle and retry, usually fine |
| Battery dies during a log download | Log data may be lost; the device is fine |
| Hard brick | No JTAG, so the unit is permanently unusable |

The BootROM is mask ROM and cannot be erased, so a failed write is usually
recoverable by power-cycling and re-flashing your own backup. A wrong image, or a
power loss at the wrong moment, can still leave the unit permanently unusable.

## 3. Pre-flight checklist

1. **Dump your own firmware first.** The tool reads it twice and refuses to save
   unless both hashes match. Keep that file, read-only, somewhere safe. It is
   your only rollback.
2. **Verify the dump matches a known profile.** `m241 fw patch --dry-run` prints
   the detected profile. If detection fails, stop. Do not force it.
3. Use a known-good USB cable and a machine that will not sleep, update, or lose
   power mid-write. On a laptop, plug in the charger.
4. Fresh battery in the M-241.
5. Close every other program that might open the serial port.
6. **Only flash an image you generated yourself** from your own dump. Do not
   flash an image you found on the internet, including one from this project's
   users.

## 4. Warranty, liability, law

The software is provided **as is**, without warranty of any kind, under the MIT
licence. The authors are not liable for any damage to hardware, data or property,
nor for any legal consequence of your use.

Modifying firmware for personal repair is lawful in many jurisdictions, but not
all. You are responsible for knowing the rules where you live.

## 5. Resale

A patched unit is a **modified** unit. Selling it as original or unmodified is
fraud. The default patch rewrites the on-device version string to `V1.14` so a
modified unit is identifiable at a glance.

## 6. What this project does not do

- It does not ship proprietary binaries, firmware images or vendor tools.
- It does not modify your device without an explicit, typed confirmation.
- It does not collect telemetry or upload your logs. Everything runs locally; the
  only network feature is the optional NMEA relay, which binds to loopback by
  default.

## 7. If you are not comfortable with the above

Don't flash. The alternatives are non-destructive:

- Post-process logs with a `+7168 days` correction (`m241 log read
  --utc-offset-days 7168`).
- Correct the live NMEA stream on your computer (`m241 nmea proxy`). The device's
  own display stays wrong, but every connected application sees the right date.

Neither can brick anything.
