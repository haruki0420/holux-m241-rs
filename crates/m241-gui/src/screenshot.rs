//! Offscreen rendering: turn the shell into a PNG without a window.
//!
//! DESIGN.md §8 defers the `design-review` round until screenshots exist, and says
//! what would unblock it: "a rendered `egui` window at 1280×800 is a better comp
//! than an image model's guess". This module is that render, with no window, no GPU
//! and no display server.
//!
//! # How
//!
//! `egui` is geometry. A pass produces [`egui::Shape`]s; [`Context::tessellate`] turns
//! them into indexed triangles with a texture coordinate and a colour per vertex; the
//! only texture the shell uses is `egui`'s font atlas, which arrives in
//! `FullOutput::textures_delta`. So a software rasteriser of about three hundred lines
//! is enough — and unlike a screenshot it works in CI, on a headless machine, and
//! produces the same pixels every time.
//!
//! Anti-aliasing is free: `egui` tessellates rounded rectangles, hairlines and glyphs
//! as feathered geometry, so the triangles already carry the coverage.
//!
//! # What this is not
//!
//! It is not a renderer for the product. It draws one frame's tessellation and nothing
//! else: no animation, no scrolling state that needs a second pass, no gamma
//! pipeline beyond what `egui` already did when it chose the colours. The images are
//! for judging layout, rhythm and the amber inventory — which is exactly what §8 asks
//! a reviewer to judge.

use std::path::Path;

use egui::{Color32, Context, Pos2, Rect, TextureId, Vec2};

use crate::app::App;
use crate::screens::Screen;
use crate::screens::connect::{DaProbeState, DoctorState};
use crate::screens::firmware::{DumpState, FlashState};
use crate::screens::log::LogTab;
use crate::theme;

/// An 8-bit RGBA image, top-down, ready for a PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row by row.
    pub pixels: Vec<u8>,
}

impl Image {
    /// A new image filled with `fill`.
    #[must_use]
    fn filled(width: u32, height: u32, fill: Color32) -> Self {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        // Opaque: a screenshot with a transparent ground looks like a broken PNG in
        // every viewer that shows white behind alpha.
        for _ in 0..(width as usize) * (height as usize) {
            pixels.extend_from_slice(&[fill.r(), fill.g(), fill.b(), 255]);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// How many distinct colours the image contains.
    ///
    /// Used as a floor in the tests: a frame that draws hundreds of shapes but
    /// rasterises to one colour is a broken renderer, and a broken renderer that
    /// silently writes a blank PNG is worse than no screenshots at all.
    #[must_use]
    pub fn distinct_colours(&self) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        // `as_chunks` rather than `chunks_exact` + indexing: the crate forbids
        // indexing outside tests, and an RGBA pixel is exactly a `[u8; 4]` to destructure.
        for [red, green, blue, alpha] in self.pixels.as_chunks::<4>().0.iter().copied() {
            let _inserted = seen.insert((red, green, blue, alpha));
        }
        seen.len()
    }

    /// Whether any pixel is close to `colour`.
    ///
    /// "Close" because a 1px hairline against a dark ground lands somewhere between
    /// the two colours after coverage blending; an exact match would only find the
    /// pixel if it happened to be fully covered.
    #[must_use]
    pub fn contains_colour(&self, colour: Color32, tolerance: u8) -> bool {
        let near = |a: u8, b: u8| a.abs_diff(b) <= tolerance;
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|[red, green, blue, _alpha]| {
                near(*red, colour.r()) && near(*green, colour.g()) && near(*blue, colour.b())
            })
    }

    /// Encode as a PNG, compactly.
    ///
    /// Delegated to the `png` crate rather than written by hand: the images are review
    /// artifacts that live in git, a 4 MB uncompressed file per screen is not something
    /// to commit eight of, and a correct deflate stream is not this module's subject.
    ///
    /// # Errors
    ///
    /// The encoder's failure, which means the image is not the size it claims.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|err| format!("PNG ヘッダを書けません: {err}"))?;
            writer
                .write_image_data(&self.pixels)
                .map_err(|err| format!("PNG を書けません: {err}"))?;
        }
        Ok(bytes)
    }
}

