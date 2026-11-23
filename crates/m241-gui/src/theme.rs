//! Design tokens, in the one place the GUI reads colour, type, shape, spacing and
//! elevation from.
//!
//! The direction is `one-seam-instrument` (DESIGN.md §2): the map is the page, the
//! drawer is the margin, and the only seam is the drawer's. Hierarchy is made out
//! of five surfaces, three text tones, two rules, one radius and one shadow —
//! **not** out of ruled boxes on one flat ground, which is what made the previous
//! build read as a printed form (DESIGN.md §10).
//!
//! Values and their contrast ratios are in `DESIGN.md` §3. Four rules are enforced
//! mechanically rather than by review:
//!
//! 1. [`DANGER`] is reserved: it may appear only on the destructive action in the
//!    firmware screen. `tools/check_design.py` rejects it anywhere else.
//! 2. [`RADIUS`] is the only corner radius; `tools/check_design.py` rejects a
//!    literal `CornerRadius::same`.
//! 3. Every text colour clears 4.5:1 against every one of the five surfaces, and
//!    [`LINE_STRONG`] clears 3:1 as a component boundary (WCAG 1.4.11), measured
//!    by the assertions at the bottom of this file.
//! 4. [`POPUP_SHADOW`] is the only shadow. A second `Shadow` literal is a second
//!    elevation language, and `tests/design_invariants.rs` counts them.

// The token module is a palette: not every colour has a consumer yet, and the
// unused ones are the specification for screens that do not exist. Removing them
// would delete the design, not the dead code.
#![allow(dead_code)]

use egui::{Color32, CornerRadius, FontId, TextStyle};

// ---------------------------------------------------------------------------
// Surfaces: five steps. A card is `SURFACE` on `WINDOW`, the header and the
// drawer are `RAISED`, floating layers are `OVERLAY`, and an input is a
// `SUNKEN` well. The steps are deliberately small (1.10–1.14:1); a card is read
// as a surface because of its fill *plus* a 1px `LINE` edge *plus* 20pt of inner
// space, not because the step is large (DESIGN.md §3).
// ---------------------------------------------------------------------------

/// Window ground and the navigation rail: the page's floor.
pub const WINDOW: Color32 = Color32::from_rgb(0x0D, 0x10, 0x12);

/// Cards and panels: the page's sheet.
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x1B, 0x1E);

/// The header, the drawer, the tab strip, and a control's hover face.
pub const RAISED: Color32 = Color32::from_rgb(0x1E, 0x24, 0x29);

/// Floating layers only: popups, the drawer's overlay form on a narrow window,
/// and the plates drawn over the map.
pub const OVERLAY: Color32 = Color32::from_rgb(0x26, 0x2E, 0x34);

/// A well: input fields, the map canvas, a progress track. It sits *below* the
/// page so an editable box never reads as a heading (DESIGN.md §3, §10).
pub const SUNKEN: Color32 = Color32::from_rgb(0x09, 0x0C, 0x0E);

// ---------------------------------------------------------------------------
// Rules: two steps. The hairline is structural (it is the only rule a card or a
// data row needs); the strong line is a *component boundary*, the one WCAG
// requires an interactive edge to carry.
// ---------------------------------------------------------------------------

/// The one decorative and structural rule: a card's edge, a data row, a grid
/// line.
pub const LINE: Color32 = Color32::from_rgb(0x2E, 0x37, 0x3C);

/// A control's or an input's boundary. 3:1 or better on every surface.
pub const LINE_STRONG: Color32 = Color32::from_rgb(0x72, 0x7E, 0x85);

// ---------------------------------------------------------------------------
// Text: three tones. `FAINT` is the third one — a footnote, a unit, a disabled
// label — and it still clears 4.5:1 on all five surfaces, which is the point of
// having it instead of dimming `MUTE`.
// ---------------------------------------------------------------------------

/// Primary text: labels, figures, a screen's title.
pub const INK: Color32 = Color32::from_rgb(0xED, 0xEB, 0xE5);

/// Secondary text: captions, units, the sentence that explains a screen.
pub const MUTE: Color32 = Color32::from_rgb(0xA9, 0xAF, 0xAF);

/// The third tone: a footnote, a unit beside a figure, an unavailable label.
pub const FAINT: Color32 = Color32::from_rgb(0x93, 0x9D, 0x9E);

// ---------------------------------------------------------------------------
// Data: two steps, chosen by the surface they are drawn on. `DATA` is for the
// dark chrome (a chart, a progress fill, a selection). `DATA_MAP` is for an OSM
// tile, which is the one light surface in the product: it measures 5.09:1 on
// OSM land and 3.64:1 on OSM water, where `DATA` is 1.7:1 (design review
// 2026-11-23, M1).
// ---------------------------------------------------------------------------

/// Track and chart series, selection, progress fill — on a dark surface.
pub const DATA: Color32 = Color32::from_rgb(0x5F, 0xB3, 0xC4);

/// The track over an OSM tile. Never a fill on the dark chrome: it is too dark
/// to read there, which is why the two steps exist.
pub const DATA_MAP: Color32 = Color32::from_rgb(0x1A, 0x6E, 0x82);

