//! 背景地図: OpenStreetMap slippy-map tiles under the track (DESIGN.md §5, §10).
//!
//! The Log screen used to draw the track on a schematic grid, because a map that
//! needs the network cannot be rendered headlessly. That left the one question
//! the screen exists to answer — *where* did the trip go — unanswered. This
//! module adds the tiles without making the offline paths depend on the network:
//!
//! * every piece of geometry is a pure function, so the projection, the visible
//!   tile set and the cache paths are tested without a socket;
//! * the fetcher is behind [`TileFetcher`], so the drawing path can be driven
//!   with a fake that returns synthetic PNG bytes;
//! * a fetch needs a real window, detected by `native_pixels_per_point` rather
//!   than by `inner_rect`: Wayland has no inner rect (winit cannot report one),
//!   so the old test disabled the map on every Wayland session. `--screenshot`
//!   also passes `sample: true`, so a screenshot cannot pretend to have measured
//!   the network, and a headless render reports why it shows no tiles.
//!
//! # Network policy, and the gate this change replaced
//!
//! Before this change the project rule was that `m241-gui` must not link a
//! network client at all, because a network fetch cannot be exercised headlessly
//! and there was therefore no way to prove an offline path stayed offline. That
//! rule was lifted by the user's request for a real map (PR #39), not because the
//! concern was wrong: `Cargo.toml` records why, and what now stands in its place.
//! In short, the guarantees are a fakeable fetcher seam, the window gate above,
//! and the `sample` flag; the non-guarantee is that none of this is a mechanical
//! gate like `tools/check_safety.py` — a later change that adds another network
//! call would not fail a check. Tiles are the GUI's only network consumer, and
//! any second one has to carry the same seam and the same note.
//!
//! # The projection, and why the track moved with it
//!
//! Tiles are laid out in Web Mercator. The previous track projection was
//! equirectangular with a `cos(latitude)` correction, which is close enough to
//! Mercator for a city loop but not identical: a track drawn in one projection
//! over tiles in another drifts by a few pixels per kilometre, and a road that
//! does not line up is exactly the defect this change is meant to remove. The
//! track now uses [`MapView`] — the same projection the tiles are placed with —
//! so the two cannot disagree by construction.
//!
//! # What this module does not do
//!
//! It does not implement OSM's tile usage policy beyond the requirements the
//! policy states: a product User-Agent naming the tool and its repository, a
//! disk cache with a TTL and a size cap, at most [`MAX_CONCURRENT`] requests in
//! flight, only the visible tiles plus one ring, and the required attribution
//! drawn on the map. There is no bulk download, no prefetch of unseen areas and
//! no scraped style.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use egui::{Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions, Ui, Vec2};

use crate::theme::{self, size, space};

/// Side of one tile, in world pixels; the whole slippy-map convention rests on
/// this number.
const TILE_SIZE: f64 = 256.0;

/// Latitude at which Web Mercator is truncated (the usual slippy-map bound).
const MERCATOR_LIMIT: f64 = 85.051_128_78;

/// Lowest zoom the map accepts.
pub(crate) const MIN_TILE_ZOOM: u8 = 1;

/// Highest zoom the map accepts (OSM serves 19).
pub(crate) const MAX_TILE_ZOOM: u8 = 19;

/// Requests allowed in flight at once. OSM's policy asks clients not to hammer
/// the tile servers; four is the number this module promises.
const MAX_CONCURRENT: usize = 4;

/// Largest tile body accepted. A PNG tile is tens of kilobytes; anything near
/// this bound is a server that is not answering with a tile.
const MAX_TILE_BYTES: u64 = 2 * 1024 * 1024;

/// Longest tile edge accepted before decoding. A tile is 256 px; this allows a
/// server to answer with an @2x tile and still refuses a decompression bomb.
const MAX_TILE_EDGE: u32 = 2_048;

/// Bytes the decoder may allocate for one tile. Four 2048² RGBA frames fit, so
/// the limit admits every tile the edge limit admits and refuses the rest.
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

/// How long a cached tile may be served without asking the server again.
const CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Upper bound on the cache on disk.
const CACHE_MAX_BYTES: u64 = 200 * 1024 * 1024;

/// Bytes written between cache-prune sweeps.
const PRUNE_STEP_BYTES: u64 = 64 * 1024 * 1024;

/// Most tiles one frame may ask for; a canvas larger than this is a bug, and
/// the cap keeps a malformed rectangle from queueing thousands of requests.
const MAX_VISIBLE_TILES: usize = 256;

/// Textures held in memory before the cache is dropped and re-requested.
const MAX_TEXTURES: usize = 256;

/// Consecutive failures that switch the map to its offline state.
const OFFLINE_FAILURES: usize = 3;

/// How long the offline state lasts before the visible tiles are tried again,
/// and how long a single failed tile is skipped before it is retried.
const OFFLINE_RETRY: Duration = Duration::from_secs(60);

/// The attribution OSM's licence requires, verbatim.
const ATTRIBUTION: &str = "© OpenStreetMap contributors";

/// Where the attribution's licence lives, shown beside the credit.
const ATTRIBUTION_URL: &str = "openstreetmap.org/copyright";

/// The User-Agent the OSM tile usage policy requires: the tool and a way to
/// reach its maintainers.
const USER_AGENT: &str = concat!(
    "m241-gui/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/haruki0420/holux-m241-rs)"
);

/// One tile addressed the way a slippy map addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TileId {
    /// Zoom level this tile belongs to (`1..=19`).
    zoom: u8,
    /// Column, counted west to east.
    x: u32,
    /// Row, counted north to south.
    y: u32,
}

impl TileId {
    /// The tile `x`, `y` at `zoom`.
    const fn new(zoom: u8, x: u32, y: u32) -> Self {
        Self { zoom, x, y }
    }
}

/// `https://tile.openstreetmap.org/{z}/{x}/{y}.png` for one tile.
///
/// The template is OSM's documented one, and it is a pure function so a test can
/// check the string without a socket.
fn tile_url(tile: TileId) -> String {
    format!(
        "https://tile.openstreetmap.org/{}/{}/{}.png",
        tile.zoom, tile.x, tile.y
    )
}

/// World pixels per tile side at `zoom`.
fn zoom_scale(zoom: f64) -> f64 {
    TILE_SIZE * 2.0_f64.powf(zoom)
}

/// Mercator's y for a latitude, as a fraction of the world (`0` north, `1`
/// south).
fn mercator_fraction(latitude: f64) -> f64 {
    let latitude = latitude.clamp(-MERCATOR_LIMIT, MERCATOR_LIMIT).to_radians();
    0.5 - ((latitude.tan() + 1.0 / latitude.cos()).ln()) / (2.0 * std::f64::consts::PI)
}

/// The inverse of [`mercator_fraction`].
fn latitude_from_fraction(fraction: f64) -> f64 {
    let n = std::f64::consts::PI * (1.0 - 2.0 * fraction);
    n.sinh().atan().to_degrees()
}

/// Longitude to a fraction of the world (`0` at 180°W, `1` at 180°E).
fn longitude_fraction(longitude: f64) -> f64 {
    (longitude + 180.0) / 360.0
}

/// A latitude/longitude as fractional tile coordinates at `zoom`.
///
/// The result is in tiles, not pixels and not degrees: tile `(x, y)` covers
/// `[x, x + 1) × [y, y + 1)`. The OSM wiki's `lon2tile`/`lat2tile` are the
/// integer form of this function.
fn lonlat_to_tile_f64(latitude: f64, longitude: f64, zoom: u8) -> (f64, f64) {
    let count = 2.0_f64.powi(i32::from(zoom));
    (
        longitude_fraction(longitude) * count,
        mercator_fraction(latitude) * count,
    )
}

/// A fractional tile coordinate back to latitude/longitude.
///
/// Round-trip evidence only: the map never converts tiles back, so this is
/// compiled for the projection test rather than for the product.
#[cfg(test)]
fn tile_to_lonlat(x: f64, y: f64, zoom: u8) -> (f64, f64) {
    let count = 2.0_f64.powi(i32::from(zoom));
    (latitude_from_fraction(y / count), x / count * 360.0 - 180.0)
}

