# Design

<!-- impeccable:design-schema 1 -->

Mode: **Operate**. The visitor is repairing a device; the interface is a tool, and
the world supplies type, palette, density, and one signature move — never the
layout, navigation model, or controls.

## 1. The brief, read twice

The subject is not "a GPS app". It is **a 2006 gadget that is dying, and the only
documentation left for it is a boxed-software manual nobody printed for this
decade**. The audience arrived because their holiday photos all say 2007.

Their real scene: a desk in the evening, one USB cable, the logger's tiny screen
in their hand, and the knowledge that one wrong click ends the device.

Two consequences drive everything below:

- The app's most important job is **telling the truth about danger**. So the only
  saturated colour in the entire product is the colour of danger.
- The app must **address the person, then the device**: "go press the button on
  the logger" is a first-class interface state, not an error dialog.

## 2. Direction (rolled: manual-acetate-tab-board)

A boxed-software reference manual lying open. Its fore edge carries a stepped
thumb index; a punched acetate leaf hinges a millimetre above the page so figures
sit on a second plane.

Translated into an Operate surface, the world lends exactly four things:

| Contribution | What transfers |
|---|---|
| Type | A low-contrast technical grotesque (Barlow) for UI, tabular mono for every number that could be misread |
| Palette | Graphite ground, bone text, one amber — and amber is spendable on nothing but danger |
| Density | Reference-manual density: ruled tables, right-aligned figures, footnote-weight secondary text |
| Signature move | **The fore-edge thumb index**: a vertical rail whose tabs step outward by position; the active tab is the one that protrudes |

No paper texture, no ruled ledger background, no serif body, no deckled edge. The
manual is a *structural* memory, not a costume.

### What the first pass proposed, and why it was rejected

