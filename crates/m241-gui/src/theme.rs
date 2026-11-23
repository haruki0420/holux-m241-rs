//! Design tokens, in the one place the GUI reads colour and type from.
//!
//! Values and their contrast ratios are in `DESIGN.md` §3. Two rules are
//! enforced mechanically rather than by review:
//!
//! 1. [`DANGER`] is reserved: it may appear only on the destructive action in
//!    the firmware screen. `tools/check_design.py` rejects it anywhere else.
//! 2. Every text colour clears 4.5:1 against both [`GROUND`] and [`PANEL`],
//!    measured by the assertions at the bottom of this file.

// The token module is a palette: not every colour has a consumer yet, and the
// unused ones are the specification for screens that do not exist. Removing them
// would delete the design, not the dead code.
#![allow(dead_code)]

use egui::{Color32, CornerRadius, FontId, TextStyle};

/// Window background and content ground.
pub const GROUND: Color32 = Color32::from_rgb(0x14, 0x18, 0x1A);

/// Raised surface: cards, table headers.
pub const PANEL: Color32 = Color32::from_rgb(0x1B, 0x20, 0x23);

/// The tab rail, inset from the content.
pub const RAIL: Color32 = Color32::from_rgb(0x11, 0x14, 0x16);

/// Hairlines between rows and sections.
pub const RULE: Color32 = Color32::from_rgb(0x2C, 0x34, 0x37);

/// Primary text.
pub const INK: Color32 = Color32::from_rgb(0xE8, 0xE6, 0xDF);

/// Secondary text, units, captions.
pub const MUTE: Color32 = Color32::from_rgb(0x9A, 0xA0, 0xA0);

/// **Reserved.** Destructive affordances and the "device state undefined" state.
///
/// Never a hover, never a focus ring, never decoration. Amber is the only
/// saturated colour in the product, and it means *this can lose the device*.
pub const DANGER: Color32 = Color32::from_rgb(0xE3, 0xA0, 0x08);

/// Track line, chart series, map selection.
pub const DATA: Color32 = Color32::from_rgb(0x5F, 0xB3, 0xC4);

/// The ten colours a where-the-logger-has-been marker can take, in ezTour's
/// order (purple → pink → olive …; 07-eztour-spec §5).
///
/// The product paints one track with one line, so the cycle shows up on the
/// plotted records rather than on ten polylines: the trip is still a single
/// series in [`DATA`], and the record dots carry the sequence. The hues are
/// desaturated to sit on `ground` and `panel` without competing with the ink
/// that marks the selection; [`DATA`] is the first entry, so a track less than
/// ten points long stays in the product's own colour.
///
/// Only the offline canvas uses them. On an OSM tile the pale hues measure
/// below 2:1 against land (#F2EFE9), so the map's records switch to the
/// `ground` fill and `ink` ring described in [`metric::track`]; the cycle is
/// what the schematic grid was designed around.
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

/// Verified, success.
pub const OK: Color32 = Color32::from_rgb(0x6F, 0xBF, 0x73);

/// Failure text. Never a filled background.
pub const ERR: Color32 = Color32::from_rgb(0xE2, 0x70, 0x5E);

/// The only corner radius in the product. A manual's tabs are square-cut.
pub const RADIUS: CornerRadius = CornerRadius::same(2);

/// Spacing scale, in points. `4 / 8 / 12 / 16 / 24 / 32 / 48`.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const XXXL: f32 = 48.0;
}

/// Type sizes, in points, for `RichText::size` call sites.
///
/// These are the same five numbers [`text_styles`] installs into egui's named
/// slots. They are named here because a screen that writes `13.0` by hand is
/// one edit away from a sixth step in a scale that is specified as five
/// (DESIGN.md §3: 11 → 13 → 16 → 20 → 24).
pub mod size {
    /// Screen title, one per screen.
    pub const DISPLAY: f32 = 24.0;
    /// Section heading.
    pub const TITLE: f32 = 20.0;
    /// Prose and labels.
    pub const BODY: f32 = 16.0;
    /// Buttons, tabs, table cells.
    pub const UI: f32 = 13.0;
    /// Units, timestamps, footnotes.
    pub const CAPTION: f32 = 11.0;
}

