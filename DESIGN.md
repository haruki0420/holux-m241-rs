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

```
┌─ m241 ────────────────────────────────────────────────────────────┐
│ ▌接続          │ ┌ 地図 ────────────────────────────────────────┐ │
│   ▌ログ        │ │                                              │ │
│     ▌ファーム  │ │        o----------o                          │ │
│       ▌設定    │ │       ╱            ╲                         │ │
│                │ │  o---'              o---o---o                │ │
│ ┌ トラック ──┐ │ │                          [ OpenStreetMap ]   │ │
│ │ [x] 11/14    │ │ └──────────────────────────────────────────────┘ │
│ │ [x] 11/15    │ │ ┌ 速度 ───────────────────────────────────────┐  │
│ │ [ ] 11/15    │ │ │ 60 ┤        ╭╮                              │  │
│ └────────────┘ │ │  0 ┴────────╯╰──────────────────────         │  │
│ ┌ 写真 ──────┐ │ └──────────────────────────────────────────────┘  │
│ │ [x] 0042.jpg │ │ ┌ 選択した点 ──────────────────────────────────┐  │
│ │ [x] 0043.jpg │ │ │ 2026-11-15 08:14:22 UTC  35.681236 139.767125│  │
│ └────────────┘ │ │ 標高 42 m  速度 18.4 km/h  衛星 9           │  │
│ ┌ 集計 ──────┐ │ └──────────────────────────────────────────────┘  │
│ │ 距離 42.3  │ │                                                   │
│ │ 時間 2h18m │ │  [ GPX ] [ KML ] [ KMZ ] [ CSV ] [ 写真に位置 ]   │
│ │ 平均 18.4  │ │                                                   │
│ └────────────┘ │                                                   │
└───────────────────────────────────────────────────────────────────┘
```

The left column is a **rail block**, not a second tab rail — same 8px staircase
rhythm, but subordinate: headers in `caption`, rows 28px, no fill.

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

Plain rows, no rail block: map cache size and location, units, theme
(light theme is not built; the row states that), language, reduced motion.

## 6. States the build must ship

| State | Where | Treatment |
|---|---|---|
| No port found | Connect | Rail tab stays; content says what to check, in the order to check it |
| Sync failed | Connect, Firmware | The device's own words first, the hint second |
| Device state undefined | Firmware step 3 failure | `danger` border on the panel, the recovery command as copyable text |
| Waiting for the user | Firmware step 2 | Progress held at 100%, the instruction given as an imperative |
| Empty log | Log | "まだログがありません" plus the download action |
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
- Table density on a 1024-wide window: the Log screen's three-zone layout needs a
  collapse rule below 1100px.

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
- The Log page's map is 160pt and its speed chart 80pt (`metric::PLOT`,
  `metric::CHART`). That is smaller than a first draft and is what lets the
  1280×800 page hold the heading, the file loader, the map, the speed chart and
  the pinned export strip without scrolling. The selected-point block and the
  point list do **not** fit above the fold at 1280×800 either: they are in the
  scrolling body and reached by scroll at both documented sizes. An earlier
  version of this section said §9 accepted the fold; §9 only leaves the collapse
  rule open, so that sentence was wrong.
- The Log screen's left column draws its three blocks (トラック / 写真 / 集計) on
  one left edge, which is how §5's figure draws them. §5's prose says "same 8px
  staircase rhythm"; the prose and the figure disagree, and the build follows the
  figure, because a second staircase would compete with the rail's signature.
- Body line-height 1.5 and the caption's +0.01em tracking are not implemented:
  egui 0.37 sizes text by font and has no per-style line-height or letter-spacing
  hook. The five sizes are exact; the leading and tracking are the OS face's.
- There are no table skeleton rows for a Loading state. The only loading state
  that exists is the read-only device probe, which has no table to skeleton, so
  it prints the probing status line instead of inventing rows.
- Footers are pinned, because the action is the reason the screen exists:
  Connect carries 再読込 (secondary) / 接続して確認 (primary); Log carries the
  export strip; Firmware carries 中止 / 続行・書き込む plus the step they belong
  to; Settings carries the version and the session-scope sentence.

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
- The firmware write button is armed by its gates, but pressing it reaches
  `attempt_flash`'s refusal: `m241-gui` does not link `m241-flash` (safety gate),
  so no write runs from this screen yet. The panel and screen say 見本, and the
  refusal names the reason after the press rather than hiding the control.

### Copy and states

- Device-panel row labels are Japanese: ファーム / ビルド / モデル / フラッシュ /
  日付. The values are unchanged; the English labels in §5's figure were the
  implementation's mixed-language leftover.
- The log-file input's placeholder is `ログファイルのパス`, not a
  `path/to/log.bin` sample.
- Empty log: 「まだログがありません」, the path with the ログを開く primary
  action, and a disabled ログを吸い出す marked 未検証.
- The map says 「地図タイルなし・軌跡のみ」. No tile fetcher is linked, so the
  canvas draws the track itself rather than claiming a map it does not have.
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
- `KMZ` is a disabled export format marked `未対応`: the exporter has no KMZ
  writer, and a button that quietly wrote GPX under a KMZ label would be worse.
- The selected point shows time, coordinate, altitude and speed. It does not show
  a satellite count because `TrackPoint` carries none — the log record has no
  such field. The record count is printed without a thousands separator
  (`10399`), not as §5's `10,399`.
- The Connect screen's next-step rows carry no `→` (the figure in §5 draws one;
  see below).
- Settings' cache location is `—（未使用）`. No tile fetcher is linked, so there
  is no cache directory to name; the row states that rather than printing a path
  nothing writes to.
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