/// The world-pixel origin of a tile at its own zoom.
#[cfg(test)]
fn tile_pixel_origin(tile: TileId) -> (f64, f64) {
    (f64::from(tile.x) * TILE_SIZE, f64::from(tile.y) * TILE_SIZE)
}

/// A latitude/longitude in continuous world pixels at `zoom`.
fn world_pixel(latitude: f64, longitude: f64, zoom: f64) -> (f64, f64) {
    let scale = zoom_scale(zoom);
    (
        longitude_fraction(longitude) * scale,
        mercator_fraction(latitude) * scale,
    )
}

/// The map as the screen sees it: a centre, a fractional zoom and a canvas.
///
/// This is the one projection the track and the tiles share. A fractional zoom
/// is deliberate: [`Self::tile_zoom`] rounds to the level a tile server has, and
/// the tiles are drawn scaled by [`Self::tile_scale`]. Fit derives its zoom from
/// the canvas size and the Mercator bbox, so it can land between levels; the
/// `+`/`−` buttons otherwise move in whole steps from the zoom actually drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MapView {
    /// Latitude at the canvas centre, in degrees.
    center_latitude: f64,
    /// Longitude at the canvas centre, in degrees.
    center_longitude: f64,
    /// Fractional slippy-map zoom (`1.0..=19.0`). Read by the Log screen, which
    /// prints the zoom the map was actually drawn at.
    pub(crate) zoom: f64,
    /// The canvas the view is projected into.
    rect: Rect,
}

impl MapView {
    /// A view centred on a coordinate.
    pub(crate) fn new(center_latitude: f64, center_longitude: f64, zoom: f64, rect: Rect) -> Self {
        Self {
            center_latitude,
            center_longitude,
            zoom: zoom.clamp(f64::from(MIN_TILE_ZOOM), f64::from(MAX_TILE_ZOOM)),
            rect,
        }
    }

    /// A view that fits `points` inside `rect`, centred on their bounding box.
    ///
    /// One scale for both axes, so the drawn shape is the shape of the trip. A
    /// single point (or a degenerate box) lands at [`MAX_TILE_ZOOM`] and at the
    /// canvas centre, rather than dividing by zero.
    #[must_use]
    pub(crate) fn fit(rect: Rect, padding: f32, points: &[(f64, f64)]) -> Self {
        let Some(&(first_latitude, first_longitude)) = points.first() else {
            return Self::new(0.0, 0.0, f64::from(MIN_TILE_ZOOM), rect);
        };
        let (first_x, first_y) = (
            longitude_fraction(first_longitude),
            mercator_fraction(first_latitude),
        );
        let (mut min_x, mut max_x) = (first_x, first_x);
        let (mut min_y, mut max_y) = (first_y, first_y);
        for &(latitude, longitude) in points {
            let x = longitude_fraction(longitude);
            let y = mercator_fraction(latitude);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        // The available canvas, in units where the whole world is 1 wide: the
        // zoom that fits the box is the log2 of the ratio.
        let available_width = f64::from((rect.width() - padding * 2.0).max(1.0)) / TILE_SIZE;
        let available_height = f64::from((rect.height() - padding * 2.0).max(1.0)) / TILE_SIZE;
        let span_x = (max_x - min_x).max(1e-12);
        let span_y = (max_y - min_y).max(1e-12);
        let zoom = (available_width / span_x)
            .min(available_height / span_y)
            .log2()
            .clamp(f64::from(MIN_TILE_ZOOM), f64::from(MAX_TILE_ZOOM));
        Self {
            center_latitude: latitude_from_fraction((min_y + max_y) / 2.0),
            center_longitude: (min_x + max_x) / 2.0 * 360.0 - 180.0,
            zoom,
            rect,
        }
    }

    /// The integer zoom the tiles are requested at.
    fn tile_zoom(self) -> u8 {
        let rounded = self.zoom.round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let zoom = rounded as i64;
        u8::try_from(zoom.clamp(i64::from(MIN_TILE_ZOOM), i64::from(MAX_TILE_ZOOM)))
            .unwrap_or(MIN_TILE_ZOOM)
    }

    /// How much a tile at [`Self::tile_zoom`] is scaled to reach this view.
    fn tile_scale(self) -> f64 {
        2.0_f64.powf(self.zoom - f64::from(self.tile_zoom()))
    }

    /// Project a coordinate into the canvas.
    pub(crate) fn project(self, latitude: f64, longitude: f64) -> Pos2 {
        let (x, y) = world_pixel(latitude, longitude, self.zoom);
        let (center_x, center_y) =
            world_pixel(self.center_latitude, self.center_longitude, self.zoom);
        self.rect.center() + Vec2::new((x - center_x) as f32, (y - center_y) as f32)
    }

    /// Where one tile lands on the canvas.
    fn tile_rect(self, tile: TileId) -> Rect {
        let scale = 2.0_f64.powf(self.zoom - f64::from(tile.zoom));
        let tile_pixels = TILE_SIZE * scale;
        let (center_x, center_y) =
            lonlat_to_tile_f64(self.center_latitude, self.center_longitude, tile.zoom);
        let center = self.rect.center();
        let min = Pos2::new(
            center.x + ((f64::from(tile.x) - center_x) * tile_pixels) as f32,
            center.y + ((f64::from(tile.y) - center_y) * tile_pixels) as f32,
        );
        Rect::from_min_size(min, Vec2::splat(tile_pixels as f32))
    }

    /// Every tile the canvas touches, plus one ring around it.
    ///
    /// The ring is what stops a pan from revealing an empty column for a frame;
    /// nothing further is requested, so the map never prefetches an area nobody
    /// is looking at. Rows and columns are clamped to the world, which is what
    /// makes zoom 1 and the poles finite instead of wrapping or panicking.
    fn visible_tiles(self) -> Vec<TileId> {
        if !self.rect.is_positive() {
            return Vec::new();
        }
        let tile_zoom = self.tile_zoom();
        let count = i64::from(1_u32 << u32::from(tile_zoom));
        let tile_pixels = TILE_SIZE * self.tile_scale();
        let (center_x, center_y) =
            lonlat_to_tile_f64(self.center_latitude, self.center_longitude, tile_zoom);
        let half_columns = f64::from(self.rect.width()) / 2.0 / tile_pixels;
        let half_rows = f64::from(self.rect.height()) / 2.0 / tile_pixels;
        // `floor` at both edges: the tile containing the edge, plus one of ring.
        // Using `ceil` on the far edge would add a second empty ring column
        // whenever the edge fell inside a tile.
        #[allow(clippy::cast_possible_truncation)]
        let left = (center_x - half_columns).floor() as i64 - 1;
        #[allow(clippy::cast_possible_truncation)]
        let right = (center_x + half_columns).floor() as i64 + 1;
        #[allow(clippy::cast_possible_truncation)]
        let top = (center_y - half_rows).floor() as i64 - 1;
        #[allow(clippy::cast_possible_truncation)]
        let bottom = (center_y + half_rows).floor() as i64 + 1;

        let mut tiles = Vec::new();
        for y in top..=bottom {
            if y < 0 || y >= count {
                continue;
            }
            for x in left..=right {
                if x < 0 || x >= count {
                    continue;
                }
                let Ok(x) = u32::try_from(x) else { continue };
                let Ok(y) = u32::try_from(y) else { continue };
                tiles.push(TileId::new(tile_zoom, x, y));
                if tiles.len() >= MAX_VISIBLE_TILES {
                    return tiles;
                }
            }
        }
        tiles
    }
}

/// Where the tile cache lives on this OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachePaths {
    /// The `m241-gui/tiles` directory.
    pub(crate) root: PathBuf,
}

impl CachePaths {
    /// The cache under the OS cache directory.
    ///
    /// `XDG_CACHE_HOME` (or `~/.cache`) on Linux, Known Folders on Windows and
    /// `Library/Caches` on macOS are all resolved by `dirs`, so no path in this
    /// module names a home directory.
    pub(crate) fn locate() -> Option<Self> {
        dirs::cache_dir().map(|base| Self::from_root(base.join("m241-gui").join("tiles")))
    }