/// Fixed metrics that are not spacing but are still part of the composed page.
///
/// Row heights and hairlines come from DESIGN.md §3. The canvas sizes are here
/// for the same reason the spacing scale is: a screen that writes `220.0` by
/// hand has invented a second grid, and the next screen copies it.
pub mod metric {
    /// One table row.
    pub const ROW: f32 = 32.0;
    /// A compact row: one step tighter than [`ROW`], for a subordinate rail
    /// block (DESIGN.md §5, ログ).
    ///
    /// Its own token rather than reusing [`TABLE_HEADER`]: a body row that borrows
    /// the header's height makes two meanings share one number, and the next
    /// change to the header silently moves every body row with it.
    pub const ROW_COMPACT: f32 = 28.0;
    /// A table's header row, one step tighter than its body.
    pub const TABLE_HEADER: f32 = 28.0;
    /// A hairline rule, and the width of any ordinary divider.
    pub const HAIRLINE: f32 = 1.0;
    /// Stroke width of the focus ring (DESIGN.md §6).
    pub const FOCUS_RING: f32 = 2.0;
    /// Width of a track line, a chart series, or a plotted path.
    pub const TRACK_LINE: f32 = 2.0;
    /// Numbers carried over from the reference implementation's polyline
    /// (07-eztour-spec §5 「既定の線幅 5 px」) plus the casing the map needs.
    ///
    /// The reference draws the line at 70% opacity because its map was a dark
    /// schematic. This map's default surface is OSM tile (land #F2EFE9, water
    /// #AAD3DF): `data` alone measures 1.7:1 on land, so the line is painted at
    /// full opacity over a `ground` casing — 11:1 or better on either surface
    /// (design review 2026-11-23, M1).
    pub mod track {
        /// Stroke width of the map's track polyline.
        pub const LINE_WIDTH: f32 = 5.0;
        /// Stroke width of the casing drawn under the polyline.
        pub const CASING_WIDTH: f32 = 7.0;
        /// Opacity of one plotted record's fill.
        ///
        /// The line is painted opaque over its casing (see below); the records
        /// keep the reference implementation's 70% so a dense track does not turn
        /// into a solid ribbon.
        pub const LINE_OPACITY: f32 = 0.7;

        /// The map's track polyline stroke, in the product's data colour.
        ///
        /// `PathStroke` rather than `Stroke` because the map line is a path.
        pub fn line_stroke() -> egui::epaint::PathStroke {
            egui::epaint::PathStroke::new(LINE_WIDTH, super::super::DATA)
        }

        /// The casing under [`line_stroke`]: the same geometry in `ground`.
        ///
        /// Drawn first so the polyline keeps a dark edge on the light tile; on
        /// the offline grid it is invisible, which is harmless.
        pub fn casing_stroke() -> egui::epaint::PathStroke {
            egui::epaint::PathStroke::new(CASING_WIDTH, super::super::GROUND)
        }

