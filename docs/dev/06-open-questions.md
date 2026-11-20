# Open questions

Recorded so nobody fabricates an answer. Each of these is a real gap, not a
placeholder for work already done.

## 1. Where does the log actually live?

The DA reports a **512 KiB** flash part (MX29LV400CB) while a unit was measured
holding **212,860 bytes of log alongside a 406,620-byte firmware image**. Those
do not fit in 512 KiB, so the log area is not the chip the DA describes.

Open because: the flash geometry is only known from the DA report, and no
datasheet-level probe was done.

What would settle it: a full-chip read past `0x80000`, or a schematic.

## 2. Does a Japanese-language firmware SKU exist?

No. The release notes list TC / SC / DE / FR / EN, and the 41-entry string table
is byte-identical between V1.12 and V1.13, with no Japanese. Manuals exist in
Japanese; firmware does not.

Treat as closed unless an image turns up that contains Japanese strings.

## 3. What does the `0xD3` status byte mean beyond `0x00`?

`0x00` is success. No non-zero value was ever observed, because every write this
project performed succeeded.

What would settle it: a deliberate failure (write to a protected range) on a unit
that is already a soft brick.

## 4. Was Bluetooth SPP ever exercised?

No. Every hardware session went over USB.

## 5. Can the reset-line entry be made safe?

`MtkBootRom` refuses to assert DTR/RTS when it cannot tell whether NMEA is
flowing. That refusal is correct but means a unit that answers neither NMEA nor
`0xA0` cannot be recovered in software.

Open because: knowing whether the firmware is running without a serial read would
need another signal — a current measurement, or a GPIO on the cable.