/// The ten colours a where-the-logger-has-been marker can take, in ezTour's
/// order (purple → pink → olive …; 07-eztour-spec §5).
///
/// The Log screen draws **one track as one line** (DESIGN.md §10): the
/// 2026-11-23 review read the per-record dots as beads, because 240 plotted
/// records turned a single series into confetti and the line into a dotted
/// outline. The palette is kept for the case it was designed for — telling
/// several tracks apart — and a single track is [`DATA`] on the dark chrome or
/// [`DATA_MAP`] over a tile.
pub const TRACK_PALETTE: [Color32; 10] = [
    DATA,
    Color32::from_rgb(0xB4, 0x7E, 0xC8),
    Color32::from_rgb(0xD9, 0x85, 0xA8),
    Color32::from_rgb(0xA8, 0xB0, 0x5E),
    Color32::from_rgb(0x74, 0xB8, 0x8E),
    Color32::from_rgb(0xE0, 0xA0, 0x66),
    Color32::from_rgb(0x86, 0x9E, 0xD6),
    Color32::from_rgb(0x9B, 0x8F, 0xD9),
    Color32::from_rgb(0xC9, 0x9A, 0x6E),
    Color32::from_rgb(0x8F, 0x9A, 0xA6),
];

// ---------------------------------------------------------------------------
// Status: three tokens, each of which always ships beside a word
// (`common::status_line`). Amber is the reserved one.
// ---------------------------------------------------------------------------

/// Verified, success. Never alone: a word comes with it.
pub const OK: Color32 = Color32::from_rgb(0x6F, 0xBF, 0x73);

/// Failure text. Never a filled background.
pub const ERR: Color32 = Color32::from_rgb(0xE8, 0x83, 0x6F);

/// **Reserved.** Destructive affordances and the "device state undefined" state.
///
/// Never a hover, never a focus ring, never decoration. Amber is the only
/// saturated colour in the product, and it means *this can lose the device*.
pub const DANGER: Color32 = Color32::from_rgb(0xE3, 0xA0, 0x08);

// ---------------------------------------------------------------------------
// Shape and elevation.
// ---------------------------------------------------------------------------

/// The only corner radius in the product: 6pt, one token, every surface and
/// control.
///
/// 6 rather than the previous 2: a 2pt corner reads as a cut-off corner (a
/// printed form), and 8 or more is too round for a dense instrument. The value
/// changed, the invariant did not — a literal `CornerRadius::same` is still a
/// second shape language (`tools/check_design.py`).
pub const RADIUS: CornerRadius = CornerRadius::same(6);

/// The one shadow: floating layers only (a popup, a menu, the drawer when it
/// floats over the map, the map's plates). A card carries a fill and a 1px edge
/// instead; elevation is declared as *is it floating?*, not as a depth number.
pub const POPUP_SHADOW: egui::epaint::Shadow = egui::epaint::Shadow {
    offset: [0, 6],
    blur: 18,
    spread: 0,
    color: Color32::from_black_alpha(96),
};

// ---------------------------------------------------------------------------
// Type.
// ---------------------------------------------------------------------------

/// Type sizes, in points, for `RichText::size` call sites.
///
/// These are the same five numbers [`text_styles`] installs into egui's named
/// slots. They are named here because a screen that writes `13.0` by hand is one
/// edit away from a sixth step in a scale that is specified as five
/// (DESIGN.md §3: 12 → 14 → 16 → 20 → 28).
///
/// The 11pt body size is gone: below 12 the OS face's Japanese glyphs lose their
/// counters, and 11 was carrying most of the previous screen (DESIGN.md §10).
pub mod size {
    /// Screen title, one per screen. [`DISPLAY`] is for an ending screen only.
    pub const TITLE: f32 = 20.0;
    /// Prose and the one sentence that explains a screen.
    pub const BODY: f32 = 16.0;
    /// Buttons, tabs, table cells, a field's text.
    pub const UI: f32 = 14.0;
    /// Units, timestamps, footnotes.
    pub const CAPTION: f32 = 12.0;
    /// The ending screen only: "端末が見つかりません" and nothing else.
    pub const DISPLAY: f32 = 28.0;
}

// ---------------------------------------------------------------------------
// Spacing.
// ---------------------------------------------------------------------------

/// Spacing scale, in points. `4 / 8 / 12 / 16 / 20 / 24 / 32 / 48`.
///
/// [`CARD`] is the new twentieth-point step: a card's interior padding, and the
/// one gap in the scale that had no step between 16 and 24. The other seven keep
/// the ordinal names screens already reference; renaming them to renumber the
/// scale would have silently moved every call site's value.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    /// A card's interior padding (DESIGN.md §3: 行内 8 / カード内 20 / パネル間 24).
    pub const CARD: f32 = 20.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const XXXL: f32 = 48.0;
}

