# Open questions

Recorded so nobody fabricates an answer. Each of these is a real gap, not a
placeholder for work already done.

One question has been answered and removed: the `0xD3` status byte, which used to
be §3. `docs/reference/01-da-protocol.md` §4.1 carries the answer — the byte is
always `0x00`, and it is not sent at all when the command is refused. That page
still says "§3" when it closes the question; the number is the old one, before
this renumbering.

One question is a decision rather than a measurement: §8 records the options for
the `--allow-vendor` read-back and leaves the choice to the user. Nothing in the
code or in the safety gate has been changed while the decision is open.

## 1. Where does the log actually live?

The DA reports a **512 KiB** flash part (MX29LV400CB) while a unit was measured
holding **212,860 bytes of log alongside a 406,620-byte firmware image**. Those
do not fit in 512 KiB, so the log area is not the chip the DA describes.

Open because: the flash geometry is only known from the DA report, and no
datasheet-level probe was done.

A further observation (2026-11-22, `docs/dev/05-verification.md` §1.5): after the
three writes, `m241 log download` returned 3,228 bytes and then 3,260. The bytes
hold only the settings / state journal — no track point at all. The parser that
ran then read the journal as 20-byte records, which is why every record checksum
failed and the decoded dates were nonsense; that was a parser misread, not
damage. PR #34 (merged) recognises and skips the journal: re-parsing the same
dumps gives 0 points with `journal_only: true`, and the pre-write 2026-11-21 dump
gives 10,942 points with 0 record-checksum mismatches (the old parser had
exported 4,481 of its 11,088 points from checksum-failing bytes). The write
therefore did not damage the log area, as far as these dumps show.

What remains open in this area:

* **Whether the firmware writes any track point with no fix.** The post-write
  dumps are journal-only, so this is 未検証 with or without a fix.
* **Where the log area physically lives** — the question this section is named
  for.
* **The shape on other firmware / units.** V1.12 was never dumped, and the
  4 MiB units and GR-245 variants were never available.

What would settle it: a full-chip read past `0x80000`, or a schematic.

## 2. Does a Japanese-language firmware SKU exist?

No. The release notes list TC / SC / DE / FR / EN, and the 41-entry string table
is byte-identical between V1.12 and V1.13, with no Japanese. Manuals exist in
Japanese; firmware does not.

The evidence is the vendor release packages inspected here (V1.12 and V1.13);
no Japanese-market unit, box or image was seen. The statement is therefore "no
Japanese firmware in the images inspected", not "Holux never shipped one".

Treat as closed unless an image turns up that contains Japanese strings.

## 3. Was Bluetooth SPP ever exercised?

No. Every hardware session went over USB, including the 2026-11-22 GUI session
(`docs/dev/05-verification.md` §1.5). No Bluetooth pairing, SPP socket or SPP
transfer was attempted.

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

## 8. Should `--allow-vendor`'s read-back skip the firmware-managed per-unit bytes?

This one is waiting on a decision, not a measurement. Nothing has been changed in
the code or in the safety gate: the merged behaviour is C.

What the 2026-11-22 hardware session established
(`docs/dev/05-verification.md` §1.5):

