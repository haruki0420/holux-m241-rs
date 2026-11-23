# Working notes for automated contributors

Read before touching `crates/m241-core/src/protocol/`.

## 0. How to work here

- **Check command before every commit**: `./tools/check.sh` — runs 6 gates (`fmt`, `clippy`, `test`, `docs`, `design`, `safety`).
- **Tauri frontend (`crates/m241-app`)**: embeds `ui/dist` at compile time. `check.sh` auto-runs `pnpm --dir crates/m241-app/ui install --frozen-lockfile && pnpm --dir crates/m241-app/ui build` when `ui/dist/index.html` is missing (pnpm only, never npm/npx). If pnpm is missing/fails, `m241-app` is excluded with `[SKIP] m241-app: ui/dist missing: …` (5/6 + explicit skip, never claiming 6/6).
- **Design gate (`tools/check_design.py`)**: enforces 3 `DESIGN.md` invariants — amber only in destructive module, corner radius always `theme::RADIUS`, status colour paired with text.
- **Safety gate (`tools/check_safety.py`)**: keeps reading (`m241-core`) and writing (`m241-flash`) apart. Fails if destructive commands (`$PMTK182,6`, `0xD3`/`0xD4`/`0xD5`/`0xD9` incl. `\xNN`) or write identifiers appear in `m241-core` (`src/`/`tests/`), if `m241-core` depends on `m241-flash`, or if `set_control_lines` appears outside exact BootROM entry/port/simulator paths. Raw transport/framing primitives (`Port::open`, `Transport::write_all`, `pmtk::line`, `Frame`) remain public in `m241-core` for `m241-flash`.
- **GUI writes (user decision, 2026-11-23)**: `m241-gui` intentionally links `m241-flash` and routes writes through `m241_flash::flow::flash`, gated by typed `WRITE` (Firmware) / `PROBE` (Connect), with `Confirmation::granted` / `Job::Flash` restricted to those screens and destructive opcodes forbidden in GUI code (prose allowed). This text gate is deliberately weaker than the old dependency-graph rule; structural guarantees remain inside `m241-flash` (fixed range, `check_layout`, `check_image`, `verify_device`, full read-back). See [`docs/dev/11-safety-review.md`](docs/dev/11-safety-review.md).
- **Network use (map tiles only, two places)**: egui Log screen (`crates/m241-gui/src/screens/log/tiles.rs` via `TileFetcher`; headless/`--screenshot`/sample never fetch, gated by `native_pixels_per_point`) and Tauri WebView (`crates/m241-app`, MapLibre to `tile.openstreetmap.org` in `tauri.conf.json` CSP, persisted toggle). All other commands are offline; `nmea proxy` binds loopback by default.
- **Discipline**: run long `cargo build`s in background; never bulk-edit files with scripts (`git mv` + editor tools only); never claim unverified hardware behaviour (only `docs/dev/05-verification.md` moves 未検証 → 検証済み); a subagent request is a tool call with a session id; try software recovery (`fw da-restart` `0xD9`, `$PMTK180`, `--assume-brom`) before physical power cycle; simulator is a regression pin, not a hardware oracle; read §9 before merging.

## 1. What this project is

Cross-platform (Windows / Linux / macOS) tool for **Holux M-241** (MediaTek **MT3318**, firmware `B-core_1.20`, model `01017-00F`):
1. **Fix GPS week-number rollover**: replace 4-byte epoch constant at `0x402F4` (`315964800` → `935280000`; **3 bytes change**; fixes 7,168-day offset).
2. **Download, convert, edit, and geotag track logs**.
3. **Repair or relay live NMEA streams**.

Runtime dependencies: Rust std, `serialport`, and `image` (`jpeg` feature only for KMZ long-edge reduction). Offline commands never open a port.

## 2. Non-negotiable safety rules

### 2.1 Never pulse DTR/RTS at a logger streaming NMEA
Asserting reset lines at running firmware hangs the unit until battery removal. Entry checks for NMEA first and returns a tri-state: `true` (use `$PMTK180*3B`), `false` (definitively silent, reset route safe), `None` (**refuse**). Do not simplify away.

### 2.2 `0xD5` erases every sector the recorded range touches
`0xD3` (`0x00101F98`–`0x00102034`) only records a range; `0xD5` (`0x0010251C`–`0x0010252C` → `FUN_00100D5C`) erases each sector immediately before programming. `0xD4` is a separate destructive erase with `0x04 <pct>` progress frames (`docs/reference/01-da-protocol.md` §5.4).
- Range must be a hard-coded constant (never user input); refuse unexpected constants; verify intact marker before erase.
- Require explicit confirmation: user types `WRITE` (CLI/GUI) or passes `--yes` (CLI only). **An agent must never type `WRITE` into a prompt/PTY/GUI.** GUI has no `--yes` (source checks in `tools/check_safety.py`, not type-enforced). Agents may pass `--yes` to `fw flash` or `fw probe-da` only with §2.5 approval.