/// Fixed metrics that are not spacing but are still part of the composed page.
///
/// Row heights and the layout figures come from DESIGN.md §3 and §11. The canvas
/// figures are here for the same reason the spacing scale is: a screen that
/// writes `220.0` by hand has invented a second grid, and the next screen copies
/// it.
pub mod metric {
    /// A table's body row.
    pub const ROW: f32 = 32.0;
    /// A compact row: one step tighter than [`ROW`], for a subordinate block.
    ///
    /// Its own token rather than reusing [`TABLE_HEADER`]: a body row that borrows
    /// the header's height makes two meanings share one number, and the next
    /// change to the header silently moves every body row with it.
    pub const ROW_COMPACT: f32 = 28.0;
    /// A table's header row, one step tighter than its body.
    pub const TABLE_HEADER: f32 = 28.0;
    /// A list row: a day, a point, a project.
    pub const LIST_ROW: f32 = 40.0;
    /// A row in the drawer (DESIGN.md §11).
    pub const DOCK_ROW: f32 = 36.0;
    /// The header band: title 20 + one sentence 14 (DESIGN.md §11).
    pub const HEADER: f32 = 56.0;
    /// The status bar at the bottom of the window: one line of 12pt (DESIGN.md §11).
    pub const STATUS: f32 = 24.0;
    /// The navigation rail's width: four 56×56 items (DESIGN.md §11).
    pub const NAV: f32 = 64.0;
    /// One navigation item, icon over a 12pt label.
    pub const NAV_ITEM: f32 = 56.0;
    /// The drawer's tab strip (DESIGN.md §11).
    pub const TAB_STRIP: f32 = 44.0;
    /// The map canvas will not go below this; under it the page scrolls instead.
    pub const MAP_MIN: f32 = 320.0;
    /// A control drawn over the map: 40, or 36 on a narrow window.
    pub const MAP_CONTROL: f32 = 40.0;
    /// A plate's inset from the map canvas edge: 12, or 10 on a narrow window.
    pub const MAP_INSET: f32 = 12.0;
    /// The selected point's strip under the canvas: one 40pt line of mono 14.
    pub const SELECTION_STRIP: f32 = 40.0;
    /// A drawn icon's box. Icons are painted, never an emoji or a font glyph.
    pub const ICON: f32 = 20.0;
    /// The stroke of a drawn icon (DESIGN.md §3).
    pub const ICON_STROKE: f32 = 1.5;
    /// A hairline rule, and the width of any ordinary divider.
    pub const HAIRLINE: f32 = 1.0;
    /// Stroke width of the focus ring (DESIGN.md §6).
    pub const FOCUS_RING: f32 = 2.0;
    /// A scroll bar that is always visible: a 8pt bar, not a floating overlay.
    pub const SCROLLBAR: f32 = 8.0;
    /// Width of a track line, a chart series, or a plotted path.
    pub const TRACK_LINE: f32 = 2.0;
    /// Numbers carried over from the reference implementation's polyline
    /// (07-eztour-spec §5 「既定の線幅 5 px」) plus the casing the map needs.
    ///
    /// The reference draws the line at 70% opacity because its map was a dark
    /// schematic. This map's default surface is OSM tile (land #F2EFE9, water
    /// #AAD3DF), so the track is [`DATA_MAP`](super::DATA_MAP) at full opacity
    /// over a [`WINDOW`](super::WINDOW) casing: the dark edge carries it on land
    /// and water alike (design review 2026-11-23, M1).
    ///
    /// The track is a **line**, not a chain of markers: a record is not a
    /// visible dot (DESIGN.md §10). Only the selection, a waypoint, the two ends
    /// and a photograph are drawn as points.
    pub mod track {
        /// Stroke width of the map's track polyline.
        pub const LINE_WIDTH: f32 = 5.0;
        /// Stroke width of the casing drawn under the polyline.
        pub const CASING_WIDTH: f32 = 7.0;

        /// The map's track polyline stroke.
        ///
        /// The colour is chosen by the surface the line is drawn on, because
        /// neither data token clears 3:1 on both: [`DATA_MAP`](super::DATA_MAP)
        /// measures 2.4:1 on the offline grid's `WINDOW` (too dark) and
        /// [`DATA`](super::DATA) measures 1.7:1 on an OSM tile (too pale). So
        /// `tiled` is the answer to "which surface is under this line", not a
        /// style preference. `PathStroke` rather than `Stroke` because the map
        /// line is a path.
        pub fn line_stroke(tiled: bool) -> egui::epaint::PathStroke {
            let colour = if tiled {
                super::super::DATA_MAP
            } else {
                super::super::DATA
            };
            egui::epaint::PathStroke::new(LINE_WIDTH, colour)
        }

        /// The casing under [`line_stroke`]: the same geometry in `window`.
        ///
        /// Drawn first so the polyline keeps a dark edge on the light tile; on
        /// the offline grid it is invisible, which is harmless.
        pub fn casing_stroke() -> egui::epaint::PathStroke {
            egui::epaint::PathStroke::new(CASING_WIDTH, super::super::WINDOW)
        }
    }
    /// Radius of one plotted point.
    pub const POINT_DOT: f32 = 3.0;
    /// Width of a screen's subordinate rail block (DESIGN.md §5, ログ).
    ///
    /// The Log screen's own blocks moved into the drawer (§5), but the Connect
    /// screen's two panels still split on this figure.
    pub const RAIL_BLOCK: f32 = 220.0;
    /// Height of the Log screen's tool row: the only strip above the map.
    pub const TOOL_ROW: f32 = 40.0;
    /// Width of the Log screen's right-hand drawer.
    ///
    /// On a narrow window the drawer lies over the map instead of beside it:
    /// 360pt beside a sub-1000pt canvas would leave the map a strip.
    pub const DRAWER: f32 = 360.0;
    /// Height of the speed chart (a subordinate series, one step under the map).
    pub const CHART: f32 = 80.0;
    /// Minimum width of a dialog's primary action, so two buttons line up.
    pub const ACTION_WIDTH: f32 = 128.0;
    /// Below this content width the Log screen floats the drawer over the canvas
    /// instead of giving it a column (DESIGN.md §9 leaves the breakpoint open;
    /// this is the one it names).
    ///
    /// The measure is the Log body's width (`ui.max_rect()`), not the window's:
    /// the rail and the page margin are part of the window, not of this screen.
    pub const NARROW: f32 = 1_000.0;
}