* `fw flash --allow-vendor` wrote the stock V1.13 image (406,620 bytes) in full,
  all 1,589 packets acknowledged, and the unit booted it (`device info`
  answered; the NMEA default date was `06-01-80`, the unpatched epoch's value).
* The read-back compared the whole 406,620-byte range and differed at exactly
  one offset: the unit held `0x91` at `0x5C`, where the vendor image has `0xFF`.
* A fresh dump after that boot shows the unit had restored `0x5C`–`0x5F` to its
  own earlier values (`91 1B D4 CF`). Those four bytes are inside the 14 bytes
  of per-unit data the vendor gate warns it overwrites.
* The other ten per-unit bytes (`0xF8`–`0xFD`, `0x63458`–`0x6345B`) were **not**
  restored: after the vendor write they held the vendor image's values.
* The read-back runs after `0xD9` restarts the chip, the firmware boots, and the
  BootROM is re-entered with `$PMTK180*3B`. The firmware therefore runs between
  the write and the comparison, so at the four restored offsets the compared
  byte is the firmware's value, not the byte the write programmed.
* Where the firmware gets those four bytes (a second copy in the chip, or a
  derivation) is not established.

`DownloadAgent::verify_written` stops at the first differing offset and returns
an error, so today a completed vendor write reports a read-back failure at
`0x5C` together with the recovery advice.

### A. Exclude the four firmware-managed offsets from the vendor read-back

Compare every byte except `0x5C`–`0x5F`, only on the vendor-token path
(`write_vendor`), never on the strict backup/patch path and never for
`--recover`. The four offsets would be a constant in `m241-flash`, not a flag, a
file or a user value.

* **What is excluded, and why that is defensible.** The firmware rewrites exactly
  those four bytes during the boot that sits between the write and the
  read-back, so the comparison there cannot pass for any correct vendor write: it
  tests the firmware's restore, not the write. The remaining 406,616 bytes —
  including the ten per-unit bytes the firmware did not restore — are still
  compared.
* **What it gives up.** The contents of those four bytes would no longer be
  checked; the only evidence that the write reached them is the packet
  acknowledgements and the unit booting.
* **Remaining risk.** The restore's origin is unknown. If the firmware derives
  the bytes rather than copying them from a second copy, excluding them could
  hide a defect in the state it reads. A write that failed only in that range
  would be accepted.
* **Safety gate.** It removes four bytes from one comparison. That is a narrow
  but real relaxation, and it is the user's decision.
* **`AGENTS.md` §2.3.** Would need a qualification: for `--allow-vendor`, the
  written range is read back and compared **except** the four firmware-managed
  offsets. Leaving §2.3 as it is while changing only the code would make the code
  and the gate disagree.
* **What a test can pin.** 🧪 only: the fake could restore `0x5C`–`0x5F` between
  the write and the read-back; a vendor-path write would then accept a
  difference at those four offsets and refuse one anywhere else, while the strict
  and `--recover` paths still refuse a `0x5C` difference. No test shows what the
  real firmware restores.

### B. Judge success by the pre-write comparison alone; read-back is information

Success becomes "the unit matched the backup before the DA was loaded, and `0xD5`
acknowledged every packet"; the read-back is printed but does not fail the
command.

* **Safety gate.** The read-back is the only post-write proof that the intended
  bytes are in flash. Demoting it means `fw flash` can report success on a unit
  that differs from the image. Applied to every write it also leaves `--recover`
  with no success criterion at all: `--recover` skips `verify_device` by design
  (`docs/dev/05-verification.md` §1.5), so its only comparison is the read-back.
  Scoping B to `--allow-vendor` avoids that, but still drops the only check on the
  ten per-unit bytes and on the rest of the image.
* **`AGENTS.md` §2.3.** Would need a change: §2.3 says the written range "is read
  back and compared with the vendor image", and B redefines what success means.
* **What a test can pin.** 🧪 only: exit status and report wording against the
  fake. Nothing pins whether a unit that was not read back is correct.

### C. Keep the gate and document the expected mismatch

No code change. The pages say that a **correct** vendor write can end with a
read-back mismatch at `0x5C`–`0x5F`, why, and that the way back to the unit's own
per-unit bytes is `fw flash --recover` with the pre-vendor backup (the image must
be byte-identical to the backup, which `--recover` checks; it skips
`verify_device`, so the unit's current difference from that backup is expected).

* **Safety gate.** Unchanged: every read-back difference still fails the command.
  The cost is availability — a successful vendor write reports failure, and the
  operator has to read the offset and know it is expected.
* **`AGENTS.md` §2.3.** No change needed; the comparison still runs and the
  outcome is documented.
* **What a test can pin.** No new code, so nothing beyond the existing mismatch
  reporting; the docs gate only checks links. 🧪
* **Operational caveat.** The `--recover` path was measured on 2026-11-22, but
  after the 99%-complete write, not after a vendor write; restoring a pre-vendor
  backup over a vendor-written unit has not been run.

### Recommendation

**A**, with B rejected and C as the state until the user decides. A is the only
option that keeps the read-back as a post-write check: every byte the firmware
does not rewrite is still compared, and only the comparison that cannot pass is
removed. It is a relaxation of `AGENTS.md` §2.3, so it needs the user's decision;
establishing the origin of the four restored bytes first would turn A's "what it
gives up" paragraph into a measured statement.

A fourth route exists and is not one of the three. The DA is still in its command
loop after `0xD5`, so a `0xD6` read **before** `0xD9` would see the programmed
bytes without a firmware boot and without the restore. It is not ready: only one
`0xD6` page was ever read on hardware, multi-page reads are unmeasured, and it
would add a new read path to the write crate
(`docs/reference/01-da-protocol.md` §7 item 4, `docs/dev/05-verification.md` §2).

## 9. Where does the no-fix clock get its value?

Measured 2026-11-22 on the patched V1.14 image with no fix
(`docs/dev/05-verification.md` §1.5): the unit reported the then-current UTC date
and time in every `$GPRMC` / `$GPGGA` of the window, through `m241 nmea proxy`
with no offset applied, and in a separate pass about 46 s earlier. Right after
the patched write, with no fix, the same unit had reported `22-08-99` (the value
the patched epoch `935280000` produces when there is no clock), and the stock
image reports `06-01-80` (its epoch `315964800`), so the sentence carries a clock
value, not only the constant.

Open because: whether that clock is a free-running RTC, a value retained from an
earlier GPS fix, or something set over the serial link is not established. The
unit was not observed from a known-unset clock to this state, and no unpatched
image was observed with a running clock, so how much of the correct date comes
from the patch and how much from the clock's current value is not separated. The
date from a **live fix** after patching is also still unobserved.

What would settle it: an unpatched image observed with a running clock; a
power-cycle after a known interval with no fix and no host interaction, reading
the date before any fix; and whether any protocol path can set the time (none is
known).

## 10. Which GUI paths have been run against a unit?

The Connect screen and 「接続して確認」 ran against the unit on 2026-11-22
(`docs/dev/05-verification.md` §1.5): the probe read the firmware identity and the
log usage over USB, and the screen showed 接続済み（読み取り専用）. The attached
capture was cut off at the SDK line, so the 状態 row drawn under it is not in the
capture; the code draws that row after the SDK line but this page does not record
it as observed.

Open because three paths were not exercised: the Log screen (its file-path field
needs keyboard input, and no keyboard could be injected into the X11 window in
that session), the 追従 render (it needs a log loaded through that field), and a
photo geotag / JPEG write through the GUI (it needs a real camera JPEG and a
photo folder; every test JPEG here is synthetic). The Log screen also carries the
KMZ writer, the photo clock shift and track editing wired by PR #31; because the
screen itself could not be opened, none of them was exercised either, and no KMZ
output has been opened in Google Earth or `unzip`.

What would settle it: a session that opens a log file in the GUI and writes a
real camera JPEG through it, plus a check that the screens are reachable without
a pointer.