### 2.3 Take a dump before writing anything
Refuse any image differing from the user's backup outside profile patch offsets (identical image accepted). Do not weaken.

**Official vendor V1.13 gate (user decision, 2026-11-22)**: separate gate (`accept_vendor_image` → `VendorImage` → `write_vendor`), never a digest branch in `check_image`:
- Only stock V1.13: exact length **406,620 bytes**, full SHA-256 `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435` (V1.12 and all others refused).
- Requires `--allow-vendor`, `--yes`, `--backup`, `--da`, and `verify_device` against `--backup` + full read-back.
- Warns before/after that it overwrites 14 bytes of per-unit data and is **unpatched stock V1.13** (epoch `315964800`, 7,168 days behind). Inside §2.5.

### 2.4 Never commit personal data
See §6.

### 2.5 Who may approve a write-capable run
Covers `fw flash --yes` (incl. `--recover`, `--allow-vendor`), `fw probe-da --yes`, and GUI equivalents (`WRITE` / `PROBE`). By default requires the **user's own approval**. Subagent approval applies **only** when explicitly delegated by the user in that session; any human-approval requirement always overrides this section. `log erase` is **never** covered (requires user's own instruction).

**Pinned DA gate (`DaDigest::vendor()`)**: `--da` must be 14,896 bytes, start `00 00 A0 E1`, and match full SHA-256 `190d7064cf2320752d8eb65681142730b21cddc90e0d1491d4e8379c3f2e89aa` (enforced before opening port via `check_da_image_against` / `DaLoader::load_against` with `da_digest()`). Any other digest is unapprovable.