        /// The fill of one plotted record, in ezTour's ten-colour cycle.
        ///
        /// The hue carries the sequence on the offline grid. On a tile the hue is
        /// too pale to read, so the caller draws a `ground` disc just under it —
        /// the cycle stays true and the marker stays visible.
        pub fn record_fill(index: usize) -> egui::Color32 {
            let palette = super::super::TRACK_PALETTE;
            palette
                .get(index % palette.len())
                .copied()
                .unwrap_or(super::super::DATA)
                .gamma_multiply(LINE_OPACITY)
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
    ///
    /// The map is the page (DESIGN.md §5). This row is one line — the screen's
    /// title and the sample marker — and every other control is drawn *over* the
    /// canvas as an overlay plate, so none of them takes height from the map.
    pub const TOOL_ROW: f32 = 40.0;
    /// Width of the Log screen's right-hand drawer.
    ///
    /// Five tab panels' worth of controls. On a window narrower than
    /// [`NARROW`] the drawer lies over the map instead of beside it: 360pt
    /// beside an 800pt content area would leave the map a strip rather than the
    /// main surface.
    pub const DRAWER: f32 = 360.0;
    /// Height of the speed chart (a subordinate series, one step under the map).
    pub const CHART: f32 = 80.0;
    /// Minimum width of a dialog's primary action, so two buttons line up.
    pub const ACTION_WIDTH: f32 = 128.0;
    /// Below this content width the Log screen stacks instead of splitting
    /// (DESIGN.md §9 leaves the breakpoint open; this is the one it names).
    pub const NARROW: f32 = 1_100.0;
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
        // The UI's copy is Japanese (DESIGN.md §3, Noto Sans JP). Asking for
        // `ja` makes the OS fallback pick a Japanese face for Han characters
        // instead of whichever Chinese face fontconfig lists first.
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
/// primary, with [`system_fonts`] still installed underneath as the fallback
/// for every glyph it lacks (Japanese, symbols). See `DESIGN.md` §10 for what
/// exactly is approximated.
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

/// Type scale (DESIGN.md §3): 11 / 13 / 16 / 20 / 24.
///
/// egui has five named slots, so `display` (24) is [`TextStyle::Heading`] and
/// screen titles ask for 20 directly via `RichText::size`. Monospace shares the
/// 13pt size with buttons on purpose: it is a different family carrying a
/// different job, not a step in the scale.
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

fn apply(style: &mut egui::Style) {
    style.text_styles = text_styles().into_iter().collect();

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.panel_fill = GROUND;
    visuals.window_fill = GROUND;
    visuals.extreme_bg_color = RAIL;
    // An input field is a raised surface, not a hole: `PANEL` sits one step
    // above `GROUND`, so a path field reads as an editable box instead of
    // vanishing into the page (the design direction's "fields must be
    // distinguishable from headings"). egui paints the TextEdit frame from this
    // colour, so every screen's inputs take the same step.
    visuals.text_edit_bg_color = Some(PANEL);
    visuals.faint_bg_color = PANEL;
    visuals.override_text_color = Some(INK);
    visuals.window_stroke = egui::Stroke::new(1.0, RULE);
    visuals.selection.bg_fill = DATA.gamma_multiply(0.35);
    visuals.selection.stroke = egui::Stroke::new(1.0, DATA);

    // §3: the only elevation is a `rule` hairline. egui's dark defaults carry a
    // drop shadow on windows and popups, which would put one on the port ComboBox
    // and on every hover tooltip — a second elevation language borrowed from the
    // toolkit rather than designed.
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;

    // Focus is a 2px ink outline. Not amber: amber means danger, not "here".
    visuals.widgets.active.bg_stroke = egui::Stroke::new(metric::FOCUS_RING, INK);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, MUTE);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = RADIUS;
    }

    style.spacing.item_spacing = egui::vec2(space::S, space::S);
    style.spacing.button_padding = egui::vec2(space::M, space::XS);
    style.spacing.menu_margin = egui::Margin::same(space::XS as i8);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_text_colour_clears_wcag_aa_on_both_surfaces() {
        for (name, colour) in [
            ("INK", INK),
            ("MUTE", MUTE),
            ("DANGER", DANGER),
            ("DATA", DATA),
            ("OK", OK),
            ("ERR", ERR),
        ] {
            for (surface_name, surface) in [("GROUND", GROUND), ("PANEL", PANEL)] {
                let ratio = contrast(colour, surface);
                assert!(
                    ratio >= 4.5,
                    "{name} on {surface_name} is {ratio:.2}, below 4.5:1"
                );
            }
        }
    }