    /// The cache under an explicit root, for tests.
    fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// `{root}/{z}/{x}/{y}.png` for one tile.
    fn tile_path(&self, tile: TileId) -> PathBuf {
        self.root
            .join(tile.zoom.to_string())
            .join(tile.x.to_string())
            .join(format!("{}.png", tile.y))
    }
}

/// Whether a cache entry may be served without asking the server.
///
/// A modification time in the future (a clock that moved backwards) counts as
/// fresh: treating it as stale would delete a tile that was just written.
fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map_or(true, |age| age <= CACHE_TTL)
}

/// What the cache holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CacheUsage {
    /// Number of cached tile files.
    files: usize,
    /// Their total size.
    bytes: u64,
}

impl CacheUsage {
    /// The figure the Settings row shows.
    pub(crate) fn label(self) -> String {
        if self.files == 0 {
            return String::from("0 B（未使用）");
        }
        format!(
            "{} / {}",
            human_bytes(self.bytes),
            human_bytes(CACHE_MAX_BYTES)
        )
    }
}

/// `512 B` / `12.3 KB` / `4.5 MB`, for a cache figure.
fn human_bytes(bytes: u64) -> String {
    let value = bytes as f64;
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if value < KIB {
        format!("{bytes} B")
    } else if value < MIB {
        format!("{:.1} KB", value / KIB)
    } else {
        format!("{:.1} MB", value / MIB)
    }
}

/// One cache file: path, size, and the time it was last written.
type CacheEntry = (PathBuf, u64, SystemTime);

/// Every file under a cache root, recursively. Unreadable entries are skipped.
fn cache_entries(root: &Path) -> Vec<CacheEntry> {
    fn walk(directory: &Path, found: &mut Vec<CacheEntry>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                walk(&path, found);
            } else if metadata.is_file() {
                found.push((
                    path,
                    metadata.len(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found
}

/// What a cache root holds.
fn cache_usage_at(root: &Path) -> CacheUsage {
    let entries = cache_entries(root);
    CacheUsage {
        files: entries.len(),
        bytes: entries.iter().map(|(_path, size, _at)| *size).sum(),
    }
}

/// The last measured cache figure and when it was taken.
static LAST_USAGE: Mutex<Option<(Instant, CacheUsage)>> = Mutex::new(None);

/// The cache figure, recomputed at most every five seconds.
///
/// The Settings screen asks once a frame; walking a 200 MB tree every frame
/// would make the settings page the slowest thing in the product.
pub(crate) fn cache_usage() -> Option<CacheUsage> {
    let cache = CachePaths::locate()?;
    if let Ok(guard) = LAST_USAGE.lock()
        && let Some((at, usage)) = guard.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return Some(*usage);
    }
    let usage = cache_usage_at(&cache.root);
    if let Ok(mut guard) = LAST_USAGE.lock() {
        *guard = Some((Instant::now(), usage));
    }
    Some(usage)
}

fn invalidate_usage_cache() {
    if let Ok(mut guard) = LAST_USAGE.lock() {
        *guard = None;
    }
}

/// Delete the tile cache. A missing directory is success: the cache is already
/// in the state the cache was asked to reach.
///
/// # Errors
///
/// The I/O failure, with the path and the reason.
pub(crate) fn clear_cache() -> Result<(), String> {
    let Some(cache) = CachePaths::locate() else {
        return Err(String::from("この環境ではキャッシュの場所を決められません"));
    };
    match std::fs::remove_dir_all(&cache.root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "{} を消去できません: {error}",
                cache.root.display()
            ));
        }
    }
    invalidate_usage_cache();
    Ok(())
}

/// Which cached files an over-budget cache must drop, oldest first.
///
/// Pure so the policy can be tested without a filesystem and without waiting 30
/// days for a file to age.
fn prune_plan(mut entries: Vec<CacheEntry>, limit: u64) -> Vec<PathBuf> {
    let total: u64 = entries.iter().map(|(_path, size, _at)| *size).sum();
    if total <= limit {
        return Vec::new();
    }
    entries.sort_by_key(|entry| entry.2);
    let mut plan = Vec::new();
    let mut freed = 0_u64;
    for (path, size, _at) in entries {
        if total.saturating_sub(freed) <= limit {
            break;
        }
        freed = freed.saturating_add(size);
        plan.push(path);
    }
    plan
}

/// Bring a cache root under `limit` by deleting its oldest tiles.
fn prune_cache(root: &Path, limit: u64) -> u64 {
    let mut freed = 0_u64;
    // One walk, then the plan: a fetch running in parallel can only add a file,
    // and the sweep is bounded by the limit rather than by that file.
    for path in prune_plan(cache_entries(root), limit) {
        if let Ok(metadata) = std::fs::metadata(&path) {
            let size = metadata.len();
            if std::fs::remove_file(&path).is_ok() {
                freed = freed.saturating_add(size);
            }
        }
    }
    freed
}

/// Fetch one tile's bytes from the network.
///
/// A trait rather than a function so the drawing path can be tested with a fake:
/// the tests must not depend on a socket, and a screenshot must not quietly
/// produce evidence of a network that was never there.
pub(crate) trait TileFetcher: Send + Sync + std::fmt::Debug {
    /// Fetch one tile as encoded bytes (PNG, or JPEG if a server answers so).
    ///
    /// # Errors
    ///
    /// [`TileError`] names why the tile could not be fetched.
    fn fetch(&self, tile: TileId) -> Result<Vec<u8>, TileError>;
}

/// Why a tile could not be fetched or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TileError {
    /// The server could not be reached, or the transfer failed.
    Offline(String),
    /// The server answered with a non-success status.
    Status(u16),
    /// The server answered with something that is not an image.
    Decode(String),
    /// The body exceeded [`MAX_TILE_BYTES`].
    TooLarge(u64),
}

impl std::fmt::Display for TileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline(reason) => write!(formatter, "offline: {reason}"),
            Self::Status(code) => write!(formatter, "server answered {code}"),
            Self::Decode(reason) => write!(formatter, "not an image: {reason}"),
            Self::TooLarge(size) => write!(formatter, "body too large: {size} bytes"),
        }
    }
}

/// The real fetcher: `ureq` over TLS, with the policy's User-Agent.
#[derive(Default)]
struct HttpTiles {
    /// Built on the first fetch, so creating a screen costs no TLS setup.
    agent: OnceLock<ureq::Agent>,
}

impl std::fmt::Debug for HttpTiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HttpTiles").finish_non_exhaustive()
    }
}

impl HttpTiles {
    fn agent(&self) -> &ureq::Agent {
        self.agent.get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .user_agent(USER_AGENT)
                .build()
                .new_agent()
        })
    }
}

/// Turn a body-read failure into the tile error the map reports.
///
/// `BodyExceedsLimit` used to fall into `Offline`, so an oversized answer was
/// reported as a network outage and [`TileError::TooLarge`] was unreachable.
fn map_body_error(error: ureq::Error) -> TileError {
    match error {
        ureq::Error::BodyExceedsLimit(limit) => TileError::TooLarge(limit),
        other => TileError::Offline(other.to_string()),
    }
}

impl TileFetcher for HttpTiles {
    fn fetch(&self, tile: TileId) -> Result<Vec<u8>, TileError> {
        let mut response =
            self.agent()
                .get(&tile_url(tile))
                .call()
                .map_err(|error| match error {
                    ureq::Error::StatusCode(code) => TileError::Status(code),
                    other => TileError::Offline(other.to_string()),
                })?;
        // OSM answers 200 with an image that says the request was blocked. The
        // body is a valid PNG of the wrong thing, so the status and the decoder
        // cannot tell it from a tile; the header is the tell. It is not cached,
        // because a block notice painted on the map for 30 days is worse than
        // the missing tile it replaced.
        let headers = response.headers();
        if headers.get("x-blocked").is_some() {
            return Err(TileError::Offline(String::from(
                "the tile server answered with its block image",
            )));
        }
        if let Some(content_type) = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            && !content_type.starts_with("image/")
        {
            return Err(TileError::Decode(format!("server answered {content_type}")));
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_TILE_BYTES)
            .read_to_vec()
            .map_err(map_body_error)
    }
}

