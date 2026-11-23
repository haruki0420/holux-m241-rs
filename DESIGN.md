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

## 2. Direction (rolled: one-seam-instrument)

An instrument panel beside a map: the map is the page, the drawer is the margin,
and **the only seam through the screen is the drawer's edge**. Hierarchy is made
with a stack of five surfaces, 20pt of inner space and one corner radius. A rule
is used for a *data* boundary — a table row, a figure row — and never as the way a
screen is divided.

This replaces the previous direction (`manual-acetate-tab-board`). The diagnosis
is in §10: the world view was "a printed form", so the build put everything on one
flat ground and drew a hairline around each block. On a 2020s tool, hierarchy is a
fill and a change of surface; the manual's grammar made every screen a ruled
sheet.

Why this object fits this product: the job is **reading a recording on a map**.
The reference implementation (ezTour, `docs/dev/07-eztour-spec.ja.md` §5) puts the
map at 62% × 84%, a docked information column on the right, a 22px status bar at
the bottom and controls in the map's corners. That placement is right for the job.
What looks fourteen years old is its **execution** — a 96px three-row chrome, a
Windows 95 bevel, a blue gradient, 116 glyph icons, the Google Maps frame. This
design keeps the placement and replaces the execution, which is the whole
difference between "reference" and "imitation".

What the world lends, in four columns:

| Contribution | What transfers |
|---|---|
| Type | A small sans scale (12 / 14 / 16 / 20 / 28) where the smallest step is 12: no 11pt body copy. Every figure that could be misread is monospace. |
| Palette | Five surfaces from `#090C0E` to `#262E34`, three text tones, two rules, two data colours — and one amber, spendable on nothing but danger. |
| Density | 40pt list rows, 36pt drawer rows, 32pt table rows, 28pt headers; 20pt inside a card, 24pt between panels, 32pt between sections. |
| Signature move | **The seam**: one full-height 1px `LINE` where the drawer meets the page. Nothing else divides the content. |

What it refuses outright: bevels, gradients, glows, glass, a second accent colour,
an icon that stands in for a word in the destructive path, and a card inside a
card. The three mechanical invariants survive from the previous direction: amber
is reserved, `theme::RADIUS` is the only corner, and a status colour always ships
beside a word.

### What the previous pass proposed, and why it was rejected

