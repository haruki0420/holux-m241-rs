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

## 5. Raw device speed vs. point-to-point recomputed speed (ezTour parity)

`docs/dev/07-eztour-spec.ja.md` §4.2 establishes that HOLUX ezTour v3.0 did not
display the raw speed byte from the device log; `HLX_GetTrkInfo.dll` recomputed
speed and distance from the track point sequence (which also handles 16-byte log
records that carry no speed field).

Open because: this project has not yet decided whether track summaries and charts
should prefer the device's raw speed field when present and fall back to
recomputed speed, or recompute uniformly from point pairs.

## 6. What do ezTour's "one camera" and "selected photos" scopes mean?

`docs/dev/07-eztour-spec.ja.md` §6.1-7 records that `Shift photo time…` targets
every photo, the selected photos or one camera, but not how either narrower
scope is decided. `PhotoShiftScope` therefore compares exact values and invents
no normalisation:

- `ByCamera` reads the Exif IFD0 `Make` / `Model` tags and compares them after
  removing trailing NUL/space padding only. Invalid UTF-8 is an unreadable name,
  never decoded with replacement characters, because a lossy decode would make
  two different raw names compare equal. Which tag ezTour grouped on, and
  whether case or spelling differences fold together, is unknown.
- `SelectedPhotos` compares the paths exactly: a silently canonicalised path
  would hide the typo that made two spellings differ.
- A `PhotoTimeShift` day is 24 hours (86,400 s), not a calendar day; whether
  ezTour steps across a daylight-saving change by the calendar is unknown.
- Whether a real camera places `Make` / `Model` in IFD0 the way the reader's walk
  accepts is also unverified: every test JPEG in this repository is synthetic.

Open because: no ezTour UI capture, project file or help text settles any of
these.

## 7. How did ezTour's KMZ carry the `icon/` thumbnails?

`07-eztour-spec.ja.md` §6.3 says a KMZ carries photos under `photo/` and their
thumbnails under `icon/` (or as reduced copies). It does not record whether
ezTour wrote thumbnails there, how large they were, what they were named, or how
the KML referred to them.

`export_kmz_with_icons` takes the thumbnail size from the caller, uses the
`photo/` name and leaves the KML unchanged; `export_kmz` keeps writing no
`icon/` entries. None of those choices is verified against ezTour output.

Open because: no ezTour KMZ output has been inspected.