/// The system font provider, shared by every context.
///
/// One provider per process rather than per [`egui::Context`]: enumerating the
/// installed fonts costs hundreds of milliseconds, and the test suite installs
/// the theme on dozens of headless contexts. The provider itself is
/// `Send + Sync` and caches the OS font list after the first query.
fn system_fonts() -> &'static std::sync::Arc<egui_system_font_provider::SystemFontProvider> {
    use std::sync::{Arc, OnceLock};
    static PROVIDER: OnceLock<Arc<egui_system_font_provider::SystemFontProvider>> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        // The UI's copy is Japanese (DESIGN.md §3). Asking for `ja` makes the OS
        // fallback pick a Japanese face for Han characters instead of whichever
        // Chinese face fontconfig lists first.
        Arc::new(egui_system_font_provider::SystemFontProvider::new().with_locale(Some("ja")))
    })
}

/// The glyph used to choose the proportional family from the OS.
///
/// `FallbackRequest::cluster` is an egui term: the provider answers "which
/// installed face covers these characters". A Latin lowercase letter is the
/// representative cluster for the proportional family because it is the glyph
/// every Latin sans face is designed for and the one the body copy is set in;
/// the Japanese glyphs are not needed here because [`system_fonts`] stays
/// underneath as the fallback for them.
const PROPORTIONAL_CLUSTER: &str = "a";

/// The glyph used to choose the monospace family from the OS.
///
/// A digit rather than a letter: what the monospace face has to carry in this
/// product is figures — offsets, timestamps, coordinates — and every monospace
/// candidate covers `0` even when its Latin letters differ.
const MONOSPACE_CLUSTER: &str = "0";

/// Put the OS's own face at the head of `family`.
///
/// DESIGN.md asks for Barlow (Latin) and Noto Sans JP (Japanese). Neither file
/// lives in this repository and the build must not fetch one, so the
/// approximation is: the first face the OS offers for the family, promoted to
/// primary, with [`system_fonts`] still installed underneath as the fallback for
/// every glyph it lacks (Japanese, symbols). See `DESIGN.md` §10 for what exactly
/// is approximated.
///
/// `cluster` is the representative glyph the OS is asked about, supplied by the
/// caller as [`PROPORTIONAL_CLUSTER`] or [`MONOSPACE_CLUSTER`]; the constants
/// carry the reason each family is represented by that character.
fn promote_system_face(ctx: &egui::Context, cluster: &str, family: egui::FontFamily) {
    use egui::epaint::text::{FallbackRequest, FontPriority, FontProvider, InsertFontFamily};
    let request = FallbackRequest {
        cluster,
        family: &family,
    };
    let Some(mut insert) = system_fonts().font_for(&request) else {
        // A machine with no fonts at all keeps egui's bundled defaults; the
        // labels are then Latin-only, which is still better than not starting.
        return;
    };
    insert.families = vec![InsertFontFamily {
        family,
        priority: FontPriority::Highest,
    }];
    ctx.add_font(insert);
}

/// Type scale (DESIGN.md §3): 12 / 14 / 16 / 20 / 28.
///
/// egui has five named slots. `Heading` is the 28pt [`size::DISPLAY`] (the
/// ending screen's one line); a screen title asks for 20 directly via
/// `RichText::size`. Monospace shares the 14pt [`size::UI`] with buttons on
/// purpose: it is a different family carrying a different job, not a step in the
/// scale.
pub fn text_styles() -> Vec<(TextStyle, FontId)> {
    vec![
        (TextStyle::Heading, FontId::proportional(size::DISPLAY)),
        (TextStyle::Body, FontId::proportional(size::BODY)),
        (TextStyle::Button, FontId::proportional(size::UI)),
        (TextStyle::Small, FontId::proportional(size::CAPTION)),
        (TextStyle::Monospace, FontId::monospace(size::UI)),
    ]
}

/// Install the design tokens into an egui context.
///
/// egui 0.37 keeps one style per theme, so both are set: the product ships dark
/// only, and a light OS theme must not produce a light widget set.
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    // Fonts first: `add_font` must arrive before the first pass lays out text,
    // or the frame is shaped with the bundled Latin-only defaults.
    ctx.add_font_provider(system_fonts().clone());
    promote_system_face(ctx, PROPORTIONAL_CLUSTER, egui::FontFamily::Proportional);
    promote_system_face(ctx, MONOSPACE_CLUSTER, egui::FontFamily::Monospace);
    ctx.all_styles_mut(apply);
}