First pass: warm bone paper (#F4F1EA) with a terracotta accent, a display serif,
and the map as a framed plate. Rejected on review: that is the default
"humanist editorial" treatment any model produces for a heritage object, and it
would have made a repair tool look like a magazine.

Second pass: dark graphite, one amber that means *this can destroy the device*,
and a stepped tab rail as the single authored move. Kept for a year; rejected in
the 2026-11-23 review because the rail was the only *surface* in a build made
entirely of hairlines, and because the map had become a strip under the panels.
The palette, the amber rule, the monospace figures and the unverified language are
carried forward; the ruled-form grammar and the staircase are not.

## 3. Tokens

### Colour

Amber is a **reserved channel**. It may appear only on: the destructive action,
the wizard step that writes to the device, and the state "device state is
undefined" — three places in the firmware screen, counted by
`tools/check_design.py` and by `tests/design_invariants.rs`. It never appears on
hover, focus, selection, or as decoration.

| Token | Value | On window | On surface | On raised | On overlay | Role |
|---|---|---|---|---|---|---|
| `WINDOW` | `#0D1012` | — | — | — | — | Window ground and the navigation rail |
| `SURFACE` | `#161B1E` | 1.10 | — | — | — | Cards and panels: the page's sheet |
| `RAISED` | `#1E2429` | 1.22 | 1.11 | — | — | Header, drawer, tab strip, a control's hover face |
| `OVERLAY` | `#262E34` | 1.38 | 1.26 | 1.14 | — | Floating layers only: popups, the map's plates, a selected tab |
| `SUNKEN` | `#090C0E` | 1.03 | 1.13 | 1.25 | 1.42 | A well: input fields, the map canvas, a progress track |
| `LINE` | `#2E373C` | 1.57 | 1.43 | 1.29 | 1.13 | A card's edge and a data row's boundary; the one rule |
| `LINE_STRONG` | `#727E85` | 4.58 | 4.17 | 3.76 | 3.31 | A control's or an input's boundary (WCAG 1.4.11, 3:1) |
| `INK` | `#EDEBE5` | 16.0 | 14.6 | 13.2 | 11.6 | Primary text |
| `MUTE` | `#A9AFAF` | 8.6 | 7.8 | 7.1 | 6.2 | Secondary text, units, captions |
| `FAINT` | `#939D9E` | 6.9 | 6.3 | 5.6 | 5.0 | Third tone: footnotes, unavailable labels |
| `DATA` | `#5FB3C4` | 7.9 | 7.2 | 6.5 | 5.7 | Track, chart series, progress fill, selection — on dark chrome |
| `DATA_MAP` | `#1A6E82` | — | — | — | — | The track over an OSM tile: 5.09 on OSM land, 3.64 on water |
| `OK` | `#6FBF73` | 8.5 | 7.8 | 7.0 | 6.2 | Verified / success, always beside a word |
| `ERR` | `#E8836F` | 7.2 | 6.5 | 5.9 | 5.2 | Failure text (never a filled background) |
| `DANGER` | `#E3A008` | 8.5 | 7.7 | 6.9 | 6.2 | **Reserved.** Destructive affordances and states only |

The two rules are not text and are not judged as text: `LINE` clears 1.4–1.6:1
and that is what a divider should measure. `LINE_STRONG` is the *component
boundary* token and clears 3:1 on every surface, which is why a control's edge and
an input's edge use it and a card's edge does not.

Surfaces step by only 1.10–1.14:1. That is deliberate: a card is read as a
surface because it is **a fill, plus a 1px `LINE` edge, plus a 20pt inner
margin**, not because the step is large. Stacking grey boxes with large luminance
steps is the other way to look generated.

`DATA_MAP` is a separate token because the map is the product's one light surface.
`DATA` measures 1.7:1 on OSM land; the track over a tile is `DATA_MAP` and the
casing under it is `WINDOW`, which is what makes a 5pt line legible on both land
and water (design review 2026-11-23, M1).

### Type

| Token | Size | Role |
|---|---|---|
| `DISPLAY` | 28 | The ending screen's one line ("端末が見つかりません"). Not used on a normal screen. |
| `TITLE` | 20 | A screen's title, one per screen |
| `BODY` | 16 | Prose, and the one sentence that says why a screen exists |
| `UI` | 14 | Buttons, tabs, table cells, a field's text, a card's caption |
| `CAPTION` | 12 | Units, timestamps, footnotes, the third tone |

Twelve is the floor: there is no 11pt text anywhere in the product. The scale is
12 → 14 → 16 → 20 → 28; adjacent steps are at least 2pt apart and the smallest
ratio is 1.14, which is what lets the two smallest steps carry two different jobs
(a caption and a control's label) instead of one.

- **One weight.** The OS face is installed at one weight and egui does not
  synthesise a bold, so hierarchy is size + tone (`ink` / `mute` / `faint`) +
  surface. Barlow 600 and Noto Sans JP bold are still not bundled (§10).
- **Monospace for figures.** `0x402F6`, coordinates, byte counts, `10,399`,
  zoom levels. Not decoration: a column of offsets is a column because the glyphs
  have one width.
- Fixed scale, no fluid type — desktop apps are viewed at a fixed DPI.

### Spacing, shape and elevation

4px base: `4 / 8 / 12 / 16 / 20 / 24 / 32 / 48`. A card's interior is 20, the gap
between panels is 24, between sections 32, and inside a row 8. Rows: **list 40,
drawer 36, table 32, table header 28**.

Radius: **6px**, one token (`theme::RADIUS`). Not 2 (a cut-off corner reads as a
printed form) and not 8 or more (too round for a dense instrument). Every control,
card and plate takes it.

Elevation is a **fill plus a 1px edge**. There is exactly **one shadow**
(`POPUP_SHADOW`: offset `[0,6]`, blur 18, `black` at alpha 96) and it means *this
layer floats*: a popup, a menu, the drawer when it floats over the canvas, a plate
over the map. A card does not get a shadow; it gets `SURFACE` and a `LINE` edge.

### Motion

90ms for a hover, 150ms for a screen change, 180ms for a drawer opening; all
ease-out, and all zero under the app's reduced-motion setting. There is no
page-load choreography, no hover growth, no blur, no parallax.

**Motion thesis.**

- *Focal moment:* the drawer's seam opening — one edge moving, and the map
  reclaiming the width behind it. It is the only motion in the product that says
  "the layout is about the map".
- *Continuity:* the firmware wizard's three steps, and the selection moving along
  the track.
- *Feedback:* the progress bar, a button's hover face, the `WRITE` field's gate.
  Nothing else acknowledges input with motion.
- *Budget:* a fill change and a width change. The map redraws on pan/zoom and is
  the only expensive surface; it never animates on its own. Reduced motion keeps
  every end state and removes the transition.

## 4. The signature move

```
 ┌──────────────────────────────────────────────────────────────┐
 │ m241   ログ                                  接続なし   [開く]│ 56 RAISED
 ├────┬───────────────────────────────────┬──────────────────────┤
 │ 接 │                                   │ トラック 写真 速度   │ 44 tab strip
 │ 続 │           地図キャンバス          │ 書き出し 操作        │
 │    │                                   ├──────────────────────┤
 │ ロ │        (SUNKEN, the page)         │  drawer body         │
 │ グ │                                   │  (SURFACE/RAISED)    │
 │    │                                   │                      │
 │フ  │                                   │                      │
 │ァ  │                                   ├──────────────────────┤
 │ム  │                                   │ [GPX][KML][KMZ]      │
 │    │                                   │                      │
 ├────┴───────────────────────────────────┴──────────────────────┤
 │ 10,399 点 · 42.3 km · 2h18m                  zoom 6.1 · OSM    │ 24
 └──────────────────────────────────────────────────────────────┘
          ↑ one vertical rule: the drawer's seam
```

**The rule.** There is exactly one full-height vertical rule in the product: the
1px `LINE` where the drawer meets the page. The navigation rail is a `WINDOW`
surface, not a ruled column; a card is a fill, not a boxed outline; a block is
separated by 24–32pt of space or by moving to a new surface, not by a rule. A
horizontal `LINE` remains correct in exactly two places: under a data row (a
figure row, a table row) and at the header's bottom edge, where the chrome ends.

Consequences for the build:

- Navigation is a 64pt rail of drawn 20×20 icons with 12pt labels, not the
  stepped thumb index. The staircase is gone with the manual: a tab that protrudes
  was the signature of a world view this design replaces.
- The drawer is a surface (`RAISED` when docked, `OVERLAY` + `POPUP_SHADOW` when
  it floats), and its edge is the seam. Its tabs are `choice_button`s on a 44pt
  strip inside it, so the only full-height rule stays the one.
- Everything drawn over the map — the transport, the zoom, the credit, the
  selected point — is a plate: `OVERLAY`, 1px `LINE`, the one radius, a fixed
  height. A plate is a floating layer, so it is the one place a card-like frame
  may carry the shadow.


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

### The previous generation, removed

The 2026-11-23 review diagnosed the old build as **a printed form on a dark
desk**: one flat ground (`#14181A`) for almost every surface, a 1px rule around
every block, 11pt text carrying most of the screen, an input field that was a hole
with no border, and a port ComboBox that painted egui's default `#323232` block.
Eight symptoms, and the fix was not to re-tune the colours but to replace the
grammar. This pass does that at the token and component level; the screen pass
(§5) applies it.

| Old symptom | What replaced it |
|---|---|
| Panels had a border and no fill | `common::card`: `SURFACE` fill + 1px `LINE` edge + `RADIUS` + 20pt inside |
| Surface hierarchy was 1.03–1.13:1 of nearly one colour | Five surfaces, each step 1.10–1.14:1, plus `SUNKEN` wells for inputs and the map |
| 11pt was the de-facto body size | The scale starts at 12; 14 is the working size |
| Inputs were invisible (`extreme_bg` was a near-black rail, no border) | `SUNKEN` well + `LINE_STRONG` edge; the theme derives the TextEdit frame so every field shares it |
| The ComboBox was an undesigned `#323232` block | Every `widgets.*` face is set explicitly (`RAISED` / `OVERLAY` / `SURFACE`) |
| Text-only buttons with no hover or focus | Four button classes, six states each, each with a fill, an edge and a 2px ink focus ring |
| No shadows at all, so a popup looked flat | One `POPUP_SHADOW`, only for floating layers |
| Rules did everything | A rule is a data boundary; blocks separate by surface and 20–32pt of space |

### Why these tokens

- **Five surfaces instead of three.** Three could not express "the page", "a card",
  "the chrome above a card", "a floating layer" and "a well *below* the page" at
  once, which is why the old build put cards and the window on one tone. The five
  are close in luminance on purpose (1.10–1.14:1); the card reads as a surface
  because of its fill *and* its 1px edge *and* its 20pt inner margin.
- **`SUNKEN` for inputs, not `RAISED`.** A field is a well: it takes an answer, so
  it sits below the sheet. A raised field reads as a button. §3 of the design
  direction names `SUNKEN`; the brief that commissioned this pass said `RAISED`
  in two places and `SUNKEN` in one — the well is the resolved choice, because
  the same document's input grammar (`well()`) is a `SUNKEN` frame and because a
  raised field would have re-created the input/button confusion the review found.
- **`LINE_STRONG` exists because `LINE` is a divider.** A control's edge is a
  WCAG 1.4.11 component boundary and needs 3:1; a card's edge is decoration and
  must stay quiet. One token for both would force one of the two wrong.
- **`FAINT` exists because dimming `MUTE` cannot be done safely.** egui's
  `disabled_alpha` would take a 6:1 caption under 4.5:1. A third tone is defined
  against all five surfaces and clears AA on every one.
- **`DATA_MAP` exists because the map is the one light surface.** `DATA` measures
  1.7:1 on OSM land; the track over tiles is `#1A6E82` (5.09 land / 3.64 water)
  over a `WINDOW` casing.
- **`RADIUS` is 6, not 2.** The invariant is "one radius", not "2px"; the value
  changed with the direction. The gate still rejects a literal `CornerRadius`.
- **The one shadow is a popup shadow.** Elevation is declared as *is this layer
  floating?*; the in-app chrome has no OS window, so `window_shadow` stays
  `Shadow::NONE` and a second `Shadow` literal is rejected by
  `tests/design_invariants.rs`.

### This pass's unfinished edges

- **The map's track still paints `DATA`, not `DATA_MAP`.** The token is defined,
  measured and pinned, but choosing the colour per surface (tiles vs the offline
  grid) belongs to the Log screen pass; on the offline grid `DATA_MAP` would be
  2.4:1 against `WINDOW`, so it cannot simply be swapped everywhere.
- **The navigation rail is still the old 176pt staircase.** It now paints
  `WINDOW` with an `OVERLAY` selected tab, but the 64pt drawn-icon rail of §4 is
  the screen pass's work. The rail's public geometry (`rail::STEP`, `tab_left`)
  is unchanged, so its five tests still describe the staircase.
- **The objects the type scale moved.** Raising `CAPTION` 11 → 12 and `UI`
  13 → 14 grew every existing screen by roughly 8%. The map plates' fixed heights,
  the settings page's fold and the Log screen's empty-state plate were laid out
  for the old scale; this pass re-measures the plates and records the change, and
  the screen pass replaces those layouts with §4's geometry. `common::heading`
  moved to 20 (`TITLE`), so a screen's title no longer shares 24 with the shell's
  own title.
- **`DISPLAY` (28) has no consumer yet.** It is the ending screen's step; no
  screen is an ending screen today, so the token and the `Heading` style are
  defined and the ordinary screens stay on 20.
- **Icons are not drawn yet.** §3 asks for a 20×20 drawn-icon set; this pass has
  the `metric::ICON` / `ICON_STROKE` figures only. Every control is still words or
  a drawn shape from the previous build.
- **The drawer is not built.** Its surface tokens (`RAISED` docked, `OVERLAY`
  floating, `POPUP_SHADOW`) are defined and the existing drawer column uses
  `RAISED`; its 44pt tab strip and the seam are the Log pass.
- **Plates do not carry the shadow yet.** They are `OVERLAY` at the existing 0.92
  channel weight (opaque, not translucent — `gamma_multiply` scales the channels,
  it does not composite) and a `LINE` edge; adopting `POPUP_SHADOW` on the plates
  is part of the same screen pass that restructures them.

### Superseded language in §5–§9 (to be fixed by the screen pass)

- §4 previously described the staircase; it now describes the seam. §5's Log
  figure, §6's rail/drawer states, §7's "rail blocks are ruled, not boxed" and
  §8's review protocol still use the staircase vocabulary and the old 24pt/11pt
  type names, and §7's "no shadows anywhere" is superseded by the one
  `POPUP_SHADOW`. They are listed here rather than edited, so that one pass owns
  the screens and the document together.
- §3's old palette table (`ground` / `panel` / `rail` / `rule`) is gone; a
  reviewer quoting those names from memory is quoting the previous direction.

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
  the `ink` / `mute` / `faint` tones: 28 and 20 are `ink`; 16, 14 and 12 choose by
  role. The five sizes are exact; the weight step between them is not.
- **Tabular figures.** `0x402F6` and `0x402F7` line up because every figure is
  monospace on the header's own column fractions (`common::table_header`,
  `common::table_row`), not because the proportional face happens to have
  tabular digits.

### Density and composition

- `common::figure_row` (height 32) and `common::figure_row_compact` (height 28)
  draw the caption at the left edge, the figure at the right edge and one `line`
  hairline underneath; `common::table_header` is 28. The rule is a data boundary,
  which is the only thing a rule is for (§2).
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
  and wraps to that width, so the two cannot meet; the layout keeps at least
  12pt (`space::M`) between the credit's plate and the zoom plate.
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

- primary = `INK` face with a `WINDOW` word, 40pt high; secondary = a
  transparent face with a `LINE_STRONG` edge, 40pt; a tool = a `RAISED` face at
  36pt, selected as an `OVERLAY` face with an `INK` word; destructive = the amber
  書き込む, still the only amber-filled control. A disabled class drops its face
  (to `WINDOW`), keeps a `FAINT` word and a `LINE` edge, and stays above 4.5:1 —
  dimming a designed contrast pair with egui's `disabled_alpha` is how the
  previous build took the disabled label under AA.
- Focus is the theme's 2pt `ink` outline on every control; a focused TextEdit
  gets egui's 1px `DATA` frame (0.37 derives it from `selection.stroke`), which
  is visible at 7.9:1 on the well. Selection uses an `OVERLAY` face plus an `INK`
  word (chips, segmented choices, the selected point's row) and never amber.
- The button classes set their state faces in a scoped `widgets.*` override
  rather than with `Button::fill()`. An explicit fill freezes a button at one
  state, which is why the previous build's buttons had no hover or pressed
  feedback at all — the "text-only button" symptom had a mechanical cause.
- An input field is a **well**: `visuals.text_edit_bg_color` is `SUNKEN`,
  `visuals.extreme_bg_color` is `SUNKEN`, and the theme's `inactive.bg_stroke` is
  `LINE_STRONG` because egui 0.37 derives a TextEdit's frame stroke from it. The
  shared grammar is `common::text_field` / `common::labelled_field`, so the
  caption position, the fill and the border are one decision; `common::well` is
  the frame for read-only content that belongs inside the page.
- egui 0.37's combo box, checkbox, slider and plain button all read
  `visuals.widgets.*`, so the port ComboBox now paints `RAISED` with a
  `LINE_STRONG` edge instead of the toolkit's `#323232`. This is the concrete fix
  behind "the controls look borrowed from Windows".
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
- The port ComboBox and hover tooltips carry the one `POPUP_SHADOW`; before this
  pass they carried none and the ComboBox wore egui's `#323232` default, because
  `theme::apply` set only the panel fill. The rail's tab transition is still
  150 ms ease-out (quadratic) because the staircase is still in the build —
  reduced motion drops it to 0. These are the design's own values, recorded
  because §3's motion and elevation rules are easy to lose to a toolkit default.
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