/// Decode tile bytes into a texture-ready image.
///
/// The limits are installed on the decoder *before* it allocates. Checking the
/// decoded image's dimensions (as this function used to) was too late: a server
/// could hand over a 10000×10000 PNG and the decoder would allocate hundreds of
/// megabytes before the check ran. `ImageReader::decode` enforces the strict
/// edge limits and reserves the output against `max_alloc` first.
fn decode_tile(bytes: &[u8]) -> Result<ColorImage, TileError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| TileError::Decode(error.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_TILE_EDGE);
    limits.max_image_height = Some(MAX_TILE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| match error {
        // The limit is measured in bytes the decoder may allocate, but the cause
        // is a tile whose decoded size is too large for this map; report the
        // decoded ceiling rather than a number the caller cannot act on.
        image::ImageError::Limits(_) => {
            TileError::TooLarge(u64::from(MAX_TILE_EDGE) * u64::from(MAX_TILE_EDGE) * 4)
        }
        other => TileError::Decode(other.to_string()),
    })?;
    let rgba = decoded.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

/// Read a fresh cache entry.
fn read_cached(cache: &CachePaths, tile: TileId, now: SystemTime) -> Option<Vec<u8>> {
    let path = cache.tile_path(tile);
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || !is_fresh(metadata.modified().ok()?, now) {
        return None;
    }
    std::fs::read(path).ok().filter(|bytes| !bytes.is_empty())
}

/// Read a cache entry regardless of its age, as a last resort when offline.
fn read_stale(cache: &CachePaths, tile: TileId) -> Option<Vec<u8>> {
    std::fs::read(cache.tile_path(tile))
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

/// Write a cache entry, returning how many bytes reached the disk (0 on error).
fn write_cached(cache: &CachePaths, tile: TileId, bytes: &[u8]) -> u64 {
    let path = cache.tile_path(tile);
    if let Some(parent) = path.parent() {
        let _created = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, bytes) {
        Ok(()) => u64::try_from(bytes.len()).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Fetch one tile through the cache and decode it.
///
/// This is the whole pipeline, synchronous and independent of egui: a cache hit
/// that decodes is returned without touching the network, a miss fetches and
/// writes, and a network failure falls back to a stale cache entry so a map that
/// was looked at yesterday still draws today. The second return value is the
/// number of bytes newly written to the cache, for the prune sweep.
fn load_tile(
    fetcher: &dyn TileFetcher,
    cache: Option<&CachePaths>,
    tile: TileId,
    now: SystemTime,
) -> (Result<ColorImage, TileError>, u64) {
    if let Some(cache) = cache
        && let Some(bytes) = read_cached(cache, tile, now)
        && let Ok(image) = decode_tile(&bytes)
    {
        return (Ok(image), 0);
    }
    match fetcher.fetch(tile) {
        Ok(bytes) => {
            let written = cache.map_or(0, |cache| write_cached(cache, tile, &bytes));
            (decode_tile(&bytes), written)
        }
        Err(error) => match cache.and_then(|cache| read_stale(cache, tile)) {
            Some(bytes) => match decode_tile(&bytes) {
                Ok(image) => (Ok(image), 0),
                Err(_) => (Err(error), 0),
            },
            None => (Err(error), 0),
        },
    }
}

/// One finished tile, on its way from a worker thread to the UI thread.
struct TileDone {
    tile: TileId,
    image: Result<ColorImage, TileError>,
}

/// What the map layer is showing, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Background {
    /// At least one tile is on screen.
    Tiles,
    /// Tiles are wanted and the first results have not arrived yet.
    Loading,
    /// Tiles are wanted but the fetches are failing (the offline fallback).
    Offline,
    /// The setting is off; the schematic canvas is the map.
    Disabled,
    /// The screen is sample data; no tile is fetched or shown.
    Sample,
    /// There is no window to fetch into (a headless render or `--screenshot`).
    Headless,
}

impl Background {
    /// Whether this state should keep requesting tiles.
    fn may_fetch(self) -> bool {
        matches!(self, Self::Tiles | Self::Loading)
    }

    /// The caption the map shows for this state, if it needs one.
    pub(crate) fn note(self) -> Option<&'static str> {
        match self {
            Self::Tiles => None,
            Self::Loading => Some("地図タイルを読み込み中"),
            Self::Offline => Some("地図タイルを取得できません（オフライン）"),
            Self::Disabled => Some("地図タイルはオフです（設定で切り替え）"),
            Self::Sample => Some("見本：地図タイルは表示しません"),
            Self::Headless => Some("地図タイルは表示しません（ヘッドレス）"),
        }
    }
}

/// What the Log screen allows the map layer to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapOptions {
    /// The Settings screen's 「地図タイルを使う（要インターネット）」.
    pub(crate) enabled: bool,
    /// The screen is showing sample data (a screenshot must not fetch).
    pub(crate) sample: bool,
}

/// The tile service and the textures it has loaded.
pub(crate) struct MapTiles {
    fetcher: Arc<dyn TileFetcher>,
    cache: Option<CachePaths>,
    textures: HashMap<TileId, TextureHandle>,
    pending: HashSet<TileId>,
    /// Failed tiles with the time they failed, so a partial failure retries
    /// after [`OFFLINE_RETRY`] instead of staying grey for the session.
    failed: HashMap<TileId, Instant>,
    queue: VecDeque<TileId>,
    in_flight: usize,
    failures: usize,
    offline_until: Option<Instant>,
    sender: Sender<TileDone>,
    receiver: Receiver<TileDone>,
    written: Arc<AtomicU64>,
    pruned: bool,
}

impl std::fmt::Debug for MapTiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MapTiles")
            .field("textures", &self.textures.len())
            .field("pending", &self.pending.len())
            .field("failed", &self.failed.len())
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl Default for MapTiles {
    fn default() -> Self {
        Self::with_fetcher(Arc::new(HttpTiles::default()))
    }
}