**Delegated approval requirements (per execution, independent non-forked reviewer, raw artifacts only — never preparer's summary; `docs/dev/09-review-protocol.md`)**:
- `fw probe-da --yes`: current-session `device info` log, resolved `--port` (never `auto`), `--da` path & full SHA-256, exact command line.
- `fw flash --yes`: above plus `fw dump` and `fw probe-da` logs confirming unit (`B-core_1.20`, `01017-00F`, `00C2:22BA`); independently recomputed backup/image/DA SHA-256s; byte-diff provenance (`PATCH_OFFSETS` `[0x402F6, 0x402F7, 0x59C2C]` for V1.13 `0x6345C`, or byte-identical for `--recover`, or pinned V1.13 SHA + user acknowledgement of 14-byte overwrite for `--allow-vendor`); settled `FLASH_WRITE_START = 0` in `docs/dev/05-verification.md` §1 and empty `UNRESOLVED_STARTS` in `crates/m241-flash/src/da.rs`; read-back & recovery plan; exact command verified against `m241 fw flash --help`.
- **Range invariance (refusal if unverified)**: range comes solely from `FLASH_WRITE_START = 0` and `FLASH_WRITE_LENGTH = 0x6345C`; `0xD3` records `0..=0x6345B`; retries keep exact range; image length == `0x6345C`; `check_layout` + `check_image`/vendor token run before first `0xD3`.
- Reviewer does not open the port or edit the branch, and **authors its own durable verdict record directly** (PR comment or committed record with SHA-256s, command line, session ID) **before** `--yes` is passed. Never write pre-write approvals into `05-verification.md` §1. If refused, fix root cause and re-gather fresh artifacts (no approver shopping).

## 3. Verified hardware facts — do not re-derive

| Fact | Value | Established by |
|---|---|---|
| Firmware base (BootROM view) | `0x00200000` | `0x20000000` → `42 20 02 02` (unmapped); `0x00200000` → `18 f0 9f e5 …` |
| DA load address | `0x00100800` | `0x100000`–`0x1007FF` is BootROM scratch (writing kills chip) |
| DA info report | `>BHBLHHHHL`, 20 B, starts `0xC0` | `C0 03 08 49 …`; 4 `H` = mfr `0x00C2`, dev `0x22BA`, `0`, `0` (ids, not geometry); trailing `L` from RAM |
| Retail flash | 512 KiB, MX29LV400CB, mfr `0xC2`, dev `0x22BA`, DA id `0x49` | DA info report |
| Firmware image size | 406,620 bytes (`0x6345C`) | read back & hashed |
| NMEA | 38400 8N1, `$GPGGA/$GPGSA/$GPGSV/$GPRMC/$GPVTG` at 1 Hz | measured |
| UI languages | EN / FR / DE (no Japanese) | release notes |

Log area is separate from the 512 KiB chip (212,860 B log measured alongside 406,620 B firmware; `docs/dev/06-open-questions.md`).

## 4. Protocol gotchas

- **4.1 `0xD3` reply width**: `5A 00` (2 B) on accept (`0x00102004`–`0x00102034`, status always `0x00`), single `A5` (1 B) on refusal (`0x0010202C`–`0x00102034`, ⚠️ 未検証 on hardware). Read 1 byte; read 2nd only if 1st is `0x5A`. Refused `0xD3` keeps prior ranges.
- **4.2 WRITE16 echo trails by 1 byte**: after N bytes only N−1 echo bytes arrive. Compare against `sent[..len-1]`, then drain tail.
- **4.3 Reads carry no checksum trailer**: `0xA2`/`0xAF` return echo + data only; checksum is separate `0xA4`.
- **4.4 Width byte orders**: `0xA2`/`0xA1` swap 16-bit pairs (`swap_pairs`); `0xAF` reverses each 32-bit word (`reverse_words32`).
- **4.5 Count units**: `0xAF` takes dwords; `0xA2`/`0xA1`/`0xA4` take words (`Frame::for_bytes`).
- **4.6 `$PMTK180*3B` needs a live NMEA stream**: probe once (`StreamState`), send at NMEA baud using `EntryTiming` (`crates/m241-core/src/protocol/pmtk.rs::enter_bootrom_after`), switch to 115200. Double-probing breaks handover. ✅ Measured 2026-11-22 (`05-verification.md` §1.5).
- **4.7 Four sync probes back to back**: `0xA0`→`0x5F`, `0x0A`→`0xF5`, `0x50`→`0xAF`, `0x05`→`0xFA` with no pause.

## 5. Log download (PMTK182)

Commands: `$PMTK182,2,8` (used hex), `$PMTK182,5` (pause), `$PMTK182,7,<hex_addr>,<hex_len>` (read, `0x800` chunks), `$PMTK182,4` (resume in `finally`), `$PMTK182,6` (erase). Address/length are hex without `0x`.

Each 64 KiB block has a `0x200` header mask (16 B or 20 B records; lat/lon `f32`, height `u24`, 1 B XOR checksum without `*`). Blocks also interleave settings-journal entries (16 B `AA×7 type u32 BB×4`) and 20 B `HOLUXGR241LOGGER` separators (`crates/m241-core/src/log/journal.rs`). Header masks can be stale; actual record length is determined by checksum chains while the mask supplies field meanings.

## 6. Personal data

Public repo. Never commit real coordinates (use `35.6812360, 139.7671250` or `8960.0000,N,00000.0000,E`), local/home paths, real machine serial-port enumerations, or `holux/`, `vendor/`, `backup/`, device captures.

## 7. Open questions

See `docs/dev/06-open-questions.md` (do not fabricate): log storage location, Japanese firmware SKU, Bluetooth SPP.

## 8. Review and fix: three stages, three different agents

1. **Find**: reviewer lists findings with `file:line`, does not edit.
2. **Fix**: different agent fixes (or argues disagreement rather than silently making a bad change).
3. **Verify**: third agent checks findings + diff + tests without reading fixer's narrative; runs `./tools/check.sh` 6/6; posts `review-gate` status on the reviewed head SHA (author never posts it; new push requires new review). Details in [`docs/dev/09-review-protocol.md`](docs/dev/09-review-protocol.md).

## 9. What went wrong here (evidence for §8)

- **9.1 Merged with Stage 3 unstarted**: green `check.sh` proves tree builds, not that findings were fixed. Never merge while `10-review-log.md` says 未検証.
- **9.2 Lead started writing fixes**: §8 applies to coordinators too; assign fixes, don't self-fix.
- **9.3 Reading a diff treated as verification**: reading a diff is Stage 1; acceptance is Stage 3 by another agent.
- **9.4 Worker failure treated as lost artifact**: always check `git status`/`git log` in a failed worker's worktree first.
- **9.5 `/.cargo-home/` missed symlinks**: use `/.cargo-home` without trailing slash in `.gitignore`.
- **9.6 Ten worktrees filled disk (214 GB)**: keep `debug = 0` in `[profile.dev]` and delete finished worktrees' `target/`.
- **9.7 Self-reported completion with failing checks**: verify counts and run `./tools/check.sh` directly — the artifact is the evidence, not the report.
