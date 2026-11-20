//! Design tokens, in the one place the GUI reads colour and type from.
//!
//! Values and their contrast ratios are in `DESIGN.md` §3. Two rules are
//! enforced by tests rather than by review:
//!
//! 1. [`DANGER`] is reserved: it may appear only on the destructive action in
//!    the firmware screen. `tests/design_invariants.rs` greps for it.
//! 2. Every text colour clears 4.5:1 against both [`GROUND`] and [`PANEL`].

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

/// Type scale (DESIGN.md §3): 11 / 13 / 16 / 20 / 24.
///
/// egui has five named slots, so `display` (24) is [`TextStyle::Heading`] and
/// screen titles ask for 20 directly via `RichText::size`. Monospace shares the
/// 13pt size with buttons on purpose: it is a different family carrying a
/// different job, not a step in the scale.
pub fn text_styles() -> Vec<(TextStyle, FontId)> {
    vec![
        (TextStyle::Heading, FontId::proportional(24.0)),
        (TextStyle::Body, FontId::proportional(16.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
}

/// Install the design tokens into an egui context.
///
/// egui 0.37 keeps one style per theme, so both are set: the product ships dark
/// only, and a light OS theme must not produce a light widget set.
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(apply);
}

fn apply(style: &mut egui::Style) {
    style.text_styles = text_styles().into_iter().collect();

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.panel_fill = GROUND;
    visuals.window_fill = GROUND;
    visuals.extreme_bg_color = RAIL;
    visuals.faint_bg_color = PANEL;
    visuals.override_text_color = Some(INK);
    visuals.window_stroke = egui::Stroke::new(1.0, RULE);
    visuals.selection.bg_fill = DATA.gamma_multiply(0.35);
    visuals.selection.stroke = egui::Stroke::new(1.0, DATA);

    // Focus is a 2px ink outline. Not amber: amber means danger, not "here".
    visuals.widgets.active.bg_stroke = egui::Stroke::new(2.0, INK);
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
}
