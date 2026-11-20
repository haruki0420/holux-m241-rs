# Open questions

Recorded so nobody fabricates an answer. Each of these is a real gap, not a
placeholder for work already done.

One question has been answered and removed: the `0xD3` status byte, which used to
be §3. `docs/reference/01-da-protocol.md` §4.1 carries the answer — the byte is
always `0x00`, and it is not sent at all when the command is refused. That page
still says "§3" when it closes the question; the number is the old one, before
this renumbering.

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

## 3. Was Bluetooth SPP ever exercised?

No. Every hardware session went over USB.

## 4. Can the reset-line entry be made safe?

`MtkBootRom` refuses to assert DTR/RTS when it cannot tell whether NMEA is
flowing. That refusal is correct but means a unit that answers neither NMEA nor
`0xA0` cannot be recovered in software.

Open because: knowing whether the firmware is running without a serial read would
need another signal — a current measurement, or a GPIO on the cable.