First pass: warm bone paper (#F4F1EA) with a terracotta accent, a display serif,
and the map as a framed plate. Rejected on review: that is the default
"humanist editorial" treatment any model produces for a heritage object, and it
would have made a repair tool look like a magazine. The palette also put the
saturated colour on decoration rather than on the one thing that deserves it.

Second pass, kept: dark graphite, one amber that means *this can destroy the
device*, and the tab rail as the single authored move.

## 3. Tokens

### Colour

Amber is a **reserved channel**. It may appear only on: the destructive action,
the wizard step that writes to the device, and the state "device state is
undefined". It never appears on hover, focus, selection, or as decoration.

| Token | Value | On `ground` | On `panel` | Role |
|---|---|---|---|---|
| `ground` | `#14181A` | — | — | Window background, content ground |
| `panel` | `#1B2023` | — | — | Raised surface: cards, table headers |
| `rail` | `#111416` | — | — | The tab rail, inset from content |
| `rule` | `#2C3437` | 1.4 | 1.3 | Hairlines between rows and sections |
| `ink` | `#E8E6DF` | 14.3 | 13.2 | Primary text |
| `mute` | `#9AA0A0` | 6.7 | 6.2 | Secondary text, units, captions |
| `danger` | `#E3A008` | 7.9 | 7.3 | **Reserved.** Destructive affordances and states only |
| `data` | `#5FB3C4` | 7.4 | 6.8 | Track line, chart series, selection on the map |
| `ok` | `#6FBF73` | 8.0 | 7.3 | Verified / success |
| `err` | `#E2705E` | 5.7 | 5.3 | Failure text (never a filled background) |

Every text value clears 4.5:1 on both `ground` and `panel`; `err` is the tightest
at 5.3:1 and sets the floor for the palette. `rule` is a hairline, not text, so it
is exempt. Status is never encoded by colour alone — `ok` and `err` always ship
beside a word.

The map is the one light surface: OSM tiles are pale, and letting the map be the
only bright rectangle on a graphite page is the intended reading — the paper
figure on the manual's dark cover.

### Type

| Token | Size / weight | Role |
|---|---|---|
| `display` | 24 / 600 | Screen title, one per screen |
| `title` | 20 / 600 | Section heading |
| `body` | 16 / 400, 1.5 line | Prose, labels |
| `ui` | 13 / 500 | Buttons, tabs, table cells |
| `caption` | 11 / 400, +0.01em | Units, timestamps, footnotes |

Steps are 11 → 13 → 16 → 20 → 24, ratios 1.18 / 1.23 / 1.25 / 1.20. The first
draft used 13/14 for `ui`/`body`, which is a 1.08 step — too close to carry two
different jobs, so 14 was dropped and `body` moved to 16.

Light text on a dark ground needs compensation, so body buys one extra weight
step and +0.01em tracking relative to what the same face would need on paper.

- **Barlow** for Latin UI. Highway-signage grotesque: it belongs to a device built
  for driving, and its slightly squared bowls survive small sizes.
- **Noto Sans JP** for Japanese. Loaded from the system when present; embedded
  fallback only for the subset the UI actually uses.
- **A tabular mono** for hashes, byte offsets (`0x402F4`), coordinates, and every
  number in a column. Not decoration: `0x402F6` and `0x402F7` must align.
- Fixed scale, no fluid type — desktop apps are viewed at a fixed DPI.

### Spacing and shape

4px base. `4 / 8 / 12 / 16 / 24 / 32 / 48`. Content margin 24. Panel padding 16.
Row height 32, table header 28.

Radius: **2px**. Not rounds — a manual's tabs are square-cut. Elevation is
declared once, as a `rule` hairline; no shadows anywhere.

### Motion

150 ms, ease-out, and only for state change. No page-load choreography, no
hover growth. The one authored moment is the tab rail's active tab stepping out
by 4px. `prefers-reduced-motion` (and the app's own setting) drops it to zero.

**Motion thesis.**

- *Focal moment:* the rail tab stepping out 4px and joining the content edge
  when a screen is selected. It is the only motion in the product that says
  "you are now here", and it maps to the manual's thumb index — a tab that
  physically protrudes past its neighbours.
- *Continuity:* the wizard's three steps. The step row is the only place a
  horizontal transition is allowed, because the user is moving through a
  sequence.
- *Feedback:* the progress bar and the `WRITE` field's enabled state. Nothing
  else acknowledges input with motion.
- *Budget:* a 4px translate and a background fill. No blur, no shadow, no
  layout animation. The map redraws on pan/zoom and is the only expensive
  surface; it never animates on its own.

Reduced motion keeps the tab's 4px offset as a static state and removes the
transition; the progress bar updates without easing.

## 4. The signature move

```
 ┌──────────────────────────────────────────────────────────────┐
 │ ▌接続        │                                               │
 │   ▌ログ      │   content                                     │
 │     ▌ファーム│                                               │
 │       ▌設定  │                                               │
 └──────────────────────────────────────────────────────────────┘
```

Each tab is indented 8px more than the one above it, so the rail's outer edge is
a staircase. The active tab is filled with `panel` and extends flush to the
content edge, joining the content panel; the others sit on `rail` and do not.
Stepping happens on selection over 150 ms.

Consequences for the build: this is a custom-painted rail, not `SidePanel` with
buttons. Route the whole navigation through one widget so the step appears
nowhere else.

## 5. Screens

### 接続 — Connect

```
┌─ m241 ──────────────────────────────────────────────── ---------┐
│ ▌接続          │  Holux M-241                                    │
│   ▌ログ        │  ファームの日付が 19.6 年ずれています。          │
│     ▌ファーム  │                                                 │
│       ▌設定    │  ┌ ポート ───────────────────────────────────┐  │
│                │  │ [ /dev/ttyUSB0        v ]  o 接続済み      │  │
│                │  └────────────────────────────────────────────┘  │
│                │  ┌ デバイス ────────────┐ ┌ ログ ─────────────┐  │
│                │  │ firmware  B-core_1.20│ │ 使用量  212,860 B │  │
│                │  │ build     0043       │ │ 点数    10,399    │  │
│                │  │ model     01017-00F  │ │ 期間    11/14 →   │  │
│                │  │ flash     512 KiB    │ │         11/15     │  │
│                │  │ 日付      7168 日ずれ│ │ 記録    有効      │  │
│                │  └──────────────────────┘ └───────────────────┘  │
│                │                                                 │
│                │  次にすること                                   │
│                │  ─────────────────────────────────────────────  │
│                │  ファームを吸い出す        読み取りのみ・安全 →  │
│                │  日付定数を書き換える      PC 内で完結        →  │
│                │  書き戻す                  本体を書き換える    →  │
└──────────────────────────────────────────────────────────────────┘
```

Density rule: the two panels are 50/50, not a dashboard grid. The "next step"
block is a plain ruled list — no cards, no icons, no numbering until the user is
inside the wizard.

### ログ — Log

The map **is** the page. One 40pt tool row (title, the sentence and 見本), then
the canvas at the window's remaining height minus the one-row selected-point
strip. Nothing on this screen scrolls. The panels that used to push the map down
are tabs in a right-hand drawer that starts closed: the canvas is sized from the
window, and opening the drawer takes 360pt from its width where the window is
wide enough, or floats over it where it is not.

```
┌─ m241 ────────────────────────────────────────────────────────────┐
│ ▌接続          │ ログ  地図が主面です。点を選ぶと下の行に…  見本   │
│   ▌ログ        │ ┌───────────────────────────────────┐ ┌─────────┐ │
│     ▌ファーム  │ │ [パス          ] 開く  [パネル]   │ │ 集計 写真│ │
│       ▌設定    │ │                                   │ │ 編集 書き│ │
│                │ │        o----------o               │ │ 出し 操作│ │
│                │ │       ╱            ╲              │ │ ──────── │ │
│                │ │  o---'              o---o---o     │ │ 距離 42.3│ │
│                │ │                                   │ │ 時間 2h18│ │
│                │ │ © OpenStreetMap   [− ＋ zoom 15]  │ │ 平均 18.4│ │
│                │ │    ┌ 再生 ───────────────────┐    │ │          │ │
│                │ │    │ ▶ 前 次 全点 追従  12/240 │    │ │          │ │
│                │ │    └────────────────────────┘    │ │          │ │
│                │ └───────────────────────────────────┘ └─────────┘ │
│                │ 選択した点  2026-11-15 08:14:22 UTC  35.681236 … │
└───────────────────────────────────────────────────────────────────┘
```

Five plates live **over** the canvas, each anchored to a corner so none of them
moves when another changes:

| Plate | Corner | Contents |
|---|---|---|
| Log file | top-left | the path, 開く, and the result of the last load |
| パネル | top-right | the drawer toggle, the only route into the panels |
| 再生 | bottom-centre | transport and timeline (the selected point's figures are the strip under the canvas) |
| zoom | bottom-right | `−` / `＋`, the level actually drawn, the map's state note |
| credit | bottom-left | `© OpenStreetMap contributors`, only when OSM pixels were drawn |

Each plate is `rail` at 0.92 alpha with `ink` text: over OSM's light face that
composites above 11:1, where bare `mute` measured 2.3:1. The plate rects are
computed in one place (`screens::log::drawer`), and a render test asserts that no
two of them — nor the credit and the zoom plate, with the drawer open at
1100–1280pt — intersect.

The drawer is a 360pt right column holding five tabs: 集計 (figures, day filter,
speed chart, point list), 写真, 編集, 書き出し (format chips, destination, geotag,
KMZ options) and ログ操作 (download, backup, erase gates, projects, NMEA, read).
Below 1000pt of content width there is no room for a 360pt column beside a
readable map, so the drawer floats over the canvas instead; the map keeps its
size and the toggle stays the last thing painted, so it always remains reachable.
The floating drawer is lifted above the playback and zoom plates, so opening it
does not bury the map's own controls.

A 32pt selected-point strip sits directly under the canvas, not in a panel below
the fold: the canvas is a picker, and the answer to a click belongs under the
thing clicked. 編集, 写真 and ログ操作 no longer draw a second staircase beside
the rail.

The map is the product's one light face: OpenStreetMap tiles under the track,
with the licence's `© OpenStreetMap contributors` credit on the canvas. When the
tiles cannot be fetched — offline, or the setting switched off — the same canvas
falls back to the schematic grid and says so on the zoom plate in `ink` (not bare
`mute` on a tile); the track, its records and every control keep working, because
none of them needs the network.

### ファーム — Firmware

```
┌─ m241 ────────────────────────────────────────────────────────────┐
│ ▌接続          │  ファームを書き換える                            │
│   ▌ログ        │  本体のファームを吸い出し、日付定数を書き換えて   │
│     ▌ファーム  │  書き戻します。                                   │
│       ▌設定    │                                                   │
│                │  1 吸い出し   2 書き換え   3 書き戻し              │
│                │  ============O--------------O                      │
│                │                                                   │
│                │  保存先  [ backup/stock.bin            ] [参照]   │
│                │                                                   │
│                │  [#############         ]  214,880 /     │
│                │                                    406,620  53%   │
│                │                                                   │
│                │  1 回目の読み取りが終わりました。                 │
│                │  電池を抜いて 10 秒待ち、入れ直してから続行して   │
│                │  ください。                                       │
│                │                                                   │
│                │                              [ 中止 ]  [ 続行 ]  │
└───────────────────────────────────────────────────────────────────┘
```

Step 3 changes only in its lower half, and that is the whole point of the screen:

```
│  ┌ 書き込み ───────────────────────────────────────────────────┐ │
│  │ 本体のファームを上書きします。失敗すると使えなくなる場合が    │ │
│  │ あります。                                                    │ │
│  │                                                               │ │
│  │ firmware   fixed.bin       406,620 B   9f192786…             │ │
│  │ DA         v3.8.4.bin      0x0308      512 KiB   id 0x49     │ │
│  │ 差分       3 バイト（0x402F6, 0x402F7, 0x59C2C）              │ │
│  │                                                               │ │
│  │ 確認のため WRITE と入力  [                    ]               │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                          [ 中止 ]  [ 書き込む ]    │
```

`[ 書き込む ]` is the **only** amber-filled control in the product. It stays
disabled until the field contains `WRITE`.

### 設定 — Settings

Plain rows, no rail block: map cache size and location, the map-tiles switch,
units, theme (light theme is not built; the row states that), language, the
coordinate format (10進度 / 度分秒 / UTM; display only — the stored data is
unchanged, and UTM falls back to decimal degrees below its latitude bands),
reduced motion, and the clear-cache action.

## 6. States the build must ship

| State | Where | Treatment |
|---|---|---|
| No port found | Connect | Rail tab stays; content says what to check, in the order to check it |
| Sync failed | Connect, Firmware | The device's own words first, the hint second |
| Device state undefined | Firmware step 3 failure | `danger` border on the panel, the recovery command as copyable text |
| Waiting for the user | Firmware step 2 | Progress held at 100%, the instruction given as an imperative |
| Empty log | Log | A plate at the canvas centre: 「まだログがありません」, what to open or download, ログ操作を開く (opens that drawer tab) and the 見本 checkbox. The canvas stays the page |
| Offline map | Log | Schematic grid plus 「地図タイルを取得できません（オフライン）」 on the zoom plate in `ink`, never bare `mute` on a tile; the track still draws |
| Drawer closed | Log | The default: five plates over the canvas and no panel taking height from it |
| Drawer open | Log | 360pt column beside the canvas at or above 1000pt content width; below it the drawer floats over the canvas, above the playback and zoom plates, and the toggle stays on top |
| Unverified feature | Anywhere | The word 未検証 beside it, `mute`, never `danger` |
| Loading | Any | Skeleton rows in the table, never a centred spinner |
| Keyboard | All | Every control focusable, focus ring is a 2px `ink` outline inset 1px |

## 7. What this design refuses

- Amber as decoration, hover, or accent. It has one meaning.
- Cards inside cards; the rail blocks are ruled, not boxed.
- Icons standing in for words in the destructive path. Words are unambiguous.
- Emoji in the interface, including in status.
- A modal for the power-cycle instruction — it is inline, on the step.
- A light-to-dark inversion of the palette as a "theme"; the design is dark
  because the scene is a desk at night beside a lit device.

## 8. Review protocol (post-build)

`m241-gui --screenshot` is now built and renders the four screens offscreen at
1280×800 and 1024×640; the initial design review and its Stage 3 verified fixes
are recorded in `docs/dev/10-review-log.md`. Repeat this protocol whenever the
UI changes:

1. `m241-gui --screenshot` renders each screen to PNG at 1280×800 and 1024×640
   using `egui`'s offscreen path, saved under
   `.design/firmware-wizard/screenshots/`.
2. Review against this document, not against taste: contrast measured, rail
   staircase verified, amber inventory verified to be exactly one control.
3. Findings recorded in `docs/dev/10-review-log.md` with Must / Should / Could.

The standard set is the four screens × two sizes, plus the Log drawer: five
`log-drawer-<tab>-*` images at each size, because the map is the page and four of
the five panels would otherwise have no rendered frame at all. The two closed
Log images stay the map the review is about. A unit test
(`every_drawer_tab_opens_and_draws_its_panel`) still checks each tab draws its
signature control without paying for a full-size render, and the opt-in
`M241_GUI_OPS_SCREENSHOTS=<dir>` run (`cargo test -p m241-gui` writes
`ops-<mode>.png` for each ログ操作 panel, from the empty log at 1024×640) renders
the console's own six panels.

Three checks are mechanical and belong in CI, not in a human pass:

```text
tools/check_design.py
  1. Amber is used only inside the destructive module.
  2. No egui Shape carries a corner radius other than RADIUS.
  3. Every status colour is paired with a text label.
```

`crates/m241-gui/tests/design_gate.rs` drives that script against planted
violations, so a broken regex fails the test suite rather than silently passing
everything.

The amber inventory is the important one: grep the GUI crate for `tokens::DANGER`
and assert every use site is in `screens::firmware` and none of them is a hover,
focus, or selection colour.

### Blocked on

A comp round needs image generation. Two ways to unblock, in preference order:

1. Log in to an image provider in the DSH web app, then render the three
   compositional comps per `.agents/skills/impeccable/reference/visualize.md`.
2. Build the shell in egui first and screenshot the real thing; a rendered
   `egui` window at 1280×800 is a better comp than an image model's guess, and
   it is the artifact the review will judge anyway.

## 9. Open

- Whether the app ships light-mode at all. A daylight/outdoor reading mode may be
  warranted; not designed yet.

## 10. As built — where the implementation approximates this document

This section records every place the build departs from the world above. It adds
no token and changes none; it exists so a reviewer can tell a deliberate
approximation from a mistake.

### Type

- **No font file is committed or fetched.** `theme::install` promotes the OS's
  own sans face to primary for `Proportional` and the OS's monospace face for
  `Monospace`, and keeps `egui_system_font_provider` (locale `ja`) underneath
  for glyphs those faces lack. On the reference Linux machine that resolves to
  **LINE Seed JP** (Latin and Japanese) and **UDEV Gothic NF** (numbers).
  Barlow and Noto Sans JP are therefore approximated, not shipped.
- **The approximation is a build choice, not a licence limit.** Both Barlow and
  Noto Sans JP are SIL Open Font License 1.1 and could be committed. This build
  commits no font binary (a repository-size and provenance choice), so the OS
  face stands in until bundling the brief's faces is a deliberate change. The
  build never fetches a font at run time.
- **Weights.** The OS faces are installed at one weight and `egui` does not
  synthesise a bold, so Barlow's 600 / 500 / 400 are approximated with size and
  the `ink` / `mute` pair: 24 and 20 are `ink`; 16, 13 and 11 choose by role.
  The five sizes are exact; the weight step between them is not.
- **Tabular figures.** `0x402F6` and `0x402F7` line up because every figure is
  monospace on the header's own column fractions (`common::table_header`,
  `common::table_row`), not because the proportional face happens to have
  tabular digits.

### Density and composition

- `common::figure_row` (height 32) and `common::figure_row_compact` (height 28)
  draw the caption at the left edge, the figure at the right edge and one `rule`
  hairline underneath; `common::table_header` is 28.
- The Log page's map takes the remaining height: the content's height minus the
  40pt tool row (`metric::TOOL_ROW`) and, when a track is loaded, the 32pt
  selected-point strip (one `metric::ROW`). The old fixed 160pt map and the
  `metric::PLOT` figure beside it are gone from this screen. Measured through
  `App::draw` at the two documented sizes, the canvas covers **88.2%** of the
  content at 1280×800 and **84.6%** at 1024×640;
  `log::tests::the_map_is_the_page_at_both_documented_sizes` fails below the
  80% / 75% floors the review set.
- The drawer is `metric::DRAWER` (360pt) wide and starts closed. It is a column
  beside the canvas at a content width of `metric::NARROW` (1000pt) or more and
  floats over the canvas below that, because 360pt beside a sub-1000pt canvas
  would leave the map a strip rather than the page. The threshold is the Log
  body's width (`ui.max_rect()`), not the window's. The same test drives both
  branches at 1280×800 and 1024×640.
- Nothing on the Log page itself scrolls. The one scrolling region is inside the
  drawer: its selected tab's panel scrolls, so a tall 集計 or 編集 panel never
  moves the map. The selected-point strip is one row directly under the canvas
  and does not grow.
- The overlay plates are computed in `screens::log::drawer::Plates` and asserted
  non-overlapping — including the OSM credit against the zoom plate with the
  drawer open at 1100–1280pt — by
  `the_map_plates_do_not_overlap_each_other_or_the_credit` and
  `the_credit_and_the_zoom_plate_stay_apart_with_the_drawer_open`. The plates
  keep fixed heights except the file plate's status line, which is measured
  before the plate is laid out and wraps inside it (a long refusal is readable
  without hovering). The credit is laid out in the canvas left of the zoom plate
  and wraps to that width, so the two cannot meet.
- The 書き出し panel is laid out for the drawer's 332pt inner width: the six
  writer chips take two explicit rows, and the destination field and 書き出す
  stack instead of sharing a line. `horizontal_wrapped` cannot break between two
  `common::tool_button`s, because each is a `ui.scope` and a scope is placed
  without a wrap check; left to the wrapping layout, the last chip and
  写真に位置 were pushed past the drawer's clip edge, where egui stops painting
  them (present in the code, absent from the screen).
- Body line-height 1.5 and the caption's +0.01em tracking are not implemented:
  egui 0.37 sizes text by font and has no per-style line-height or letter-spacing
  hook. The five sizes are exact; the leading and tracking are the OS face's.
- There are no table skeleton rows for a Loading state. The only loading state
  that exists is the read-only device probe, which has no table to skeleton, so
  it prints the probing status line instead of inventing rows.
- Footers are pinned, because the action is the reason the screen exists:
  Connect carries 再読込 (secondary) / 接続して確認 (primary); Firmware carries
  中止 / 続行・書き込む plus the step they belong to; Settings carries the version
  and the session-scope sentence. The Log screen has no footer any more: its one
  export action moved into the 書き出し drawer tab, and the map keeps the height
  the strip used to hold.

### Buttons

- primary = `ink` face with a `ground` word; secondary = `rule` outline, no
  fill; destructive = the amber 書き込む, still the only amber-filled control.
  A disabled primary becomes an outline with a `mute` word instead of a dimmed
  bone face, so its label keeps the contrast pair it was designed for.
- Focus is the theme's 2pt `ink` outline. Selection uses the `data` tint
  (export-format chips) or an `ink` outline (segmented settings choices), never
  amber.
- A fourth, compact class (`common::tool_button`) covers the Log map's playback
  controls and the export-format chips: a `rule` outline at 28pt, selected as an
  `ink` outline, never a filled face. Leaving those on egui's default fill made
  the toolbar the brightest surface on the page and gave the Log screen a second
  primary action beside 中止. Step 1/2's 参照 / 吸い出す / 開く / 保存 also moved
  to the secondary/primary classes, so only the destructive amber control is a
  filled button.
- An input field is the raised surface: `visuals.text_edit_bg_color` is `panel`,
  one step above `ground`, so a path field reads as an editable box rather than
  as another heading (the theme sets it once, in `theme::apply`). egui's default
  left a TextEdit as a hole slightly darker than the page, which is the
  input/heading distinction the design direction asked for.
- The firmware write button is armed by its gates, and pressing it runs the same
  session the CLI uses (`m241_flash::flow`) after the user types `WRITE`: the
  range is the fixed constant, the unit is compared with the backup, the image
  is written and the range is read back before success is shown. The GUI links
  `m241-flash` (user decision, 2026-11-23); the safety gate requires the write
  to go through `flow::flash` and the confirmation to come from the typed word.

### Copy and states

- Device-panel row labels are Japanese: ファーム / ビルド / モデル / フラッシュ /
  日付. The values are unchanged; the English labels in §5's figure were the
  implementation's mixed-language leftover.
- The log-file input's placeholder is `ログファイルのパス`, not a
  `path/to/log.bin` sample.
- Empty log: 「まだログがありません」 on a plate at the canvas centre, the next
  step in one sentence, `ログ操作を開く` (which opens that drawer tab, so download
  is one press away) and the 見本 checkbox — the path input and 開く are on the
  map's own file plate, where they stay available.
- The map draws OpenStreetMap tiles under the track and credits them on the
  canvas whenever at least one OSM tile reached it. A sample, offline or
  still-loading frame draws no OSM pixels and carries no credit, because the
  credit is a statement about where the pixels came from. The attribution sits
  bottom-left, wrapped into the width the zoom plate leaves; the zoom note owns
  the bottom-right. The credit is drawn after the plates so it is never under a
  track. The schematic grid is still drawn first and is what the offline state
  shows, so a failed fetch leaves a readable canvas rather than a blank.
- The track and the tiles are projected through one `MapView` (Web Mercator).
  The previous track projection was equirectangular with a `cos(latitude)`
  correction; close, but a road drawn a few pixels off the tile is exactly the
  defect the map was added to remove, so the track moved to the tiles'
  projection rather than the tiles being bent to the track.
- The map's zoom is 1–19 (OSM's range), with fractional zoom: fit can land
  between levels and the tiles are then scaled by the fraction. `+`/`−` move in
  whole steps from the zoom actually drawn — fit publishes its zoom to the
  buttons, so a press straight after 全点表示 never lowers the map. Follow
  mode's 12-zoom floor and the fit-all/zoom/follow interactions are unchanged.
- Tiles live in the OS cache directory (`m241-gui/tiles`, 30-day TTL, 200 MB
  cap, oldest first) and the Settings rows report the measured size and the real
  path. At most four fetches are in flight and only the visible canvas plus one
  ring is asked for; nothing is prefetched. A tile that failed is retried after
  the same 60-second window as the offline latch, so one timeout does not leave
  a grey square for the whole session.
- `--screenshot` and the headless render tests never fetch: the sample frame says
  「見本：地図タイルは表示しません」, and the window gate is
  `native_pixels_per_point`, not `inner_rect`. Wayland cannot report a window
  position, so an `inner_rect` gate disabled the map on every Wayland session
  while the caption said "loading"; a headless context has no native
  pixels-per-point and is reported as 「ヘッドレス」. Fetching from a running
  Wayland window was verified (Hyprland / winit's Wayland backend, 8 tiles
  written to the cache), and X11/Xwayland was (16 tiles); the visual appearance
  on Wayland was not, because this environment's screen capture never answered
  (see README's unverified list).
- The GUI has exactly one network consumer: background tiles. It used to be a
  project rule that the GUI must not link a network client at all, because a
  fetch cannot be exercised headlessly. The user asked for a real map, so that
  rule was lifted for this module. What replaces it is the `TileFetcher` seam
  (tests inject a fake), the window gate and the `sample` flag; what does not is
  a mechanical check — `tools/check_safety.py` still knows nothing about the
  network. A second network consumer would need the same seam and an update to
  this bullet and the note in `crates/m241-gui/Cargo.toml`.
- The screenshots show the firmware write step with the sample image applied in
  memory and the confirmation armed — it is never written to disk, and the
  panel and screen still carry 「見本」. That is why the amber button appears in
  its armed state in the review images.
- Settings ends with a colophon of facts read from `m241-core` (supported
  firmware, patch offset). The offset is formatted from
  `patch::EFFECTIVE_EPOCH_OFFSET`, not written as `0x402F4`, so it cannot drift
  from the measured constant; the byte count is the V1.13 patch's figure. It is
  reference material, not a sixth setting.
- Step 3's figures are Japanese and partly unfilled: `ファーム` rather than §5's
  `firmware`, and `DA 未取得` with `未検証 — 端末から読んでいません` because this
  crate never reads the DA. The `差分` row is real: the bytes and offsets come
  from the plan.
- `KMZ` is an export chip beside GPX/KML/CSV that writes a KMZ archive rather
  than a sixth `ExportFormat` (the archive bundles photos, so it is not a
  rendering of the track alone). It is marked `未検証`: the core writer is
  covered by tests, but Google Earth and `unzip` have not been run against this
  build's output, and a chip that looked judged would claim more than that.
- The selected point shows time, coordinate, altitude and speed. It does not show
  a satellite count because `TrackPoint` carries none — the log record has no
  such field. The record count is printed without a thousands separator
  (`10399`), not as §5's `10,399`.
- The Connect screen's next-step rows carry no `→` (the figure in §5 draws one;
  see below).
- Settings' cache location is the path the fetcher actually writes to, resolved
  through the OS cache directory, and the size row is measured from it. The
  clear action deletes only that directory; it is not amber, because deleting a
  local cache is not a destructive device action.
- The port ComboBox and hover tooltips carry no drop shadow, and the rail's tab
  transition is 150 ms ease-out (quadratic); reduced motion drops it to 0. These
  are the design's own values, recorded because §3's motion and elevation rules
  are easy to lose to a toolkit default.
- The map's polyline is one `data` line, but the plotted records carry the
  ten-colour cycle in `theme::TRACK_PALETTE` (DESIGN.md §3 does not list it).
  This is inherited from the pre-GUI build and the ezTour reference spec
  (07-eztour-spec §5); on a long track it reads as colour rather than data.
  Replacing it with a single `data` colour (or adding the palette to §3) is
  follow-up work, recorded here because §10 promises to name every departure.
- The Connect screen's next-step rows have no `→`. DESIGN.md §5's mock marks the
  list's direction, but a row that looks like a link and does nothing on click or
  focus is a false affordance (design review, Should 3); the note says where the
  row leads in words. §5's arrow remains in the mock.
- UI copy separates the word from its explanation with parentheses, not an em
  dash: 「見本（実機から読んだ値ではありません）」「未検証（端末から読んで
  いません）」. The dash read as a generated label; the parentheses keep the
  explanation without the tell.
- The reduced-motion row is 「動きの低減」, not 「視差低減」: the setting turns
  movement down, and parallax is its own effect (design review, Should 6).
- Custom controls paint their focus ring from the widget's `Response`
  (`common::focus_ring`). egui's `Button` draws its own outline only when it has
  no explicit `.stroke()`, and every button here sets one to state its role, so
  without the helper a focused 接続して確認 / 中止 / 書き込む was identical to an
  unfocused one (design review, Must 1).
- The write step's three prerequisites and the result of a press live in the
  pinned footer, beside 書き込む, not at the bottom of the scrolling panel: at
  1024×640 the panel's end is below the fold, so the gates were invisible while
  the amber button was not (design review, Should 2).
- A machine with no usable system font keeps egui's bundled Latin-only faces;
  Japanese labels render as boxes there. This is the cost of not committing a
  font file; the bundled faces are the fallback, not the design.
- The ログ操作 console is six chip-selected panels under one 「ログ操作」 heading
  rather than six rail entries: the rail is navigation for the product's four
  screens, and these are modes of one task. Each panel's caption names the CLI
  command it replaces, and its status line only appears on the panel that
  produced it. The erase panel is gates only — a mandatory backup, a
  confirmation, then a refusal that says the log console calls no erase client
  (the GUI links `m241-flash` for the Firmware screen, not for this panel) and
  that erasing the user's track needs the user's own instruction — and carries
  no amber: the press cannot erase anything, so an amber control would promise a
  destructive action this build does not have.
- The download and erase panels each draw their own port field and 上書き
  checkbox. They were one shared value first, and the erase panel told the user
  to tick a control it did not draw; the labels are constants because the
  refusal quotes them.
- The KMZ `icon/` thumbnails are an opt-in checkbox, default off. The core
  records the `icon/` semantics as unconfirmed, and the CLI's `log kmz` writes
  no `icon/` entry, so the default GUI archive is the same shape as the CLI's;
  turning the option on keeps the row marked 未検証.
- A relative `log` / `photos` / `output` in a project is stored relative to the
  project file's directory with `/` separators, as `02-cli.md` specifies, so a
  saved project directory can be moved with the files it names. An absolute path
  stays absolute.
- The 「読み書き」 panel's JSON output, a project load and the ログを開く GPX path
  carry `nmea` / `input: "gpx"` / `offset_days` / `position` exactly as
  `log read --format json` does: the machine-readable output must not depend on
  which front end wrote it. `position` is decimal degrees (the CLI default; the
  Settings coordinate format is display-only) and `simplify` never appears
  because the GUI cannot thin points.