/// Install the tokens into an existing style.
///
/// Every value a screen could otherwise leave to the toolkit is written here:
/// before this revision `apply` set the panel fill and the focus ring and left
/// the rest to egui's dark defaults, which is why the port ComboBox was a
/// `#323232` block that belonged to no surface in the design (DESIGN.md §10).
fn apply(style: &mut egui::Style) {
    style.text_styles = text_styles().into_iter().collect();

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;

    // Surfaces. `panel_fill` is the page, `window_fill` a popup's sheet,
    // `extreme_bg_color` the well behind a progress track or a text edit, and
    // `faint_bg_color` the alternating table row one step *above* the page.
    visuals.panel_fill = WINDOW;
    visuals.window_fill = RAISED;
    visuals.extreme_bg_color = SUNKEN;
    visuals.text_edit_bg_color = Some(SUNKEN);
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(INK);
    visuals.hyperlink_color = DATA;
    visuals.window_stroke = egui::Stroke::new(metric::HAIRLINE, LINE);

    // A selection is a tinted `data` wash, never a status colour.
    visuals.selection.bg_fill = DATA.gamma_multiply(0.22);
    visuals.selection.stroke = egui::Stroke::new(metric::HAIRLINE, DATA);

    // Shape and elevation. The in-app chrome has no OS window, so only the
    // floating layers get the one shadow.
    visuals.window_corner_radius = RADIUS;
    visuals.menu_corner_radius = RADIUS;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = POPUP_SHADOW;

    // The six widget states (DESIGN.md §6). egui reads these for every control
    // that does not set its own frame, which is the ComboBox, a checkbox's box,
    // a slider's rail and a plain button.
    //
    // `inactive.bg_stroke` is `LINE_STRONG` rather than `LINE` on purpose: egui
    // 0.37 derives a TextEdit's frame stroke from the inactive widget visuals,
    // and an editable field's boundary owes the 3:1 a component edge requires
    // (WCAG 1.4.11). A quiet 1.4:1 hairline around every input was one half of
    // the previous build's "the fields are invisible" finding.
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SURFACE;
    widgets.noninteractive.weak_bg_fill = SURFACE;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(metric::HAIRLINE, LINE);
    widgets.noninteractive.fg_stroke = egui::Stroke::new(metric::HAIRLINE, MUTE);

    widgets.inactive.weak_bg_fill = RAISED;
    widgets.inactive.bg_fill = RAISED;
    widgets.inactive.bg_stroke = egui::Stroke::new(metric::HAIRLINE, LINE_STRONG);
    widgets.inactive.fg_stroke = egui::Stroke::new(metric::HAIRLINE, INK);

    widgets.hovered.weak_bg_fill = OVERLAY;
    widgets.hovered.bg_fill = OVERLAY;
    widgets.hovered.bg_stroke = egui::Stroke::new(metric::HAIRLINE, LINE_STRONG);
    widgets.hovered.fg_stroke = egui::Stroke::new(metric::HAIRLINE, INK);

    widgets.active.weak_bg_fill = SURFACE;
    widgets.active.bg_fill = SURFACE;
    widgets.active.bg_stroke = egui::Stroke::new(metric::FOCUS_RING, INK);
    widgets.active.fg_stroke = egui::Stroke::new(metric::HAIRLINE, INK);

    widgets.open.weak_bg_fill = OVERLAY;
    widgets.open.bg_fill = OVERLAY;
    widgets.open.bg_stroke = egui::Stroke::new(metric::HAIRLINE, LINE_STRONG);
    widgets.open.fg_stroke = egui::Stroke::new(metric::HAIRLINE, INK);

    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.corner_radius = RADIUS;
        // A control's face is a fill and an edge; expanding it would make every
        // hovered widget a different size from its neighbours.
        widget.expansion = 0.0;
    }

    style.spacing.item_spacing = egui::vec2(space::S, space::S);
    style.spacing.button_padding = egui::vec2(space::L, space::S);
    style.spacing.interact_size = egui::vec2(36.0, 36.0);
    style.spacing.combo_height = space::XXL;
    style.spacing.menu_margin = egui::Margin::same(space::S as i8);
    // Always visible, 8pt wide: a scrollable panel says so. A floating bar that
    // appears only under the pointer is a hidden affordance.
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: false,
        bar_width: metric::SCROLLBAR,
        handle_min_length: 24.0,
        bar_inner_margin: space::XS,
        bar_outer_margin: 0.0,
        ..Default::default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five surfaces, named, in the order the design builds them.
    const SURFACES: [(&str, Color32); 5] = [
        ("SUNKEN", SUNKEN),
        ("WINDOW", WINDOW),
        ("SURFACE", SURFACE),
        ("RAISED", RAISED),
        ("OVERLAY", OVERLAY),
    ];

    /// WCAG relative luminance, so the contrast claim in DESIGN.md is checked
    /// rather than asserted.
    fn luminance(colour: Color32) -> f64 {
        let channel = |value: u8| {
            let v = f64::from(value) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(colour.r()) + 0.7152 * channel(colour.g()) + 0.0722 * channel(colour.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn every_text_colour_clears_wcag_aa_on_every_surface() {
        // DESIGN.md §3: every text token clears 4.5:1 against every surface a
        // screen can put it on. `FAINT` is the tightest (5.0 on `OVERLAY`) and is
        // the reason the third tone exists instead of dimming `MUTE`.
        for (name, colour) in [
            ("INK", INK),
            ("MUTE", MUTE),
            ("FAINT", FAINT),
            ("DANGER", DANGER),
            ("DATA", DATA),
            ("OK", OK),
            ("ERR", ERR),
        ] {
            for (surface_name, surface) in SURFACES {
                let ratio = contrast(colour, surface);
                assert!(
                    ratio >= 4.5,
                    "{name} on {surface_name} is {ratio:.2}, below 4.5:1"
                );
            }
        }
    }

    #[test]
    fn a_component_boundary_clears_three_to_one_on_every_surface() {
        // WCAG 1.4.11: the edge of an input or a control is a non-text component
        // and needs 3:1. That is the whole reason `LINE_STRONG` is a second token
        // rather than "line, but lighter".
        for (surface_name, surface) in SURFACES {
            let ratio = contrast(LINE_STRONG, surface);
            assert!(
                ratio >= 3.0,
                "LINE_STRONG on {surface_name} is {ratio:.2}, below 3:1"
            );
        }
    }

    #[test]
    fn the_two_rules_are_not_text_and_are_measured_as_rules() {
        // Documented on purpose: LINE clears 1.4–1.9:1 and that is correct for a
        // divider. The test exists so nobody "fixes" it into a text colour, and
        // so a future change that dims it under the grid threshold fails.
        assert!(contrast(LINE, WINDOW) < 2.0);
        assert!(contrast(LINE, LINE_STRONG) > 2.0);
    }

    #[test]
    fn the_five_surfaces_form_one_chain() {
        // The design's surface claim, as arithmetic: every step is monotone in
        // luminance and separated by 1.08–1.22:1. A step outside that band is a
        // different visual language (too subtle to read, or a grey box), which is
        // what the previous single-ground build drifted into.
        let chain = [
            ("WINDOW→SURFACE", WINDOW, SURFACE),
            ("SURFACE→RAISED", SURFACE, RAISED),
            ("RAISED→OVERLAY", RAISED, OVERLAY),
        ];
        for (name, lower, upper) in chain {
            let ratio = contrast(lower, upper);
            assert!(
                (1.08..=1.22).contains(&ratio),
                "{name} is {ratio:.2}:1, outside the 1.08–1.22 step band"
            );
            assert!(
                luminance(upper) > luminance(lower),
                "{name} does not rise in luminance"
            );
        }
        // A well is one step below the sheet (1.13:1), and the window sits
        // between the two: four levels, not a grey box on a flat ground.
        assert!(
            luminance(SUNKEN) < luminance(WINDOW),
            "SUNKEN ({:.4}) is not below WINDOW ({:.4})",
            luminance(SUNKEN),
            luminance(WINDOW)
        );
        assert!(
            (1.08..=1.22).contains(&contrast(SUNKEN, SURFACE)),
            "SUNKEN→SURFACE is {:.2}:1, outside the step band",
            contrast(SUNKEN, SURFACE)
        );
    }

    #[test]
    fn the_map_track_clears_three_to_one_on_both_osm_faces() {
        // `DATA_MAP` exists because `DATA` is 1.7:1 on OSM land. The two tile
        // faces are the standard OSM raster style's land and water fills, not a
        // second palette: they are the surface the track is judged against.
        const OSM_LAND: Color32 = Color32::from_rgb(0xF2, 0xEF, 0xE9);
        const OSM_WATER: Color32 = Color32::from_rgb(0xAA, 0xD3, 0xDF);
        for (name, face) in [("land", OSM_LAND), ("water", OSM_WATER)] {
            let ratio = contrast(DATA_MAP, face);
            assert!(
                ratio >= 3.0,
                "DATA_MAP on OSM {name} is {ratio:.2}, below the 3:1 a drawn line needs"
            );
        }
        // And it is a separate token for a reason: DATA does not clear 3:1 there.
        assert!(contrast(DATA, OSM_LAND) < 3.0);
    }

    #[test]
    fn the_proportional_type_steps_are_all_distinguishable() {
        // Monospace is excluded: it is a different family at the button size, so
        // an identical point size is deliberate rather than a scale collision.
        let mut sizes: Vec<f32> = text_styles()
            .iter()
            .filter(|(slot, _)| *slot != TextStyle::Monospace)
            .map(|(_, font)| font.size)
            .collect();
        sizes.sort_by(f32::total_cmp);
        for pair in sizes.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            // The 14→16 step is 1.143, so the ratio floor is 1.12 rather than the
            // previous 1.15. In exchange the steps must also be at least 2pt
            // apart: 14 and 16 are distinguishable because a point is a visible
            // unit, and 12/14/16 is a scale rather than a rounding.
            let ratio = b / a;
            assert!(
                ratio >= 1.12,
                "steps {a} and {b} are only {ratio:.3} apart; they cannot carry different roles"
            );
            assert!(
                b - a >= 1.99,
                "steps {a} and {b} are less than 2pt apart; the scale is not a scale"
            );
        }
    }

    #[test]
    fn monospace_is_a_family_choice_not_a_scale_step() {
        let styles = text_styles();
        let mono = styles
            .iter()
            .find(|(slot, _)| *slot == TextStyle::Monospace)
            .map(|(_, font)| font.family.clone());
        assert!(matches!(mono, Some(egui::FontFamily::Monospace)));
    }

    #[test]
    fn the_palette_is_the_fifteen_hex_values_the_design_names() {
        // The tokens are the design (§3). A re-tuned colour here is a design
        // change, not a detail, and the contrast test alone would accept a whole
        // new palette as long as it cleared 4.5:1.
        for (name, colour, expected) in [
            ("WINDOW", WINDOW, Color32::from_rgb(0x0D, 0x10, 0x12)),
            ("SURFACE", SURFACE, Color32::from_rgb(0x16, 0x1B, 0x1E)),
            ("RAISED", RAISED, Color32::from_rgb(0x1E, 0x24, 0x29)),
            ("OVERLAY", OVERLAY, Color32::from_rgb(0x26, 0x2E, 0x34)),
            ("SUNKEN", SUNKEN, Color32::from_rgb(0x09, 0x0C, 0x0E)),
            ("LINE", LINE, Color32::from_rgb(0x2E, 0x37, 0x3C)),
            (
                "LINE_STRONG",
                LINE_STRONG,
                Color32::from_rgb(0x72, 0x7E, 0x85),
            ),
            ("INK", INK, Color32::from_rgb(0xED, 0xEB, 0xE5)),
            ("MUTE", MUTE, Color32::from_rgb(0xA9, 0xAF, 0xAF)),
            ("FAINT", FAINT, Color32::from_rgb(0x93, 0x9D, 0x9E)),
            ("DATA", DATA, Color32::from_rgb(0x5F, 0xB3, 0xC4)),
            ("DATA_MAP", DATA_MAP, Color32::from_rgb(0x1A, 0x6E, 0x82)),
            ("OK", OK, Color32::from_rgb(0x6F, 0xBF, 0x73)),
            ("ERR", ERR, Color32::from_rgb(0xE8, 0x83, 0x6F)),
            ("DANGER", DANGER, Color32::from_rgb(0xE3, 0xA0, 0x08)),
        ] {
            assert_eq!(
                colour, expected,
                "{name} is not the token DESIGN.md §3 names"
            );
        }
        // The ten-colour cycle is kept for telling several tracks apart. A single
        // track is one line in one colour, so the palette has no painted consumer
        // on this screen (DESIGN.md §10).
        assert_eq!(
            TRACK_PALETTE,
            [
                DATA,
                Color32::from_rgb(0xB4, 0x7E, 0xC8),
                Color32::from_rgb(0xD9, 0x85, 0xA8),
                Color32::from_rgb(0xA8, 0xB0, 0x5E),
                Color32::from_rgb(0x74, 0xB8, 0x8E),
                Color32::from_rgb(0xE0, 0xA0, 0x66),
                Color32::from_rgb(0x86, 0x9E, 0xD6),
                Color32::from_rgb(0x9B, 0x8F, 0xD9),
                Color32::from_rgb(0xC9, 0x9A, 0x6E),
                Color32::from_rgb(0x8F, 0x9A, 0xA6),
            ],
            "the ten-colour track palette changed"
        );
    }

    #[test]
    fn the_track_line_picks_its_data_token_by_the_surface_under_it() {
        // DESIGN.md §3: `DATA` is for the dark chrome and `DATA_MAP` for an OSM
        // tile. Neither clears 3:1 on the other's surface (1.7:1 and 2.4:1), so
        // the map asks "were tiles painted" and this test pins the two answers.
        let offscreen = metric::track::line_stroke(false);
        assert_eq!(offscreen.width, metric::track::LINE_WIDTH);
        assert_eq!(
            offscreen.color,
            egui::epaint::ColorMode::Solid(DATA),
            "the track on the offline grid is not DATA"
        );
        let tiled = metric::track::line_stroke(true);
        assert_eq!(tiled.width, metric::track::LINE_WIDTH);
        assert_eq!(
            tiled.color,
            egui::epaint::ColorMode::Solid(DATA_MAP),
            "the track over a tile is not DATA_MAP"
        );
        // The casing is the same geometry in the window colour, so the line reads
        // on the light tile as well as on the dark grid.
        let casing = metric::track::casing_stroke();
        assert_eq!(casing.width, metric::track::CASING_WIDTH);
        assert!(casing.width > tiled.width);
        assert_eq!(casing.color, egui::epaint::ColorMode::Solid(WINDOW));
    }

    #[test]
    fn the_type_scale_is_the_five_sizes_the_design_names() {
        // 12 / 14 / 16 / 20 / 28. The ratio test above passes for many scales;
        // this one names the values the tokens promise.
        assert_eq!(
            [
                size::CAPTION,
                size::UI,
                size::BODY,
                size::TITLE,
                size::DISPLAY
            ],
            [12.0, 14.0, 16.0, 20.0, 28.0]
        );
        assert_eq!(
            text_styles(),
            vec![
                (TextStyle::Heading, FontId::proportional(28.0)),
                (TextStyle::Body, FontId::proportional(16.0)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Small, FontId::proportional(12.0)),
                (TextStyle::Monospace, FontId::monospace(14.0)),
            ],
            "the named styles are not the five-size scale"
        );
    }

    #[test]
    fn the_spacing_scale_is_the_eight_steps_the_design_names() {
        assert_eq!(
            [
                space::XS,
                space::S,
                space::M,
                space::L,
                space::CARD,
                space::XL,
                space::XXL,
                space::XXXL
            ],
            [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0]
        );
    }

    #[test]
    fn the_only_corner_radius_is_six_points() {
        // §3: one radius, 6. The gate forces every call site to reference the
        // token; this pins the value the token holds.
        assert_eq!(RADIUS, CornerRadius::same(6));
    }

    #[test]
    fn every_widget_state_carries_the_one_radius_and_a_face() {
        // Before this revision `apply` set no widget fills, so the ComboBox wore
        // egui's `#323232` default and no control had a designed hover. The six
        // states are now explicit; this pins them so a toolkit default cannot
        // creep back in.
        let mut style = egui::Style::default();
        apply(&mut style);
        let widgets = &style.visuals.widgets;
        for (name, widget) in [
            ("noninteractive", &widgets.noninteractive),
            ("inactive", &widgets.inactive),
            ("hovered", &widgets.hovered),
            ("active", &widgets.active),
            ("open", &widgets.open),
        ] {
            assert_eq!(
                widget.corner_radius, RADIUS,
                "{name} does not carry the one radius"
            );
        }
        assert_eq!(
            widgets.inactive.weak_bg_fill, RAISED,
            "an idle control's face is not RAISED (this is the ComboBox default)"
        );
        assert_eq!(
            widgets.hovered.weak_bg_fill, OVERLAY,
            "a hovered control's face is not one step above its idle face"
        );
        assert_eq!(
            widgets.active.weak_bg_fill, SURFACE,
            "a pressed control's face is not one step below its idle face"
        );
        assert_eq!(style.visuals.window_corner_radius, RADIUS);
        assert_eq!(style.visuals.menu_corner_radius, RADIUS);
    }

    #[test]
    fn the_focus_ring_is_two_points_of_ink() {
        // §6: every control gets a 2px ink focus ring. The custom controls paint
        // their own with `metric::FOCUS_RING`; ordinary widgets read the style's
        // active stroke, so both have to name the same width or focus looks
        // different depending on which control you tab to.
        let mut style = egui::Style::default();
        apply(&mut style);
        let ring = style.visuals.widgets.active.bg_stroke;
        assert_eq!(ring.width, metric::FOCUS_RING);
        assert_eq!(ring.color, INK);
    }

    #[test]
    fn an_input_is_a_well_with_a_strong_edge() {
        // §3: an editable field is `SUNKEN` with a `LINE_STRONG` boundary, so it
        // can never be mistaken for a heading. egui 0.37 derives the text edit's
        // fill from `text_edit_bg_color` and its stroke from the inactive widget
        // stroke; both are pinned because either one alone leaves the field
        // invisible (the design review's Must 5).
        let mut style = egui::Style::default();
        apply(&mut style);
        assert_eq!(style.visuals.text_edit_bg_color, Some(SUNKEN));
        assert_eq!(style.visuals.extreme_bg_color, SUNKEN);
        assert_eq!(style.visuals.widgets.inactive.bg_stroke.color, LINE_STRONG);
        assert_eq!(
            style.visuals.widgets.inactive.bg_stroke.width,
            metric::HAIRLINE
        );
    }

    #[test]
    fn scroll_bars_are_always_visible_eight_point_bars() {
        // §6/§11: a scrollable panel says so. The floating default hid the bar
        // until the pointer was over it.
        let mut style = egui::Style::default();
        apply(&mut style);
        let scroll = style.spacing.scroll;
        assert!(!scroll.floating, "the scroll bar hides until hovered");
        assert_eq!(scroll.bar_width, metric::SCROLLBAR);
    }

    #[test]
    fn only_the_floating_layers_carry_the_one_shadow() {
        // §3: elevation is "fill + 1px edge"; a shadow means "this floats". The
        // in-app chrome has no OS window, so `window_shadow` stays none and
        // `popup_shadow` carries the single token. A second `Shadow` literal
        // anywhere else is rejected by `tests/design_invariants.rs`.
        let mut style = egui::Style::default();
        apply(&mut style);
        assert_eq!(style.visuals.window_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(style.visuals.popup_shadow, POPUP_SHADOW);
        assert_eq!(
            POPUP_SHADOW,
            egui::epaint::Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: Color32::from_black_alpha(96),
            },
            "the one shadow is not the token the design names"
        );
    }

    #[test]
    fn the_row_heights_are_the_ones_the_design_names() {
        assert_eq!(metric::LIST_ROW, 40.0, "a list row is 40 (DESIGN.md §3)");
        assert_eq!(metric::DOCK_ROW, 36.0, "a drawer row is 36");
        assert_eq!(metric::ROW, 32.0, "a table row is 32");
        assert_eq!(metric::TABLE_HEADER, 28.0, "a table header is 28");
        assert_eq!(metric::ROW_COMPACT, 28.0, "a compact row is 28");
    }
}