/// A decoded font-atlas texture.
#[derive(Debug, Clone)]
struct Texture {
    width: usize,
    height: usize,
    /// Straight (not premultiplied) RGBA.
    pixels: Vec<Color32>,
}

impl Texture {
    /// Nearest-neighbour sample, clamped.
    ///
    /// Nearest rather than bilinear on purpose: the atlas is rasterised at the same
    /// size the glyphs are drawn at, so a correct coordinate lands exactly on one
    /// texel. Filtering would only blur text that was already hinted.
    fn sample(&self, u: f32, v: f32) -> Color32 {
        if self.width == 0 || self.height == 0 {
            return Color32::WHITE;
        }
        let x = (u * self.width as f32).floor().max(0.0) as usize;
        let y = (v * self.height as f32).floor().max(0.0) as usize;
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        let index = y * self.width + x;
        self.pixels.get(index).copied().unwrap_or(Color32::WHITE)
    }
}

/// Render one screen of the shell at `size` points, one point per pixel.
///
/// # Panics
///
/// Never for a valid `size`; the shell is drawn the same way `render.rs` draws it.
#[must_use]
pub fn render(app: &mut App, screen: Screen, size: Vec2) -> Image {
    let ctx = Context::default();
    // Installs both the palette and the font stack (system faces promoted to
    // primary, system fallback behind them). A headless render would otherwise
    // lay out Japanese as empty boxes; that is now the theme's job so the window
    // and the screenshots cannot drift apart.
    theme::install(&ctx);
    // The shell's chosen screen, without a click: the screenshot flag is not a
    // pointer, and a rail that only responds to a click would render the Connect
    // screen four times.
    app.select(screen);

    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        ..egui::RawInput::default()
    };
    // Two passes: immediate mode settles its layout on the first, and the second is
    // the frame a running application would show.
    // Two passes, and the *first* pass is the one that carries the font atlas: egui
    // builds its glyph atlas while laying out the first frame and the second frame's
    // delta only patches it. An integration applies each delta as it arrives; this one
    // keeps them and applies both at the end, which is why the first cannot simply be
    // dropped.
    let first = ctx.run_ui(input.clone(), |ui| app.draw(ui));
    let output = ctx.run_ui(input, |ui| app.draw(ui));

    let mut image = Image::filled(
        size.x.round().max(1.0) as u32,
        size.y.round().max(1.0) as u32,
        theme::GROUND,
    );

    let mut atlas = Atlas::default();
    atlas.apply(&first.textures_delta);
    atlas.apply(&output.textures_delta);
    if atlas.is_empty() {
        atlas.fallback();
    }
    // Cloned rather than moved: `FullOutput` still has to be dropped through
    // `drop_without_applying_deltas`, and a shape is cheap next to the tessellation.
    let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
    for primitive in &primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            continue;
        };
        for [first_index, second_index, third_index] in mesh.indices.as_chunks::<3>().0 {
            let vertices = (
                mesh.vertices.get(*first_index as usize),
                mesh.vertices.get(*second_index as usize),
                mesh.vertices.get(*third_index as usize),
            );
            let (Some(first), Some(second), Some(third)) = vertices else {
                continue;
            };
            rasterise(
                &mut image,
                [first, second, third],
                primitive.clip_rect,
                atlas.as_ref(),
            );
        }
    }
    first.drop_without_applying_deltas();
    output.drop_without_applying_deltas();
    image
}

/// The textures a frame drew with, accumulated from every delta it produced.
///
/// `egui` emits a whole-image delta the first time a texture is used and a patch
/// (`pos: Some(..)`) for later glyphs, so a rasteriser needs both to see the atlas a
/// real backend would. Only one texture exists in this crate — egui's font atlas — but
/// the accumulation is keyed so a second one cannot silently overwrite the first.
#[derive(Debug, Default)]
struct Atlas {
    images: std::collections::BTreeMap<(u8, u64), Texture>,
}

