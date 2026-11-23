# Contribution guide

Hardware reports and firmware profiles are the highest-value contributions.

## Checks before every commit

Run `./tools/check.sh` and require **6/6** (`fmt`, `clippy`, `test`, `docs`, `design`, `safety`). The workspace denies `unsafe`, `unwrap`, `expect`, and `panic` outside tests so serial errors return recoverable values instead of aborting mid-write.

## Non-negotiable rule: disassembly over porting

Every constant and command in `crates/m241-flash/src/da.rs` must cite a function and instruction range in [`docs/reference/01-da-protocol.md`](docs/reference/01-da-protocol.md). Prior Python implementations are hypotheses; disagreements are recorded in [`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md).

Never weaken safety gates:
- **NMEA pre-probe** (`protocol::brom`): prevents hanging running firmware with DTR/RTS.
- **Backup diff check** (`firmware::patch`): refuses changes outside profile patch offsets.
- **`--yes` / `WRITE` confirmation** (`cli`, `gui`): deliberate consent (`AGENTS.md` §2.5).

## Adding a firmware profile

Run `m241 fw dump --port auto -o backup/mine.bin` and `m241 fw patch backup/mine.bin --dry-run`, then open an issue with the output (hashes and offsets only — never firmware bytes). Profiles live in `crates/m241-core/src/firmware/` with hash tests.

## Documentation & pull requests

- `docs/reference/` is normative; update code and reference together.
- Only [`docs/dev/05-verification.md`](docs/dev/05-verification.md) moves a claim from unverified to hardware-verified.
- Follow the three-stage independent review protocol and Tier 1/2/3 rules in [`docs/dev/09-review-protocol.md`](docs/dev/09-review-protocol.md). Every PR requires an independent reviewer's `review-gate` status on the exact head SHA plus 6/6 `./tools/check.sh` and CI.

## Commits & personal data

Use Conventional prefixes (`feat:`, `fix:`, `docs:`, `chore:`, `test:`), naming byte sequences and symptoms for protocol fixes. Never commit real track logs, coordinates, local paths, or vendor binaries — use `35.6812360, 139.7671250` or `8960.0000,N,00000.0000,E`.
