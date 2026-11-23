# Design

<!-- impeccable:design-schema 1 -->

Mode: **Operate**. Repair tool for a 2006 GPS logger (Holux M-241); world supplies type, palette, density, and one signature move.

## 1. The brief

Audience: someone at a desk with one USB cable, a logger whose dates are 19.6 years behind, and a write step that can brick it.
- Tell the truth about danger: the only saturated colour is danger (amber).
- Address the person, then the device: physical instructions ("power-cycle the logger") are inline states, not error modals.

## 2. Direction (`manual-acetate-tab-board`)

Boxed-software reference manual aesthetic (structural, no paper texture or serif body):
- **Type**: low-contrast grotesque (Barlow) + tabular mono for hashes, offsets, coordinates, and table figures.
- **Palette**: dark graphite ground, bone text, one reserved amber for danger.
- **Density**: ruled tables, right-aligned figures, footnote-weight secondary copy.
- **Signature move**: fore-edge stepped thumb-index rail; active tab protrudes and joins the content edge.

First pass (warm bone paper + terracotta + display serif) was rejected as editorial/decorative.

## 3. Tokens

### Colour

**Amber (`danger`) is reserved** for: the destructive button, the firmware write step, and "device state is undefined". Never hover, focus, selection, or decoration.

| Token | Value | On `ground` | On `panel` | Role |
|---|---|---|---|---|
| `ground` | `#14181A` | — | — | Window & content background |
| `panel` | `#1B2023` | — | — | Raised surface (cards, headers, inputs) |
| `rail` | `#111416` | — | — | Inset tab rail |
| `rule` | `#2C3437` | 1.4 | 1.3 | Hairlines |
| `ink` | `#E8E6DF` | 14.3 | 13.2 | Primary text |
| `mute` | `#9AA0A0` | 6.7 | 6.2 | Secondary text, units, captions |
| `danger` | `#E3A008` | 7.9 | 7.3 | **Reserved.** Destructive affordances/states only |
| `data` | `#5FB3C4` | 7.4 | 6.8 | Track line, chart series, map selection |
| `ok` | `#6FBF73` | 8.0 | 7.3 | Verified / success (always paired with text) |
| `err` | `#E2705E` | 5.7 | 5.3 | Failure text (never filled background; paired with text) |

All text tokens exceed 4.5:1 (`err` floor 5.3:1). Map is the one light surface: OSM raster tiles (Tauri uses MapLibre `raster-saturation: -0.7`, `raster-brightness-max: 0.85` so the 5 px track stays the only saturated element).

### Type

Fixed desktop scale (Barlow Latin, Noto Sans JP, tabular mono):

| Token | Size / weight | Role |
|---|---|---|
| `display` | 24 / 600 | Screen title (one per screen) |
| `title` | 20 / 600 | Section heading |
| `body` | 16 / 400, 1.5 line | Prose, labels |
| `ui` | 13 / 500 | Buttons, tabs, table cells |
| `caption` | 11 / 400, +0.01em | Units, timestamps, footnotes |

### Spacing, shape & motion

- **Spacing**: 4px base (`4 / 8 / 12 / 16 / 24 / 32 / 48`). Content margin 24, panel padding 16, row 32, table header 28.
- **Shape**: **2px** radius everywhere (`theme::RADIUS`); `rule` hairlines, no shadows.
- **Motion**: **150 ms ease-out** (quadratic) for state change only. One authored motion: active rail tab stepping out 4px. Wizard steps allow horizontal continuity; progress bar and `WRITE` arming give feedback. No hover growth, blur, or auto-animating map. Reduced motion drops transition duration to 0 (static 4px offset).

## 4. The signature move

Custom-painted vertical rail: each tab indents 8px more than the one above (`接続` → `ログ` → `ファーム` → `設定`). Active tab fills with `panel` and extends flush to the content edge over 150 ms; inactive tabs sit on `rail`.

## 5. Screens

- **接続 (Connect)**: Port selector + connection status; 50/50 Device & Log ruled panels; plain ruled "次にすること" list (no cards/icons).
- **ログ (Log)**: The map **is** the page (no page scroll). 40pt tool row + full remaining canvas + 32pt selected-point strip beneath. Five corner plates (`rail` at 0.92 alpha, `ink` text, non-overlapping):
  - Top-left: log file path, 開く, load status (wraps on long error).
  - Top-right: `パネル` drawer toggle.
  - Bottom-centre: playback transport & timeline.
  - Bottom-right: zoom `−`/`＋`, drawn level, map state note.
  - Bottom-left: `© OpenStreetMap contributors` (only when OSM tiles were drawn; wraps left of zoom plate with ≥12pt gap).
  - **Drawer** (starts closed, 360pt wide, 5 tabs: `集計`, `写真`, `編集`, `書き出し`, `ログ操作`): column beside canvas at ≥1000pt content width; floats above playback/zoom plates below 1000pt. Offline/off falls back to schematic grid.