impl MapTiles {
    /// A tile service over an explicit fetcher.
    fn with_fetcher(fetcher: Arc<dyn TileFetcher>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            fetcher,
            cache: CachePaths::locate(),
            textures: HashMap::new(),
            pending: HashSet::new(),
            failed: HashMap::new(),
            queue: VecDeque::new(),
            in_flight: 0,
            failures: 0,
            offline_until: None,
            sender,
            receiver,
            written: Arc::new(AtomicU64::new(0)),
            pruned: false,
        }
    }

    /// A tile service over a fake fetcher with no cache.
    ///
    /// Only the tests outside this module need this seam (the render and App
    /// tests); the tests inside it can reach `with_fetcher` directly. It is
    /// `cfg(test)` so the shipped binary keeps a single constructor.
    #[cfg(test)]
    pub(crate) fn with_fetcher_for_test(fetcher: Arc<dyn TileFetcher>) -> Self {
        let mut tiles = Self::with_fetcher(fetcher);
        tiles.cache = None;
        tiles
    }

    /// Replace the cache root (tests point this at a scratch directory).
    #[cfg(test)]
    fn set_cache(&mut self, cache: Option<CachePaths>) {
        self.cache = cache;
        self.pruned = false;
    }

    /// Forget every tile and every request.
    ///
    /// Called when the shown track is replaced: the previous area's textures and
    /// its in-flight requests belong to a log that is no longer on screen, and a
    /// map that keeps them would draw the old trip's roads under the new one.
    /// Workers already running finish and their results are ignored by [`Self::pump`]
    /// (their tile is no longer pending).
    pub(crate) fn reset(&mut self) {
        self.textures.clear();
        self.pending.clear();
        self.failed.clear();
        self.queue.clear();
        self.failures = 0;
        self.offline_until = None;
        self.pruned = false;
    }

    /// How many textures are held. Test-only: the render tests wait for it.
    #[cfg(test)]
    pub(crate) fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// The state the map is in, clearing an expired offline latch first.
    fn background(&mut self, options: MapOptions) -> Background {
        if options.sample {
            return Background::Sample;
        }
        if !options.enabled {
            return Background::Disabled;
        }
        let now = Instant::now();
        if self.offline_until.is_some_and(|until| now >= until) {
            // The retry window opened: forget the failures so the visible tiles
            // are requested again.
            self.offline_until = None;
            self.failed.clear();
            self.failures = 0;
        }
        if self.offline_until.is_some() {
            return Background::Offline;
        }
        if self.textures.is_empty() {
            if self.failed.is_empty() {
                Background::Loading
            } else {
                Background::Offline
            }
        } else {
            Background::Tiles
        }
    }

    /// Move finished tiles into textures and update the failure latch.
    fn pump(&mut self, context: &egui::Context) {
        while let Ok(done) = self.receiver.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            // A result for a tile that is no longer pending belongs to a reset or
            // to a view that has moved on; drawing it would repaint the old area.
            if !self.pending.remove(&done.tile) {
                continue;
            }
            match done.image {
                Ok(image) => {
                    let name = format!("osm/{}/{}/{}", done.tile.zoom, done.tile.x, done.tile.y);
                    let texture = context.load_texture(name, image, TextureOptions::LINEAR);
                    let _replaced = self.textures.insert(done.tile, texture);
                    self.failures = 0;
                }
                Err(_error) => {
                    let _inserted = self.failed.insert(done.tile, Instant::now());
                    self.failures = self.failures.saturating_add(1);
                    if self.failures >= OFFLINE_FAILURES {
                        self.offline_until = Some(Instant::now() + OFFLINE_RETRY);
                    }
                }
            }
        }
        if self.textures.len() > MAX_TEXTURES {
            // A map panned far enough to hold this many has left most of them
            // behind; dropping the lot is cheaper than tracking an LRU and the
            // visible tiles are re-requested on the next frame.
            self.textures.clear();
        }
    }

    /// Queue the tiles that are wanted but not held, and start up to
    /// [`MAX_CONCURRENT`] workers.
    fn request(&mut self, wanted: &[TileId], context: &egui::Context) {
        if !self.pruned {
            self.pruned = true;
            if let Some(cache) = self.cache.clone() {
                let _spawned = std::thread::Builder::new()
                    .name(String::from("m241-tile-prune"))
                    .spawn(move || {
                        let _freed = prune_cache(&cache.root, CACHE_MAX_BYTES);
                    });
            }
        }
        // A failure is not permanent. Retrying it after the same window the
        // offline latch uses turns a single timeout or a z19 gap into a tile
        // that appears a minute later, instead of an empty square for the whole
        // session (the old `failed` set was only cleared when the whole map went
        // offline).
        self.failed.retain(|_tile, at| at.elapsed() < OFFLINE_RETRY);
        // A fast pan leaves tiles in the queue that the canvas no longer touches.
        // Drop them (and their pending mark) rather than fetching an area nobody
        // is looking at, which is what the module doc promises.
        let wanted_set: HashSet<TileId> = wanted.iter().copied().collect();
        let stale: Vec<TileId> = self
            .queue
            .iter()
            .copied()
            .filter(|tile| !wanted_set.contains(tile))
            .collect();
        for tile in stale {
            let _removed = self.pending.remove(&tile);
        }
        self.queue.retain(|tile| wanted_set.contains(tile));
        for tile in wanted {
            if self.textures.contains_key(tile)
                || self.pending.contains(tile)
                || self.failed.contains_key(tile)
            {
                continue;
            }
            let _inserted = self.pending.insert(*tile);
            self.queue.push_back(*tile);
        }
        while self.in_flight < MAX_CONCURRENT {
            let Some(tile) = self.queue.pop_front() else {
                break;
            };
            match self.spawn(tile, context) {
                Ok(()) => self.in_flight += 1,
                Err(_) => {
                    let _removed = self.pending.remove(&tile);
                    let _inserted = self.failed.insert(tile, Instant::now());
                }
            }
        }
    }

    /// Start one worker for one tile.
    fn spawn(&self, tile: TileId, context: &egui::Context) -> std::io::Result<()> {
        let fetcher = Arc::clone(&self.fetcher);
        let cache = self.cache.clone();
        let sender = self.sender.clone();
        let written = Arc::clone(&self.written);
        let context = context.clone();
        std::thread::Builder::new()
            .name(format!("m241-tile-{}-{}-{}", tile.zoom, tile.x, tile.y))
            .spawn(move || {
                let (image, bytes) =
                    load_tile(fetcher.as_ref(), cache.as_ref(), tile, SystemTime::now());
                if bytes > 0 {
                    let total = written.fetch_add(bytes, Ordering::Relaxed) + bytes;
                    if total >= PRUNE_STEP_BYTES
                        && let Some(cache) = cache.as_ref()
                    {
                        written.store(0, Ordering::Relaxed);
                        let _freed = prune_cache(&cache.root, CACHE_MAX_BYTES);
                    }
                }
                let _sent = sender.send(TileDone { tile, image });
                context.request_repaint();
            })
            .map(|_handle| ())
    }
}

/// Whether this context belongs to a real window someone could be looking at.
///
/// The test is `native_pixels_per_point`, not `inner_rect`. `inner_rect` is
/// `None` on Wayland (winit cannot report a window position there), so gating on
/// it silently disabled the map on every Wayland session while the caption kept
/// saying "loading". `eframe` fills `native_pixels_per_point` on both X11 and
/// Wayland, and a headless `RawInput` (the screenshot renderer and every render
/// test) leaves it `None`, which is the distinction the offline promise needs.
fn has_window(context: &egui::Context) -> bool {
    context.input(|input| input.viewport().native_pixels_per_point.is_some())
}

/// What one map frame drew: the state the zoom note describes and how many OSM
/// tiles reached the canvas.
///
/// The count exists so the attribution is drawn only on a frame that actually
/// shows OSM data. Crediting a sample or empty frame is a false statement about
/// where the pixels came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MapDrawn {
    /// The state the caller should describe in its zoom note.
    pub(crate) background: Background,
    /// OSM tiles painted this frame.
    pub(crate) tiles: usize,
}