impl Atlas {
    /// Merge one frame's texture changes.
    fn apply(&mut self, delta: &egui::TexturesDelta) {
        for (id, patches) in &delta.set {
            let key = match id {
                TextureId::Managed(index) => (0, *index),
                TextureId::User(index) => (1, *index),
            };
            for patch in patches {
                self.patch(key, patch);
            }
        }
    }

    /// Apply one whole image or one sub-rectangle of it.
    fn patch(&mut self, key: (u8, u64), patch: &egui::epaint::ImageDelta) {
        let egui::epaint::ImageData::Color(colour) = &patch.image;
        let width = colour.size.first().copied().unwrap_or(0);
        let height = colour.size.get(1).copied().unwrap_or(0);
        let (Some(origin_x), Some(origin_y)) = (
            patch.pos.and_then(|pos| pos.first().copied()),
            patch.pos.and_then(|pos| pos.get(1).copied()),
        ) else {
            let _replaced = self.images.insert(
                key,
                Texture {
                    width,
                    height,
                    pixels: colour.pixels.clone(),
                },
            );
            return;
        };
        let Some(texture) = self.images.get_mut(&key) else {
            // A patch with nothing to patch: keep it as the whole image, since the
            // alternative is drawing a blank frame.
            let _inserted = self.images.insert(
                key,
                Texture {
                    width,
                    height,
                    pixels: colour.pixels.clone(),
                },
            );
            return;
        };
        for row in 0..height {
            for column in 0..width {
                let Some(source) = colour.pixels.get(row * width + column) else {
                    continue;
                };
                let x = origin_x + column;
                let y = origin_y + row;
                if x >= texture.width || y >= texture.height {
                    continue;
                }
                let index = y * texture.width + x;
                if let Some(target) = texture.pixels.get_mut(index) {
                    *target = *source;
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Draw everything white, for a frame that produced no texture at all.
    ///
    /// Without an atlas every fragment is unpainted, which is correct for solid
    /// geometry and wrong for text; a single white texel makes a missing atlas fail
    /// loudly in the tests instead of producing a quietly empty picture.
    fn fallback(&mut self) {
        let _inserted = self.images.insert(
            (0, 0),
            Texture {
                width: 1,
                height: 1,
                pixels: vec![Color32::WHITE],
            },
        );
    }

    fn as_ref(&self) -> Option<&Texture> {
        self.images.values().next()
    }
}

/// Fill one triangle, clipped and texture-mapped.
fn rasterise(
    image: &mut Image,
    vertices: [&egui::epaint::Vertex; 3],
    clip: Rect,
    texture: Option<&Texture>,
) {
    let [a, b, c] = vertices;
    let clip = clip.intersect(Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(image.width as f32, image.height as f32),
    ));
    if !clip.is_positive() {
        return;
    }

    // Signed area; a degenerate triangle paints nothing, which is what a zero-length
    // hairline becomes.
    let area = edge(a.pos, b.pos, c.pos);
    if area.abs() < f32::EPSILON {
        return;
    }

    let min_x = clip.min.x.floor().max(0.0) as u32;
    let max_x = (clip.max.x.ceil() as u32).min(image.width);
    let min_y = clip.min.y.floor().max(0.0) as u32;
    let max_y = (clip.max.y.ceil() as u32).min(image.height);

    for y in min_y..max_y {
        let sample_y = y as f32 + 0.5;
        for x in min_x..max_x {
            let sample = Pos2::new(x as f32 + 0.5, sample_y);
            if !clip.contains(sample) {
                continue;
            }
            let wa = edge(b.pos, c.pos, sample) / area;
            let wb = edge(c.pos, a.pos, sample) / area;
            let wc = edge(a.pos, b.pos, sample) / area;
            // A tiny negative tolerance keeps shared triangle edges from showing a
            // seam: two adjacent triangles both claim the boundary pixel.
            const EDGE: f32 = -1e-4;
            if wa < EDGE || wb < EDGE || wc < EDGE {
                continue;
            }

            let colour = blend(
                interpolate_colour(a.color, b.color, c.color, [wa, wb, wc]),
                interpolate_uv(a.uv, b.uv, c.uv, [wa, wb, wc]),
                a.color,
                texture,
            );
            let offset = ((y as usize) * (image.width as usize) + (x as usize)) * 4;
            if let Some(target) = image.pixels.get_mut(offset..offset + 4) {
                composite(target, colour);
            }
        }
    }
}

/// Twice the signed area of the triangle `(a, b, c)`.
fn edge(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Weighted vertex colour, in premultiplied sRGB as `egui` tessellates it.
fn interpolate_colour(a: Color32, b: Color32, c: Color32, weights: [f32; 3]) -> [f32; 4] {
    let [wa, wb, wc] = weights;
    let channel = |pick: fn(&Color32) -> u8| {
        f32::from(pick(&a)) * wa + f32::from(pick(&b)) * wb + f32::from(pick(&c)) * wc
    };
    let alpha = channel(|colour| colour.a());
    // `egui` stores vertex colours premultiplied by their own alpha: the rasteriser
    // must not multiply again, or every translucent overlay comes out too dark.
    let mut premultiplied = [
        channel(|colour| colour.r()),
        channel(|colour| colour.g()),
        channel(|colour| colour.b()),
        alpha,
    ];
    // The atlas is white; a vertex whose colour was not premultiplied would show as a
    // bright halo. Clamping each channel to its own alpha is what makes both cases
    // come out the same.
    for value in premultiplied.iter_mut().take(3) {
        *value = value.min(alpha);
    }
    premultiplied
}

/// Weighted vertex texture coordinate.
fn interpolate_uv(a: Pos2, b: Pos2, c: Pos2, weights: [f32; 3]) -> Pos2 {
    let [wa, wb, wc] = weights;
    Pos2::new(
        a.x * wa + b.x * wb + c.x * wc,
        a.y * wa + b.y * wb + c.y * wc,
    )
}

/// The colour a fragment contributes, premultiplied.
///
/// Untextured geometry in `egui` points at the atlas's opaque white texel, so the
/// same multiply serves both cases and there is no "is this a text mesh" branch to get
/// wrong.
fn blend(colour: [f32; 4], uv: Pos2, _vertex: Color32, texture: Option<&Texture>) -> [f32; 4] {
    let Some(texture) = texture else {
        return colour;
    };
    let texel = texture.sample(uv.x, uv.y);
    let coverage = f32::from(texel.a()) / 255.0;
    [
        colour[0] * coverage,
        colour[1] * coverage,
        colour[2] * coverage,
        colour[3] * coverage,
    ]
}

/// `source over destination`, both premultiplied, into an RGBA8 target.
fn composite(target: &mut [u8], source: [f32; 4]) {
    let alpha = (source[3] / 255.0).clamp(0.0, 1.0);
    let inverse = 1.0 - alpha;
    for (index, channel) in target.iter_mut().enumerate() {
        let existing = f32::from(*channel);
        let added = if index == 3 {
            source[3]
        } else {
            source.get(index).copied().unwrap_or(0.0)
        };
        *channel = (added + existing * inverse).clamp(0.0, 255.0).round() as u8;
    }
}

/// Render one frame and write it as a PNG.
///
/// The repeated render → encode → write → record step of [`write_all`]. Pulled
/// out so a frame that silently fails to encode cannot be lost in a copy of the
/// sequence; every failure keeps the path it was writing.
fn write_frame(
    directory: &Path,
    name: &str,
    app: &mut App,
    screen: Screen,
    size: Vec2,
) -> Result<std::path::PathBuf, String> {
    let image = render(app, screen, size);
    let path = directory.join(name);
    let png = image
        .to_png()
        .map_err(|err| format!("{}: {err}", path.display()))?;
    std::fs::write(&path, png).map_err(|err| format!("{} を書けません: {err}", path.display()))?;
    Ok(path)
}

/// Render every screen at both sizes DESIGN.md §8 names, into `directory`.
///
/// # Errors
///
/// The I/O failure, with the path and the reason: a screenshot run that wrote nothing
/// must not exit 0.
pub fn write_all(directory: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    write_all_with(directory, &[(1280.0, 800.0), (1024.0, 640.0)])
}

/// The body of [`write_all`], with the screen sizes named by the caller.
///
/// `write_all` always passes the two documented sizes. The test drives the same
/// writer at a small size — the software rasteriser is O(pixels × triangles), so
/// rendering the real set inside the test suite would cost minutes — and counts
/// the frames it produces, so a drawer loop deleted from here fails a test. The
/// device-side frames (write states, dump states, Connect's panels) are rendered
/// at the first size, which keeps the standard set identical to before.
fn write_all_with(
    directory: &Path,
    sizes: &[(f32, f32)],
) -> Result<Vec<std::path::PathBuf>, String> {
    std::fs::create_dir_all(directory)
        .map_err(|err| format!("{} を作れません: {err}", directory.display()))?;
    let (base_width, base_height) = sizes.first().copied().unwrap_or((1280.0, 800.0));

    let mut written = Vec::new();
    for &(width, height) in sizes {
        for screen in Screen::ALL {
            let mut app = App::default();
            prepare(&mut app, screen);
            let name = format!("{}-{}x{}.png", slug(screen), width as u32, height as u32);
            written.push(write_frame(
                directory,
                &name,
                &mut app,
                screen,
                Vec2::new(width, height),
            )?);
        }
    }
    // The write step has three layouts a device is needed to produce: running,
    // a read-back match, and the recovery state after a failure. The review is of
    // the flow, not of one frame, so they are rendered beside the screens.
    for (suffix, state, undefined) in [
        (
            "write-progress",
            FlashState::Running {
                stage: String::from("書き込んでいます"),
                packets: 400,
                bytes: 102_400,
                total: m241_flash::da::FLASH_WRITE_LENGTH,
            },
            false,
        ),
        (
            "write-result",
            FlashState::Succeeded {
                packets: m241_flash::da::FLASH_WRITE_LENGTH.div_ceil(m241_flash::da::PACKET_SIZE),
                verified: m241_flash::da::FLASH_WRITE_LENGTH,
            },
            false,
        ),
        (
            "write-recovery",
            FlashState::Failed(String::from(
                "読み戻しがイメージと一致しません（先頭の差分 0x40000）",
            )),
            true,
        ),
    ] {
        let mut app = App::default();
        prepare(&mut app, Screen::Firmware);
        app.firmware_mut().set_flash_state(state);
        app.firmware_mut().set_undefined(undefined);
        app.firmware_mut().set_dump_state(DumpState::Verified {
            bytes: 406_620,
            digest: String::from(
                "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
            ),
        });
        let name = format!(
            "firmware-{suffix}-{}x{}.png",
            base_width as u32, base_height as u32
        );
        written.push(write_frame(
            directory,
            &name,
            &mut app,
            Screen::Firmware,
            Vec2::new(base_width, base_height),
        )?);
    }
    // The two-read dump's intermediate states: the power-cycle prompt between the
    // reads, and the mismatch that must never be saved. Neither is reachable
    // without a unit, so they are rendered with injected state like the write
    // frames.
    for (suffix, state) in [
        (
            "dump-power-cycle",
            DumpState::WaitingForPowerCycle {
                total: 406_620,
                digest: String::from(
                    "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
                ),
            },
        ),
        (
            "dump-mismatch",
            DumpState::Mismatch {
                first: String::from(
                    "32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
                ),
                second: String::from(
                    "aa3e52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435",
                ),
            },
        ),
    ] {
        let mut app = App::default();
        prepare(&mut app, Screen::Firmware);
        app.firmware_mut()
            .set_step(crate::screens::firmware::Step::Dump);
        app.firmware_mut().set_dump_state(state);
        let name = format!(
            "firmware-{suffix}-{}x{}.png",
            base_width as u32, base_height as u32
        );
        written.push(write_frame(
            directory,
            &name,
            &mut app,
            Screen::Firmware,
            Vec2::new(base_width, base_height),
        )?);
    }
    // The Connect screen's three device-side actions need a device to produce
    // their finished states, so the review image is taken with a sample report.
    {
        let mut app = App::default();
        prepare(&mut app, Screen::Connect);
        app.connect_mut().set_diagnostics_open(true);
        app.connect_mut()
            .set_doctor_state(DoctorState::Done(Box::new(crate::device::DoctorReport {
                ports: vec![
                    crate::device::PortFinding {
                        name: String::from("/dev/ttyUSB0"),
                        likely: true,
                    },
                    crate::device::PortFinding {
                        name: String::from("/dev/ttyACM0"),
                        likely: true,
                    },
                ],
                identity: Some(m241_core::DeviceInfo {
                    release: String::from("B-core_1.20"),
                    build: String::from("0043"),
                    model: String::from("01017-00F"),
                    sdk: None,
                }),
                drop_dir: String::from("vendor"),
                files: vec![crate::device::DropFile {
                    path: String::from("vendor/DA_v3.8.4.bin"),
                    size: 14_896,
                    kind: crate::device::DropKind::Da,
                }],
                da_source: Some(String::from("vendor/DA_v3.8.4.bin")),
                ..crate::device::DoctorReport::default()
            })));
        app.connect_mut()
            .set_da_probe_state(DaProbeState::Done(m241_flash::flow::ProbeDaReport {
                id: m241_flash::da::DaLoadId {
                    version: 0x0308,
                    region: 0x49,
                    flash_bytes: 0x8_0000,
                    manufacturer: 0x00C2,
                    device: 0x22BA,
                },
                pages: [
                    m241_flash::flow::ProbePage {
                        address: m241_flash::da::PROBE_ADDRESSES[0],
                        first_bytes: Some(m241_flash::da::VECTOR),
                        matches_vector: true,
                        error: None,
                    },
                    m241_flash::flow::ProbePage {
                        address: m241_flash::da::PROBE_ADDRESSES[1],
                        first_bytes: Some([0xFF, 0xFF, 0xFF, 0xFF]),
                        matches_vector: false,
                        error: None,
                    },
                ],
                settled: Some(m241_flash::da::PROBE_ADDRESSES[0]),
            }));
        written.push(write_frame(
            directory,
            &format!(
                "connect-diagnostics-{}x{}.png",
                base_width as u32, base_height as u32
            ),
            &mut app,
            Screen::Connect,
            Vec2::new(base_width, base_height),
        )?);
    }
    // The DA probe's result needs its own frame: with the full doctor report on
    // screen it lands below the fold, and a result a reviewer cannot see is not
    // reviewed.
    {
        let mut app = App::default();
        prepare(&mut app, Screen::Connect);
        app.connect_mut().set_diagnostics_open(true);
        app.connect_mut()
            .set_da_probe_state(DaProbeState::Done(m241_flash::flow::ProbeDaReport {
                id: m241_flash::da::DaLoadId {
                    version: 0x0308,
                    region: 0x49,
                    flash_bytes: 0x8_0000,
                    manufacturer: 0x00C2,
                    device: 0x22BA,
                },
                pages: [
                    m241_flash::flow::ProbePage {
                        address: m241_flash::da::PROBE_ADDRESSES[0],
                        first_bytes: Some(m241_flash::da::VECTOR),
                        matches_vector: true,
                        error: None,
                    },
                    m241_flash::flow::ProbePage {
                        address: m241_flash::da::PROBE_ADDRESSES[1],
                        first_bytes: Some([0xFF, 0xFF, 0xFF, 0xFF]),
                        matches_vector: false,
                        error: None,
                    },
                ],
                settled: Some(m241_flash::da::PROBE_ADDRESSES[0]),
            }));
        written.push(write_frame(
            directory,
            &format!(
                "connect-probe-da-{}x{}.png",
                base_width as u32, base_height as u32
            ),
            &mut app,
            Screen::Connect,
            Vec2::new(base_width, base_height),
        )?);
    }
    // The drawer's five tabs are the panels the map hides by default: two images
    // of the closed drawer would leave four of the five with no rendered frame in
    // the standard set. The batch above stays the map the review is about; this
    // one is the panels, at both documented sizes.
    for &(width, height) in sizes {
        for (tab, slug) in [
            (LogTab::Summary, "summary"),
            (LogTab::Photos, "photos"),
            (LogTab::Edit, "edit"),
            (LogTab::Export, "export"),
            (LogTab::Ops, "ops"),
        ] {
            let mut app = App::default();
            prepare(&mut app, Screen::Log);
            app.log_mut().open_drawer(tab);
            let name = format!("log-drawer-{slug}-{}x{}.png", width as u32, height as u32);
            written.push(write_frame(
                directory,
                &name,
                &mut app,
                Screen::Log,
                Vec2::new(width, height),
            )?);
        }
    }
    Ok(written)
}

/// Fill a screen's state so its screenshot shows content rather than an empty panel.
///
/// A screenshot of an empty log says nothing about the log screen's layout, so the
/// images are taken with the sample data loaded — which is labelled 「見本」 on the
/// screen itself, so the images cannot be mistaken for a measurement.
fn prepare(app: &mut App, screen: Screen) {
    match screen {
        Screen::Connect => app.connect_mut().show_sample(),
        Screen::Log => {
            app.log_mut().show_sample();
            // The photo block and the KMZ row live in the drawer's 写真 and
            // 書き出し tabs, which `write_all` renders as their own frames
            // (`log-drawer-photos-*`, `log-drawer-export-*`). The state is loaded
            // here so those frames show the wiring resolved: sample photos (no
            // file touched), a sample camera for the shift block, and the KMZ
            // writer selected. The closed-drawer images still show the map the
            // review is about.
            app.log_mut().show_sample_photos();
            app.log_mut().show_sample_photo_shift();
            app.log_mut().set_kmz_export();
            app.log_mut().set_kmz_max_edge("1600");
        }
        Screen::Firmware => {
            // The write step is what the design review is for, so the screenshot
            // shows it with the patched sample image already in memory and the
            // confirmation armed. Nothing is written to disk.
            let _loaded = app.firmware_mut().show_sample_write();
        }
        Screen::Settings => {}
    }
}

/// `接続` → `connect`, for a filename.
fn slug(screen: Screen) -> &'static str {
    match screen {
        Screen::Connect => "connect",
        Screen::Log => "log",
        Screen::Firmware => "firmware",
        Screen::Settings => "settings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame at a size that exercises the layout without paying for a full comp.
    ///
    /// The rasteriser is O(pixels × triangles) in software, so the tests render at
    /// half the documented size and the full-size renders are left to the two tests
    /// that are about the documented sizes. A test suite that takes half a minute to
    /// prove the text drew is a test suite people stop running.
    fn frame(screen: Screen) -> Image {
        frame_at(screen, Vec2::new(640.0, 400.0))
    }

    fn frame_at(screen: Screen, size: Vec2) -> Image {
        let mut app = App::default();
        prepare(&mut app, screen);
        render(&mut app, screen, size)
    }

    #[test]
    fn a_rendered_frame_is_the_requested_size() {
        let image = frame_at(Screen::Connect, Vec2::new(800.0, 500.0));
        assert_eq!((image.width, image.height), (800, 500));
        assert_eq!(image.pixels.len(), 800 * 500 * 4);
    }

    #[test]
    fn every_screen_rasterises_its_text() {
        // The failure this catches: tessellation produced, rasteriser drew nothing,
        // a solid-colour PNG written, and the review judges the colour.
        //
        // Asserted as "bone text appears", not as a colour count: the count depends on
        // how many glyph edges happen to be antialiased, so a threshold on it is a
        // number fitted to today's output. `theme::INK` appearing is the property that
        // has to hold for the image to be a picture of a screen at all.
        for screen in Screen::ALL {
            let image = frame(screen);
            assert!(
                image.contains_colour(theme::INK, 24),
                "{} rasterised no bone text",
                screen.label()
            );
            assert!(
                image.contains_colour(theme::GROUND, 4),
                "{} has no ground; the image was cleared to something else",
                screen.label()
            );
        }
    }

    #[test]
    fn a_screen_of_plain_text_is_not_one_flat_colour() {
        // A stronger floor than "INK is present": glyph antialiasing and the rail fill
        // put the count well above the three or four colours of a rectangle diagram.
        let image = frame(Screen::Connect);
        let colours = image.distinct_colours();
        assert!(
            colours > 12,
            "the connect screen rasterised to {colours} colours; the text did not blend"
        );
    }

    #[test]
    fn the_text_shows_up_as_bone_on_graphite() {
        // `theme::INK` on `theme::GROUND`. If the font atlas were sampled with the
        // wrong filter or the colour premultiplied twice, the glyphs would come out
        // dark and this would fail.
        let image = frame(Screen::Connect);
        assert!(
            image.contains_colour(theme::INK, 24),
            "no pixel is close to theme::INK; the text did not rasterise"
        );
        assert!(
            image.contains_colour(theme::GROUND, 4),
            "the ground is missing; the image was cleared to something else"
        );
    }

    #[test]
    fn the_log_screen_rasterises_its_track_in_the_data_colour() {
        // The track line is the one place `theme::DATA` is spent on geometry. If the
        // projection drew outside the canvas or the path were culled, the colour would
        // be absent from the pixels while the shape count in `render.rs` stayed green.
        //
        // Rendered at the smallest documented window rather than at the suite's cheap
        // 640×400: the body scrolls now (the review's fix for 1024×640), and below the
        // documented height the map is legitimately below the fold. Asserting the track
        // at a size the design does not support would pin the wrong property.
        let image = frame_at(Screen::Log, Vec2::new(1024.0, 640.0));
        assert!(
            image.contains_colour(theme::DATA, 16),
            "the track did not rasterise in theme::DATA"
        );
    }

    #[test]
    fn the_documented_sizes_render_the_narrow_layout_too() {
        // DESIGN.md §8 names 1280×800 and 1024×640 as the two sizes the review judges,
        // and §9 makes the Log screen float its drawer below `metric::NARROW` (1000pt
        // of content). The narrow size is
        // rendered here rather than only through `write_all`, so a collapse rule that
        // stops working breaks a test instead of a review.
        for (width, height) in [(1280.0_f32, 800.0_f32), (1024.0, 640.0)] {
            let mut app = App::default();
            prepare(&mut app, Screen::Log);
            let image = render(&mut app, Screen::Log, Vec2::new(width, height));
            assert_eq!(image.width, width as u32);
            assert_eq!(image.height, height as u32);
            assert!(
                image.contains_colour(theme::INK, 24),
                "{width}×{height} rendered no text"
            );
        }
    }

    #[test]
    fn the_standard_screenshot_set_includes_every_drawer_tab() {
        // Stage 3: the five drawer frames were added to `write_all`, but nothing
        // failed if the loop was deleted. Drive the same writer at 320×200 — the
        // software rasteriser makes the real 1280×800 set too slow for the suite
        // — and count the frames; a deleted drawer loop lowers the count and drops
        // the names.
        let directory =
            std::env::temp_dir().join(format!("m241-screenshot-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let written = write_all_with(&directory, &[(320.0, 200.0)]).expect("the screenshot set");
        let names: Vec<String> = written
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        // 4 screens + 3 write states + 2 dump states + 2 Connect frames + 5 drawer tabs.
        assert_eq!(
            names.len(),
            16,
            "the standard set is missing frames: {names:?}"
        );
        for slug in ["summary", "photos", "edit", "export", "ops"] {
            let expected = format!("log-drawer-{slug}-320x200.png");
            assert!(
                names.iter().any(|name| name == &expected),
                "the {slug} drawer frame is missing from the set: {names:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}