- **ファーム (Firmware)**: 3-step wizard (`1 吸い出し`, `2 書き換え`, `3 書き戻し`). Step 1/2 hold inline power-cycle instructions. Step 3 shows image/DA/diff table, `WRITE` input, and the sole amber-filled `[ 書き込む ]` button (disabled until `WRITE` is typed).
- **設定 (Settings)**: Plain ruled rows: map cache size & location, map-tiles switch, units, theme (dark only), language, coordinate format (10進度 / 度分秒 / UTM, display-only), reduced motion (`動きの低減`), clear-cache action, and a `m241-core` colophon.

## 6. States the build must ship

| State | Where | Treatment |
|---|---|---|
| No port found | Connect | Ordered checklist of what to verify |
| Sync failed | Connect, Firmware | Device error first, hint second |
| Device state undefined | Firmware step 3 failure | `danger` border + copyable recovery command |
| Waiting for user | Firmware step 2 | Progress held at 100%, imperative instruction |
| Empty log | Log | Centred plate (`まだログがありません`), `ログ操作を開く`, `見本` toggle |
| Offline map | Log | Schematic grid + `地図タイルを取得できません（オフライン）` on zoom plate in `ink` |
| Drawer closed / open | Log | Default closed; ≥1000pt side column, <1000pt floating above plates |
| Unverified feature | Anywhere | `未検証` in `mute`, never `danger` |
| Loading | Any | Skeleton rows or status line, never a centred spinner |
| Keyboard | All | Every control focusable; 2px `ink` inset focus ring |

## 7. What this design refuses

Amber as decoration/hover/focus; cards inside cards; icon-only destructive controls; emoji in UI; modal power-cycle dialogs; inverted light theme.

## 8. Review protocol (post-build)

1. Run `m241-gui --screenshot` → PNGs in `.design/firmware-wizard/screenshots/` (4 screens × 1280×800 & 1024×640 + 5 Log drawer tabs × 2 sizes; opt-in `M241_GUI_OPS_SCREENSHOTS=<dir>` renders the 6 ログ操作 panels).
2. Audit contrast, rail staircase, and single-amber inventory against this doc; log Must/Should/Could in `docs/dev/10-review-log.md`.
3. CI mechanical gate `tools/check_design.py` (tested by `crates/m241-gui/tests/design_gate.rs`): (1) `tokens::DANGER` only in `screens::firmware`, (2) corner radius always `theme::RADIUS`, (3) status colour paired with text.

## 9. Open

- Daylight/outdoor light mode: not designed yet.

## 10. As built — departures & implementation notes

- **Fonts**: No font binaries committed or fetched at runtime (Barlow / Noto Sans JP are SIL OFL 1.1; non-bundling is a repo-size choice). OS sans/mono promoted (`LINE Seed JP` / `UDEV Gothic NF` on reference Linux); weights approximated via size + `ink`/`mute`. egui 0.37 lacks per-style line-height/tracking hooks.
- **Layout**: Log canvas covers 88.2% (1280×800) / 84.6% (1024×640). Drawer threshold is 1000pt content width (`ui.max_rect()`). 書き出し chips wrap into 2 explicit rows for the 332pt inner width. Firmware prerequisites and result sit in the pinned footer so they stay visible at 1024×640.
- **Buttons & focus**: `primary` (`ink` fill), `secondary` (`rule` outline), `tool_button` (compact 28pt `rule` outline, `ink` outline when selected), `destructive` (amber). Custom controls paint `common::focus_ring` (2pt `ink`). `TextEdit` uses `panel` fill.
- **Copy & markers**: Japanese device labels; parenthetical explanations (`見本（…）`, `未検証（…）`) instead of em dashes; no `→` on non-interactive next-step rows; `動きの低減` (not `視差低減`); no thousands separator on record counts; no satellite count (absent from `TrackPoint`); `KMZ` chip and `icon/` checkbox (default off) marked `未検証`; track dots still cycle 10-colour `theme::TRACK_PALETTE` (follow-up vs single `data` colour).
- **Network (two consumers, tiles only)**:
  1. egui GUI (`crates/m241-gui/src/screens/log/tiles.rs`) via `TileFetcher` seam (Web Mercator, zoom 1–19, OS cache `m241-gui/tiles`, 30-day TTL, 200 MB cap, 4 in-flight, 60 s retry). Window gate uses `native_pixels_per_point` (works on Wayland/X11; headless/`--screenshot`/sample never fetch; not enforced by `check_safety.py`).
  2. Tauri app (`crates/m241-app`) via WebView MapLibre (`tile.openstreetmap.org` in CSP; toggled by persisted switch).
- **Firmware & ログ操作**: GUI links `m241-flash` for Firmware writes (`m241_flash::flow`) gated by typed `WRITE`; Log console's erase panel stops at a refusal (no amber, CLI-only erase). Project relative paths and JSON exports (`nmea`, `input: "gpx"`, `offset_days`, `position` in decimal degrees) match `02-cli.md` byte-for-byte.