/// Draw the map's base layer: tiles when they are available, nothing when the
/// schematic grid underneath is the fallback.
///
/// The grid is drawn by the caller before this, so the offline canvas is never
/// empty.
pub(crate) fn draw_map(
    ui: &Ui,
    painter: &Painter,
    view: MapView,
    state: &mut MapTiles,
    options: MapOptions,
) -> MapDrawn {
    state.pump(ui.ctx());
    let visible = view.visible_tiles();
    let mut background = state.background(options);
    if background.may_fetch() {
        if has_window(ui.ctx()) {
            state.request(&visible, ui.ctx());
            state.pump(ui.ctx());
            background = state.background(options);
        } else {
            // Nothing can arrive without a window, so "loading" would be a
            // caption that never changes. Name the real reason instead.
            background = Background::Headless;
        }
    }
    let mut tiles = 0;
    for tile in &visible {
        if let Some(texture) = state.textures.get(tile) {
            painter.image(
                texture.id(),
                view.tile_rect(*tile),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            tiles += 1;
        }
    }
    MapDrawn { background, tiles }
}

/// The attribution OSM requires, drawn whenever OSM pixels are on the canvas.
///
/// The plate is not decoration: OSM tiles are bright, and `theme::MUTE` on a
/// white field would fail the contrast the rest of the product holds itself to.
/// The map draws this after the track and its markers, so a route that crosses
/// the corner cannot paint over the credit (design review 2026-11-23, Should 1).
/// Returns the plate so the caller can keep the zoom plate clear of it.
pub(crate) fn draw_attribution(painter: &Painter, rect: Rect) -> Rect {
    let text = format!("{ATTRIBUTION} · {ATTRIBUTION_URL}");
    let galley =
        painter.layout_no_wrap(text, egui::FontId::proportional(size::CAPTION), theme::INK);
    let text_size = Vec2::new(galley.rect.width(), galley.rect.height());
    // Bottom-left: the zoom plate owns the bottom-right corner.
    let origin = Pos2::new(
        rect.left() + space::S,
        rect.bottom() - space::S - text_size.y,
    );
    let plate = Rect::from_min_size(origin, text_size).expand(space::XS);
    painter.rect_filled(plate, theme::RADIUS, theme::RAIL.gamma_multiply(0.9));
    painter.galley(origin, galley, theme::INK);
    plate
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic city-centre pair the repository uses for every example.
    const TOKYO: (f64, f64) = (35.681_236, 139.767_125);

    /// A scratch cache root, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("m241-tiles-{}-{label}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Self(root)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A 2×2 PNG, so decoding has something real to decode.
    fn synthetic_png() -> Vec<u8> {
        png_bytes(2, 2)
    }

    /// A valid PNG of the requested size, for the decoder's limit tests.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            let pixels = vec![0_u8; usize::try_from(width * height * 4).expect("pixel count")];
            writer.write_image_data(&pixels).expect("PNG data");
        }
        bytes
    }

    #[derive(Debug)]
    struct FakeFetcher {
        calls: Mutex<usize>,
        result: Result<Vec<u8>, TileError>,
    }

    impl FakeFetcher {
        fn ok() -> Self {
            Self {
                calls: Mutex::new(0),
                result: Ok(synthetic_png()),
            }
        }

        fn failing() -> Self {
            Self {
                calls: Mutex::new(0),
                result: Err(TileError::Offline(String::from("no network in tests"))),
            }
        }

        fn too_large() -> Self {
            Self {
                calls: Mutex::new(0),
                result: Err(TileError::TooLarge(MAX_TILE_BYTES + 1)),
            }
        }

        fn calls(&self) -> usize {
            *self
                .calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
        }
    }

    impl TileFetcher for FakeFetcher {
        fn fetch(&self, _tile: TileId) -> Result<Vec<u8>, TileError> {
            if let Ok(mut calls) = self.calls.lock() {
                *calls += 1;
            }
            self.result.clone()
        }
    }

    #[test]
    fn the_projection_matches_the_known_tile_numbers() {
        // The values are the OSM slippy-map formula at the repository's example
        // coordinate: 35.681236, 139.767125 at z=13.
        let (x, y) = lonlat_to_tile_f64(TOKYO.0, TOKYO.1, 13);
        assert!((x - 7_276.478_58).abs() < 1e-4, "x was {x}");
        assert!((y - 3_225.829_44).abs() < 1e-4, "y was {y}");
        assert_eq!((x.floor() as u32, y.floor() as u32), (7276, 3225));

        // The poles clamp instead of running off the world.
        let (_x, north) = lonlat_to_tile_f64(89.9, 0.0, 3);
        let (_x, south) = lonlat_to_tile_f64(-89.9, 0.0, 3);
        // The clamp is a floating-point boundary, so allow the tiny overshoot the
        // Mercator formula produces rather than pretending it is exact.
        assert!((-1e-6..=8.0 + 1e-6).contains(&north), "north was {north}");
        assert!((-1e-6..=8.0 + 1e-6).contains(&south), "south was {south}");
    }

    #[test]
    fn tile_coordinates_round_trip_to_lat_lon() {
        for zoom in [1_u8, 7, 13, 19] {
            let (x, y) = lonlat_to_tile_f64(TOKYO.0, TOKYO.1, zoom);
            let (latitude, longitude) = tile_to_lonlat(x, y, zoom);
            assert!(
                (latitude - TOKYO.0).abs() < 1e-9,
                "latitude drifted at z={zoom}: {latitude}"
            );
            assert!(
                (longitude - TOKYO.1).abs() < 1e-9,
                "longitude drifted at z={zoom}: {longitude}"
            );
        }
    }

    #[test]
    fn the_world_scale_is_the_slippy_map_scale() {
        assert!((zoom_scale(0.0) - 256.0).abs() < f64::EPSILON);
        assert!((zoom_scale(1.0) - 512.0).abs() < f64::EPSILON);
        assert!((zoom_scale(19.0) - 256.0 * 524_288.0).abs() < 1.0);
        assert_eq!(tile_pixel_origin(TileId::new(3, 2, 5)), (512.0, 1280.0));
    }

    #[test]
    fn the_visible_set_is_the_canvas_plus_one_ring() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        let view = MapView::new(TOKYO.0, TOKYO.1, 13.0, rect);
        let tiles = view.visible_tiles();
        assert!(!tiles.is_empty());
        assert!(tiles.iter().all(|tile| tile.zoom == 13));
        // 400×200 at z=13 spans two columns and two rows of 256pt tiles; the
        // ring adds one on every side, so five columns and four rows (20) is
        // the bound, not the whole world.
        assert!(tiles.len() <= 20, "requested {} tiles", tiles.len());

        // The tile under the centre is in the set.
        let (center_x, center_y) = lonlat_to_tile_f64(TOKYO.0, TOKYO.1, 13);
        let center = TileId::new(13, center_x.floor() as u32, center_y.floor() as u32);
        assert!(tiles.contains(&center));

        // Every tile is inside the world and its screen rect touches the canvas
        // (or its ring).
        let ring = Rect::from_min_size(
            Pos2::new(-256.0, -256.0),
            Vec2::new(400.0 + 2.0 * 256.0, 200.0 + 2.0 * 256.0),
        );
        for tile in &tiles {
            let world = 1_u32 << tile.zoom;
            assert!(tile.x < world && tile.y < world);
            assert!(
                view.tile_rect(*tile).intersects(ring),
                "{tile:?} is outside the requested ring"
            );
        }
    }

    #[test]
    fn the_visible_set_is_clamped_at_the_edge_of_the_world() {
        // Zoom 1 has four tiles; a view centred on the north pole cannot ask for
        // row -1 or column 2.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0));
        for (latitude, longitude) in [(89.0, 0.0), (-89.0, 179.9), (0.0, -179.9)] {
            let view = MapView::new(latitude, longitude, 1.0, rect);
            let tiles = view.visible_tiles();
            assert!(!tiles.is_empty());
            for tile in tiles {
                assert!(tile.x < 2 && tile.y < 2, "{tile:?} left the world");
            }
        }
    }

    #[test]
    fn the_tile_level_switches_at_the_integer_zoom() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        for (zoom, expected) in [
            (12.49, 12_u8),
            (12.51, 13),
            (1.0, 1),
            (19.0, 19),
            (0.0, 1),
            (40.0, 19),
        ] {
            let view = MapView::new(TOKYO.0, TOKYO.1, zoom, rect);
            assert_eq!(view.tile_zoom(), expected, "zoom {zoom}");
        }
        // A fractional zoom scales the tiles it draws, so the map moves
        // continuously; a whole zoom draws them at their native size.
        let whole = MapView::new(TOKYO.0, TOKYO.1, 13.0, rect);
        assert!((whole.tile_scale() - 1.0).abs() < f64::EPSILON);
        // 13.5 rounds up to tile level 14, so the tiles are drawn at 2^-0.5.
        let rounded_up = MapView::new(TOKYO.0, TOKYO.1, 13.5, rect);
        assert_eq!(rounded_up.tile_zoom(), 14);
        assert!((rounded_up.tile_scale() - 2.0_f64.powf(-0.5)).abs() < 1e-9);
        // 13.4 stays on level 13 and the tiles are drawn larger than native.
        let stays = MapView::new(TOKYO.0, TOKYO.1, 13.4, rect);
        assert_eq!(stays.tile_zoom(), 13);
        assert!((stays.tile_scale() - 2.0_f64.powf(0.4)).abs() < 1e-9);
    }

    #[test]
    fn a_point_lands_inside_the_tile_that_covers_it() {
        // The track and the tiles share one projection. If they disagreed, a
        // point plotted just outside its own tile would be visible as a track
        // that runs beside the road it was recorded on.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(512.0, 320.0));
        for zoom in [13.0, 13.4, 13.5, 14.0] {
            let view = MapView::new(TOKYO.0, TOKYO.1, zoom, rect);
            let point = view.project(TOKYO.0, TOKYO.1);
            // The tile zoom is rounded, so compare against the tile actually
            // drawn at this fraction.
            let drawn_zoom = view.tile_zoom();
            let (x, y) = lonlat_to_tile_f64(TOKYO.0, TOKYO.1, drawn_zoom);
            let drawn_tile = TileId::new(drawn_zoom, x.floor() as u32, y.floor() as u32);
            let tile_rect = view.tile_rect(drawn_tile);
            assert!(
                tile_rect.expand(0.5).contains(point),
                "at zoom {zoom} the point {point:?} is outside its tile {tile_rect:?}"
            );
        }
    }

    /// A raw input that looks like a real window to the map gate.
    ///
    /// `inner_rect` is what Wayland cannot report; `native_pixels_per_point` is
    /// what eframe fills on every real window. Passing `None` for `inner_rect`
    /// therefore reproduces Wayland, and `Some(..)` reproduces X11.
    fn window_input(inner_rect: Option<Rect>) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
            ..egui::RawInput::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(1.0),
                inner_rect,
                ..egui::ViewportInfo::default()
            },
        );
        input
    }

    /// Run one map frame and return what it drew.
    fn draw_one_frame(tiles: &mut MapTiles, input: egui::RawInput) -> MapDrawn {
        let context = egui::Context::default();
        let mut drawn = MapDrawn {
            background: Background::Loading,
            tiles: 0,
        };
        let output = context.run_ui(input, |ui| {
            let (rect, _response) =
                ui.allocate_exact_size(Vec2::new(400.0, 200.0), egui::Sense::hover());
            let painter = ui.painter();
            let view = MapView::new(TOKYO.0, TOKYO.1, 13.0, rect);
            drawn = draw_map(
                ui,
                painter,
                view,
                tiles,
                MapOptions {
                    enabled: true,
                    sample: false,
                },
            );
        });
        output.drop_without_applying_deltas();
        drawn
    }

    #[test]
    fn an_x11_window_fetches_tiles() {
        // X11 reports both an inner rect and a scale factor. This is the path the
        // live screenshot was taken on; it must keep fetching after the Wayland
        // gate changed.
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(None);
        let input = window_input(Some(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, 800.0),
        )));
        let drawn = draw_one_frame(&mut tiles, input);
        assert_eq!(drawn.tiles, 0, "no tile could be drawn on the first frame");
        std::thread::sleep(Duration::from_millis(100));
        assert!(fetcher.calls() > 0, "an X11 window started no tile fetch");
    }

    #[test]
    fn a_wayland_window_has_no_inner_rect_and_still_fetches_tiles() {
        // winit reports `NotSupported` for the inner position on Wayland, so
        // egui-winit fills `inner_rect: None`. Gating the fetch on `inner_rect`
        // therefore disabled the map on every Wayland session; the gate must be
        // `native_pixels_per_point`, which is Some on Wayland too.
        let input = window_input(None);
        assert!(
            input.viewport().inner_rect.is_none(),
            "the Wayland fixture must have no inner rect"
        );
        assert!(
            input.viewport().native_pixels_per_point.is_some(),
            "the Wayland fixture must still name a scale factor"
        );
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(None);
        let _drawn = draw_one_frame(&mut tiles, input);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            fetcher.calls() > 0,
            "a Wayland window started no tile fetch"
        );
    }

    #[test]
    fn a_headless_context_never_requests_a_tile() {
        // `--screenshot` and every render test use a headless `RawInput`: no
        // window, no scale factor. Drawing there must not start a fetch, or a
        // headless artifact would depend on the network it is supposed to run
        // without. The state says why nothing is shown instead of "loading".
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(None);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
            ..egui::RawInput::default()
        };
        let drawn = draw_one_frame(&mut tiles, input);
        assert_eq!(drawn.background, Background::Headless);
        assert_eq!(
            Background::Headless.note(),
            Some("地図タイルは表示しません（ヘッドレス）")
        );
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            fetcher.calls(),
            0,
            "a headless context started a tile fetch"
        );
    }

    #[test]
    fn a_fitted_view_contains_every_point() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 240.0));
        let points = [
            (TOKYO.0, TOKYO.1),
            (TOKYO.0 + 0.01, TOKYO.1 + 0.012),
            (TOKYO.0 - 0.004, TOKYO.1 - 0.008),
        ];
        let view = MapView::fit(rect, space::M, &points);
        for (latitude, longitude) in points {
            let position = view.project(latitude, longitude);
            assert!(
                rect.contains(position),
                "{position:?} left the canvas for a fitted view"
            );
        }
        // A single point lands on the canvas at a finite position.
        let single = MapView::fit(rect, space::M, &[TOKYO]);
        let position = single.project(TOKYO.0, TOKYO.1);
        assert!(position.x.is_finite() && position.y.is_finite());
        assert!(rect.contains(position));
    }

    #[test]
    fn the_cache_path_is_z_x_y_under_the_os_cache_dir() {
        let cache = CachePaths::from_root(PathBuf::from("/cache/m241-gui/tiles"));
        assert_eq!(
            cache.tile_path(TileId::new(13, 7276, 3226)),
            PathBuf::from("/cache/m241-gui/tiles/13/7276/3226.png")
        );
        // The located root ends in m241-gui/tiles on every OS `dirs` supports.
        if let Some(located) = CachePaths::locate() {
            assert!(located.root.ends_with(Path::new("m241-gui").join("tiles")));
        }
    }

    #[test]
    fn the_cache_ttl_is_thirty_days() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let fresh = now - Duration::from_secs(29 * 24 * 60 * 60);
        let stale = now - Duration::from_secs(31 * 24 * 60 * 60);
        assert!(is_fresh(fresh, now));
        assert!(!is_fresh(stale, now));
        // A future timestamp is fresh: a clock that moved backwards must not
        // delete a tile that was just written.
        assert!(is_fresh(now + Duration::from_secs(60), now));
    }

    #[test]
    fn the_prune_plan_drops_the_oldest_files_first() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let entries = vec![
            (PathBuf::from("new"), 40, at(300)),
            (PathBuf::from("old"), 40, at(100)),
            (PathBuf::from("middle"), 40, at(200)),
        ];
        // 120 bytes against a 41-byte limit: deleting the oldest leaves 80,
        // which is still over, so the middle one goes too (leaving 40).
        let plan = prune_plan(entries, 41);
        assert_eq!(plan, vec![PathBuf::from("old"), PathBuf::from("middle")]);
        // Against 80 the oldest is enough: 120 - 40 is exactly the limit.
        let entries = vec![
            (PathBuf::from("new"), 40, at(300)),
            (PathBuf::from("old"), 40, at(100)),
            (PathBuf::from("middle"), 40, at(200)),
        ];
        assert_eq!(prune_plan(entries, 80), vec![PathBuf::from("old")]);
        // Under the limit: nothing is planned.
        let entries = vec![(PathBuf::from("small"), 10, at(100))];
        assert!(prune_plan(entries, 80).is_empty());
    }

    #[test]
    fn a_fresh_cache_hit_is_decoded_without_the_network() {
        let scratch = Scratch::new("fresh-hit");
        let cache = CachePaths::from_root(scratch.path());
        let tile = TileId::new(13, 7276, 3226);
        let path = cache.tile_path(tile);
        std::fs::create_dir_all(path.parent().expect("tile path has a parent"))
            .expect("cache directory");
        std::fs::write(&path, synthetic_png()).expect("cache file");
        let fetcher = FakeFetcher::ok();
        let (image, written) = load_tile(&fetcher, Some(&cache), tile, SystemTime::now());
        assert!(image.is_ok(), "a fresh cache entry did not decode");
        assert_eq!(written, 0);
        assert_eq!(fetcher.calls(), 0, "the fetcher was called for a cache hit");
    }

    #[test]
    fn a_stale_cache_entry_is_refetched_and_rewritten() {
        let scratch = Scratch::new("stale-refetch");
        let cache = CachePaths::from_root(scratch.path());
        let tile = TileId::new(13, 0, 0);
        let path = cache.tile_path(tile);
        std::fs::create_dir_all(path.parent().expect("tile path has a parent"))
            .expect("cache directory");
        std::fs::write(&path, synthetic_png()).expect("cache file");
        // Judge it from a moment 31 days after it was written.
        let now = std::time::SystemTime::now() + Duration::from_secs(31 * 24 * 60 * 60);
        let fetcher = FakeFetcher::ok();
        let (image, written) = load_tile(&fetcher, Some(&cache), tile, now);
        assert!(image.is_ok());
        assert_eq!(fetcher.calls(), 1, "a stale tile was not refetched");
        assert!(written > 0, "the refetched tile was not rewritten");
    }

    #[test]
    fn a_failing_fetch_falls_back_to_a_stale_cache_entry() {
        let scratch = Scratch::new("offline-fallback");
        let cache = CachePaths::from_root(scratch.path());
        let tile = TileId::new(13, 1, 1);
        let path = cache.tile_path(tile);
        std::fs::create_dir_all(path.parent().expect("tile path has a parent"))
            .expect("cache directory");
        std::fs::write(&path, synthetic_png()).expect("cache file");
        let fetcher = FakeFetcher::failing();
        let now = std::time::SystemTime::now() + Duration::from_secs(31 * 24 * 60 * 60);
        let (image, _written) = load_tile(&fetcher, Some(&cache), tile, now);
        assert!(image.is_ok(), "the stale tile was not used while offline");
    }

    #[test]
    fn a_failing_fetch_without_a_cache_is_an_error() {
        let scratch = Scratch::new("offline-empty");
        let cache = CachePaths::from_root(scratch.path());
        let fetcher = FakeFetcher::failing();
        let (image, written) = load_tile(
            &fetcher,
            Some(&cache),
            TileId::new(13, 2, 2),
            SystemTime::now(),
        );
        assert!(matches!(image, Err(TileError::Offline(_))));
        assert_eq!(written, 0);
    }

    #[test]
    fn a_fetched_tile_becomes_a_texture() {
        // The worker thread, the channel and the texture store, with a fake
        // fetcher: the network is replaced, everything else is the real path.
        let scratch = Scratch::new("texture");
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(Some(CachePaths::from_root(scratch.path())));
        let context = egui::Context::default();
        let view = MapView::new(
            TOKYO.0,
            TOKYO.1,
            13.0,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0)),
        );
        let wanted = view.visible_tiles();
        for _ in 0..200 {
            tiles.request(&wanted, &context);
            tiles.pump(&context);
            if tiles.textures.len() >= wanted.len() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !tiles.textures.is_empty(),
            "a successful fetch produced no texture"
        );
        assert!(fetcher.calls() > 0, "the fake fetcher was never called");
        // The drawn state is now the tile state, not the offline fallback.
        let options = MapOptions {
            enabled: true,
            sample: false,
        };
        assert_eq!(tiles.background(options), Background::Tiles);
    }

    #[test]
    fn a_failing_fetcher_switches_the_map_to_offline() {
        let scratch = Scratch::new("offline-state");
        let fetcher = Arc::new(FakeFetcher::failing());
        let mut tiles = MapTiles::with_fetcher(fetcher);
        tiles.set_cache(Some(CachePaths::from_root(scratch.path())));
        let context = egui::Context::default();
        let view = MapView::new(
            TOKYO.0,
            TOKYO.1,
            13.0,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 120.0)),
        );
        let wanted: Vec<TileId> = view.visible_tiles().into_iter().take(3).collect();
        tiles.request(&wanted, &context);
        for _ in 0..200 {
            tiles.pump(&context);
            if tiles.offline_until.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let options = MapOptions {
            enabled: true,
            sample: false,
        };
        assert_eq!(tiles.background(options), Background::Offline);
        assert_eq!(
            Background::Offline.note(),
            Some("地図タイルを取得できません（オフライン）")
        );
    }

    #[test]
    fn a_sample_screen_never_fetches_or_reports_tiles() {
        let mut tiles = MapTiles::with_fetcher(Arc::new(FakeFetcher::ok()));
        assert_eq!(
            tiles.background(MapOptions {
                enabled: true,
                sample: true
            }),
            Background::Sample
        );
        assert_eq!(
            Background::Sample.note(),
            Some("見本：地図タイルは表示しません")
        );
        assert_eq!(
            MapTiles::default().background(MapOptions {
                enabled: false,
                sample: false
            }),
            Background::Disabled
        );
    }

    #[test]
    fn the_attribution_is_the_text_the_licence_requires() {
        assert_eq!(ATTRIBUTION, "© OpenStreetMap contributors");
        assert!(ATTRIBUTION_URL.contains("openstreetmap.org"));
        assert!(
            USER_AGENT.starts_with("m241-gui/"),
            "the User-Agent must start with the product name: {USER_AGENT}"
        );
        assert!(
            USER_AGENT.contains("github.com/haruki0420/holux-m241-rs"),
            "the User-Agent must name the repository: {USER_AGENT}"
        );
    }

    #[test]
    fn the_tile_url_is_the_osm_template() {
        assert_eq!(
            tile_url(TileId::new(13, 7276, 3226)),
            "https://tile.openstreetmap.org/13/7276/3226.png"
        );
    }

    #[test]
    fn the_cache_figure_is_a_size_and_its_limit() {
        assert_eq!(CacheUsage::default().label(), "0 B（未使用）");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(12_288), "12.0 KB");
        assert_eq!(human_bytes(4_194_304), "4.0 MB");
        let usage = CacheUsage {
            files: 2,
            bytes: 12_288,
        };
        assert!(usage.label().contains("12.0 KB"));
        assert!(usage.label().contains("200.0 MB"));
    }

    #[test]
    fn a_body_limit_error_maps_to_too_large_not_offline() {
        // `ureq` reports the configured body limit as `BodyExceedsLimit`. The
        // old code mapped every read error to `Offline`, which was both wrong
        // and made `TileError::TooLarge` unreachable: the check after the read
        // could never fire.
        assert_eq!(
            map_body_error(ureq::Error::BodyExceedsLimit(MAX_TILE_BYTES + 1)),
            TileError::TooLarge(MAX_TILE_BYTES + 1)
        );
        assert!(matches!(
            map_body_error(ureq::Error::HostNotFound),
            TileError::Offline(_)
        ));
    }

    #[test]
    fn an_oversized_tile_is_too_large_and_is_not_cached() {
        // The oversized-body path must be distinguishable from an outage, and a
        // rejected body must not reach the disk: caching it would serve the same
        // failure for a month without asking the server again.
        let scratch = Scratch::new("too-large");
        let cache = CachePaths::from_root(scratch.path());
        let tile = TileId::new(13, 3, 3);
        let fetcher = FakeFetcher::too_large();
        let (image, written) = load_tile(&fetcher, Some(&cache), tile, SystemTime::now());
        assert!(
            matches!(image, Err(TileError::TooLarge(_))),
            "an oversized tile was not reported as TooLarge: {image:?}"
        );
        assert_eq!(written, 0, "an oversized tile was written to the cache");
        assert!(
            !cache.tile_path(tile).exists(),
            "an oversized tile left a cache file"
        );
    }

    #[test]
    fn a_png_larger_than_a_tile_is_refused_by_the_decoder_limits() {
        // The limits are installed on `ImageReader` before `decode`, so the
        // decoder refuses the width instead of allocating the bitmap first. The
        // old code decoded the whole image and then compared its dimensions.
        let wide = png_bytes(MAX_TILE_EDGE + 1, 1);
        assert!(
            matches!(decode_tile(&wide), Err(TileError::TooLarge(_))),
            "a PNG wider than a tile was decoded anyway"
        );
        // The ordinary tile still decodes.
        assert!(decode_tile(&synthetic_png()).is_ok());
    }

    #[test]
    fn a_failed_tile_is_retried_only_after_the_retry_window() {
        // A single failed visible tile used to stay failed for the whole session
        // (only the global offline latch cleared it). It now retries after the
        // same window as the offline latch, without waiting for a full outage.
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(None);
        let context = egui::Context::default();
        let view = MapView::new(
            TOKYO.0,
            TOKYO.1,
            13.0,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 120.0)),
        );
        let wanted = view.visible_tiles();
        let tile = wanted[0];
        // A fresh failure is honoured: it is not requested again immediately.
        let _fresh = tiles.failed.insert(tile, Instant::now());
        tiles.request(&wanted, &context);
        for _ in 0..30 {
            tiles.pump(&context);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !tiles.textures.contains_key(&tile),
            "a fresh failure was retried immediately"
        );

        // Once the window has passed, the next request tries the tile again.
        let _expired = tiles.failed.insert(
            tile,
            Instant::now() - OFFLINE_RETRY - Duration::from_secs(1),
        );
        for _ in 0..200 {
            tiles.request(&wanted, &context);
            tiles.pump(&context);
            if tiles.textures.contains_key(&tile) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            tiles.textures.contains_key(&tile),
            "an expired failure was never retried"
        );
        assert!(fetcher.calls() > 0, "the retry never reached the fetcher");
    }
}