    #[test]
    fn rules_are_hairlines_not_text_so_they_are_exempt() {
        // Documented on purpose: RULE clears 1.3:1 and that is correct for a
        // divider. The test exists so nobody "fixes" it into a text colour.
        assert!(contrast(RULE, GROUND) < 2.0);
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
            let ratio = b / a;
            assert!(
                ratio >= 1.15,
                "steps {a} and {b} are only {ratio:.2} apart; they cannot carry different roles"
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
    fn the_palette_is_the_ten_hex_values_the_design_names() {
        // The tokens are the design (§3). A re-tuned colour here is a design
        // change, not a detail, and the contrast test alone would accept a whole
        // new palette as long as it cleared 4.5:1.
        for (name, colour, expected) in [
            ("GROUND", GROUND, Color32::from_rgb(0x14, 0x18, 0x1A)),
            ("PANEL", PANEL, Color32::from_rgb(0x1B, 0x20, 0x23)),
            ("RAIL", RAIL, Color32::from_rgb(0x11, 0x14, 0x16)),
            ("RULE", RULE, Color32::from_rgb(0x2C, 0x34, 0x37)),
            ("INK", INK, Color32::from_rgb(0xE8, 0xE6, 0xDF)),
            ("MUTE", MUTE, Color32::from_rgb(0x9A, 0xA0, 0xA0)),
            ("DANGER", DANGER, Color32::from_rgb(0xE3, 0xA0, 0x08)),
            ("DATA", DATA, Color32::from_rgb(0x5F, 0xB3, 0xC4)),
            ("OK", OK, Color32::from_rgb(0x6F, 0xBF, 0x73)),
            ("ERR", ERR, Color32::from_rgb(0xE2, 0x70, 0x5E)),
        ] {
            assert_eq!(
                colour, expected,
                "{name} is not the token DESIGN.md §3 names"
            );
        }
        // The ten-colour cycle is still the record fill, but on an OSM tile a
        // pale hue measures under 2:1 on land, so `map_canvas` puts a `ground`
        // casing under every record as well (design review 2026-11-23, M1).
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
            "the ten-colour record cycle changed"
        );
    }

    #[test]
    fn the_type_scale_is_the_five_sizes_the_design_names() {
        // 11 / 13 / 16 / 20 / 24. The ratio test above passes for many scales;
        // this one names the values the tokens promise.
        assert_eq!(
            [
                size::CAPTION,
                size::UI,
                size::BODY,
                size::TITLE,
                size::DISPLAY
            ],
            [11.0, 13.0, 16.0, 20.0, 24.0]
        );
        let sizes: Vec<f32> = text_styles().iter().map(|(_, font)| font.size).collect();
        assert_eq!(sizes, [24.0, 16.0, 13.0, 11.0, 13.0]);
    }

    #[test]
    fn the_spacing_scale_is_the_seven_steps_the_design_names() {
        assert_eq!(
            [
                space::XS,
                space::S,
                space::M,
                space::L,
                space::XL,
                space::XXL,
                space::XXXL
            ],
            [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0]
        );
    }

    #[test]
    fn the_only_corner_radius_is_two_points() {
        // §3: a manual's tabs are square-cut. The gate forces every call site to
        // reference the token; this pins the value the token holds.
        assert_eq!(RADIUS, CornerRadius::same(2));
    }

    #[test]
    fn the_page_has_no_shadow_elevation() {
        // §3: elevation is one `rule` hairline. egui's dark default attaches a
        // drop shadow to windows and popups, which is where the port ComboBox and
        // the tooltips would pick one up; `apply` must zero both.
        let mut style = egui::Style::default();
        apply(&mut style);
        assert_eq!(style.visuals.window_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(style.visuals.popup_shadow, egui::epaint::Shadow::NONE);
    }

    #[test]
    fn input_fields_sit_on_the_raised_surface() {
        // A field has to read as editable rather than as a label. `PANEL` is the
        // product's one raised surface; the theme used to leave egui's default,
        // which made a TextEdit a hole slightly darker than the page. Pinning
        // the token keeps the input/heading distinction from drifting back.
        let mut style = egui::Style::default();
        apply(&mut style);
        assert_eq!(style.visuals.text_edit_bg_color, Some(PANEL));
    }

    #[test]
    fn the_focus_ring_is_two_points_of_ink() {
        // §6: every control gets a 2px ink focus ring. The rail paints its own
        // with `metric::FOCUS_RING`; ordinary widgets read the style's active
        // stroke, so both have to name the same width or focus looks different
        // depending on which control you tab to.
        let mut style = egui::Style::default();
        apply(&mut style);
        let ring = style.visuals.widgets.active.bg_stroke;
        assert_eq!(ring.width, metric::FOCUS_RING);
        assert_eq!(ring.color, INK);
    }

    #[test]
    fn the_row_heights_are_the_ones_the_design_names() {
        assert_eq!(metric::ROW, 32.0, "a table row is 32 (DESIGN.md §3)");
        assert_eq!(metric::TABLE_HEADER, 28.0, "a header row is 28");
        assert_eq!(metric::ROW_COMPACT, 28.0, "a rail-block row is 28");
    }
}
