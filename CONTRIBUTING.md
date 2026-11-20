# Contribution guide

This project repairs and reads a discontinued GPS logger. The most valuable
contributions are hardware reports and firmware profiles, not features.

## Before you change anything

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

All three must be clean. The workspace denies `unsafe`, `unwrap`, `expect` and
`panic`, so a serial failure becomes an error value rather than a process abort.
That is not style: the device may be mid-write when a cable is pulled, and the
recovery instructions have to survive it.

## The one rule that is not negotiable

**The flash path is rebuilt from the vendor binary's disassembly, not ported.**

`DA_v3.8.4.bin` is being disassembled, and its command semantics are recorded in
`docs/reference/01-da-protocol.md` before any code implements them. Two things
follow:

- A change to `crates/m241-core/src/protocol/da.rs` needs a citation into that
  document, naming the function and the instruction range it came from.
- The previous Python implementation and the upstream project are **hypotheses**.
  Where they disagree with the disassembly, the disassembly wins, and the
  disagreement is recorded in `docs/dev/04-prior-art.md` so it is not re-litigated.

Do not "simplify" a safety gate. The three that exist each stopped a real
failure: the NMEA pre-probe, the backup diff check, and the typed `WRITE`.

## Safety gates

| Gate | Where | Why |
|---|---|---|
| NMEA pre-probe | `protocol::brom` | Pulsing DTR/RTS at a streaming firmware leaves the unit unresponsive until the battery is removed |
| Backup diff check | `firmware::patch` | Any image whose differing bytes fall outside the patch offsets is refused |
| Typed `WRITE` | `cli`, `gui` | A confirmation dialogue is clicked by reflex; typing is not |

## Adding a firmware profile

A profile is measured facts about one image: size, SHA-256, version marker
offset, epoch offset. It belongs with the other profiles in
`crates/m241-core/src/firmware/`, with a test that asserts the hashes.

```bash
m241 fw dump --port auto -o backup/mine.bin
m241 fw patch backup/mine.bin --dry-run
```

Open an issue with that output. It contains hashes and offsets only, never
firmware bytes.

## Documentation

- `docs/reference/` is normative. If code and reference disagree, one of them is
  a bug; fix both in the same commit.
- `docs/dev/04-prior-art.md` records what was taken from where. Keep it honest.
- `docs/dev/05-verification.md` separates what was measured on hardware from
  what was only tested against the simulator. Never move a claim across that
  line without a hardware run.

## Commits

Conventional prefixes: `feat:`, `fix:`, `docs:`, `chore:`, `test:`. Say what was
measured and what it replaced. For protocol fixes, name the byte sequence and the
symptom it caused.

## Personal data

Track logs are personal location data. Never commit one, never paste coordinates
into an issue, and never add a test fixture built from a real recording. Use
`35.6812360, 139.7671250` (a city centre) or the no-fix placeholder
`8960.0000,N,00000.0000,E`.
