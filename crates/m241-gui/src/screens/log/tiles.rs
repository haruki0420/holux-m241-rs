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
//! * `--screenshot` never fetches (the screen passes `sample: true`), so a
//!   screenshot cannot pretend to have measured the network.
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
pub const TILE_SIZE: f64 = 256.0;

/// Latitude at which Web Mercator is truncated (the usual slippy-map bound).
pub const MERCATOR_LIMIT: f64 = 85.051_128_78;

/// Lowest zoom the map accepts.
pub const MIN_TILE_ZOOM: u8 = 1;

/// Highest zoom the map accepts (OSM serves 19).
pub const MAX_TILE_ZOOM: u8 = 19;

/// Requests allowed in flight at once. OSM's policy asks clients not to hammer
/// the tile servers; four is the number this module promises.
pub const MAX_CONCURRENT: usize = 4;

/// Largest tile body accepted. A PNG tile is tens of kilobytes; anything near
/// this bound is a server that is not answering with a tile.
pub const MAX_TILE_BYTES: u64 = 2 * 1024 * 1024;

/// How long a cached tile may be served without asking the server again.
pub const CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Upper bound on the cache on disk.
pub const CACHE_MAX_BYTES: u64 = 200 * 1024 * 1024;

/// Bytes written between cache-prune sweeps.
const PRUNE_STEP_BYTES: u64 = 64 * 1024 * 1024;

/// Most tiles one frame may ask for; a canvas larger than this is a bug, and
/// the cap keeps a malformed rectangle from queueing thousands of requests.
const MAX_VISIBLE_TILES: usize = 256;

/// Textures held in memory before the cache is dropped and re-requested.
const MAX_TEXTURES: usize = 256;

/// Consecutive failures that switch the map to its offline state.
const OFFLINE_FAILURES: usize = 3;

/// How long the offline state lasts before one tile is tried again.
const OFFLINE_RETRY: Duration = Duration::from_secs(60);

/// The attribution OSM's licence requires, verbatim.
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";

/// Where the attribution's licence lives, shown beside the credit.
pub const ATTRIBUTION_URL: &str = "openstreetmap.org/copyright";

/// The User-Agent the OSM tile usage policy requires: the tool and a way to
/// reach its maintainers.
pub const USER_AGENT: &str = concat!(
    "m241-gui/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/haruki0420/holux-m241-rs)"
);

/// One tile addressed the way a slippy map addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    /// Zoom level this tile belongs to (`1..=19`).
    pub zoom: u8,
    /// Column, counted west to east.
    pub x: u32,
    /// Row, counted north to south.
    pub y: u32,
}

impl TileId {
    /// The tile `x`, `y` at `zoom`.
    #[must_use]
    pub const fn new(zoom: u8, x: u32, y: u32) -> Self {
        Self { zoom, x, y }
    }

    /// The URL the tile is fetched from.
    #[must_use]
    pub fn url(self) -> String {
        tile_url(self)
    }
}

/// `https://tile.openstreetmap.org/{z}/{x}/{y}.png` for one tile.
///
/// The template is OSM's documented one, and it is a pure function so a test can
/// check the string without a socket.
#[must_use]
pub fn tile_url(tile: TileId) -> String {
    format!(
        "https://tile.openstreetmap.org/{}/{}/{}.png",
        tile.zoom, tile.x, tile.y
    )
}

/// World pixels per tile side at `zoom`.
#[must_use]
pub fn zoom_scale(zoom: f64) -> f64 {
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
#[must_use]
pub fn lonlat_to_tile_f64(latitude: f64, longitude: f64, zoom: u8) -> (f64, f64) {
    let count = 2.0_f64.powi(i32::from(zoom));
    (
        longitude_fraction(longitude) * count,
        mercator_fraction(latitude) * count,
    )
}

/// A fractional tile coordinate back to latitude/longitude.
#[must_use]
pub fn tile_to_lonlat(x: f64, y: f64, zoom: u8) -> (f64, f64) {
    let count = 2.0_f64.powi(i32::from(zoom));
    (latitude_from_fraction(y / count), x / count * 360.0 - 180.0)
}

/// The world-pixel origin of a tile at its own zoom.
#[must_use]
pub fn tile_pixel_origin(tile: TileId) -> (f64, f64) {
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
/// the tiles are drawn scaled by [`Self::tile_scale`] so a `+`/`−` press moves
/// the map continuously instead of jumping by a factor of two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    /// Latitude at the canvas centre, in degrees.
    pub center_latitude: f64,
    /// Longitude at the canvas centre, in degrees.
    pub center_longitude: f64,
    /// Fractional slippy-map zoom (`1.0..=19.0`).
    pub zoom: f64,
    /// The canvas the view is projected into.
    pub rect: Rect,
}

impl MapView {
    /// A view centred on a coordinate.
    #[must_use]
    pub fn new(center_latitude: f64, center_longitude: f64, zoom: f64, rect: Rect) -> Self {
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
    pub fn fit(rect: Rect, padding: f32, points: &[(f64, f64)]) -> Self {
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
    #[must_use]
    pub fn tile_zoom(self) -> u8 {
        let rounded = self.zoom.round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let zoom = rounded as i64;
        u8::try_from(zoom.clamp(i64::from(MIN_TILE_ZOOM), i64::from(MAX_TILE_ZOOM)))
            .unwrap_or(MIN_TILE_ZOOM)
    }

    /// How much a tile at [`Self::tile_zoom`] is scaled to reach this view.
    #[must_use]
    pub fn tile_scale(self) -> f64 {
        2.0_f64.powf(self.zoom - f64::from(self.tile_zoom()))
    }

    /// Project a coordinate into the canvas.
    #[must_use]
    pub fn project(self, latitude: f64, longitude: f64) -> Pos2 {
        let (x, y) = world_pixel(latitude, longitude, self.zoom);
        let (center_x, center_y) =
            world_pixel(self.center_latitude, self.center_longitude, self.zoom);
        self.rect.center() + Vec2::new((x - center_x) as f32, (y - center_y) as f32)
    }

    /// Where one tile lands on the canvas.
    #[must_use]
    pub fn tile_rect(self, tile: TileId) -> Rect {
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
    #[must_use]
    pub fn visible_tiles(self) -> Vec<TileId> {
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
pub struct CachePaths {
    /// The `m241-gui/tiles` directory.
    pub root: PathBuf,
}

impl CachePaths {
    /// The cache under the OS cache directory.
    ///
    /// `XDG_CACHE_HOME` (or `~/.cache`) on Linux, Known Folders on Windows and
    /// `Library/Caches` on macOS are all resolved by `dirs`, so no path in this
    /// module names a home directory.
    #[must_use]
    pub fn locate() -> Option<Self> {
        dirs::cache_dir().map(|base| Self::from_root(base.join("m241-gui").join("tiles")))
    }

    /// The cache under an explicit root, for tests.
    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// `{root}/{z}/{x}/{y}.png` for one tile.
    #[must_use]
    pub fn tile_path(&self, tile: TileId) -> PathBuf {
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
#[must_use]
pub fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map_or(true, |age| age <= CACHE_TTL)
}

/// What the cache holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheUsage {
    /// Number of cached tile files.
    pub files: usize,
    /// Their total size.
    pub bytes: u64,
}

impl CacheUsage {
    /// The figure the Settings row shows.
    #[must_use]
    pub fn label(self) -> String {
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
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
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
#[must_use]
pub fn cache_usage_at(root: &Path) -> CacheUsage {
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
#[must_use]
pub fn cache_usage() -> Option<CacheUsage> {
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
pub fn clear_cache() -> Result<(), String> {
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
#[must_use]
pub fn prune_plan(mut entries: Vec<CacheEntry>, limit: u64) -> Vec<PathBuf> {
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
pub trait TileFetcher: Send + Sync + std::fmt::Debug {
    /// Fetch one tile as encoded bytes (PNG, or JPEG if a server answers so).
    ///
    /// # Errors
    ///
    /// [`TileError`] names why the tile could not be fetched.
    fn fetch(&self, tile: TileId) -> Result<Vec<u8>, TileError>;
}

/// Why a tile could not be fetched or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TileError {
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
pub struct HttpTiles {
    /// Built on the first fetch, so creating a screen costs no TLS setup.
    agent: OnceLock<Result<ureq::Agent, String>>,
}

impl std::fmt::Debug for HttpTiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HttpTiles").finish_non_exhaustive()
    }
}

impl HttpTiles {
    fn agent(&self) -> Result<&ureq::Agent, TileError> {
        let built = self.agent.get_or_init(|| {
            Ok(ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .user_agent(USER_AGENT)
                .build()
                .new_agent())
        });
        built
            .as_ref()
            .map_err(|reason| TileError::Offline(reason.clone()))
    }
}

impl TileFetcher for HttpTiles {
    fn fetch(&self, tile: TileId) -> Result<Vec<u8>, TileError> {
        let agent = self.agent()?;
        let mut response = agent
            .get(&tile_url(tile))
            .call()
            .map_err(|error| match error {
                ureq::Error::StatusCode(code) => TileError::Status(code),
                other => TileError::Offline(other.to_string()),
            })?;
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_TILE_BYTES)
            .read_to_vec()
            .map_err(|error| TileError::Offline(error.to_string()))?;
        let size = u64::try_from(body.len()).unwrap_or(MAX_TILE_BYTES);
        if size > MAX_TILE_BYTES {
            return Err(TileError::TooLarge(size));
        }
        Ok(body)
    }
}

/// Decode tile bytes into a texture-ready image.
///
/// The size is bounded because the image ends up in a GPU texture: a server that
/// answers with a 10000×10000 PNG must not be able to allocate that here.
fn decode_tile(bytes: &[u8]) -> Result<ColorImage, TileError> {
    const MAX_EDGE: u32 = 2048;
    let decoded =
        image::load_from_memory(bytes).map_err(|error| TileError::Decode(error.to_string()))?;
    if decoded.width() > MAX_EDGE || decoded.height() > MAX_EDGE {
        return Err(TileError::Decode(format!(
            "{}×{} is larger than a tile",
            decoded.width(),
            decoded.height()
        )));
    }
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
pub enum Background {
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
}

impl Background {
    /// Whether this state should keep requesting tiles.
    #[must_use]
    pub fn may_fetch(self) -> bool {
        matches!(self, Self::Tiles | Self::Loading)
    }

    /// The caption the map shows for this state, if it needs one.
    #[must_use]
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Tiles => None,
            Self::Loading => Some("地図タイルを読み込み中"),
            Self::Offline => Some("地図タイルを取得できません（オフライン）"),
            Self::Disabled => Some("地図タイルはオフです（設定で切り替え）"),
            Self::Sample => Some("見本：地図タイルは表示しません"),
        }
    }
}

/// What the Log screen allows the map layer to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapOptions {
    /// The Settings screen's 「地図タイルを使う（要インターネット）」.
    pub enabled: bool,
    /// The screen is showing sample data (a screenshot must not fetch).
    pub sample: bool,
}

/// The tile service and the textures it has loaded.
pub struct MapTiles {
    fetcher: Arc<dyn TileFetcher>,
    cache: Option<CachePaths>,
    textures: HashMap<TileId, TextureHandle>,
    pending: HashSet<TileId>,
    failed: HashSet<TileId>,
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
    /// A tile service over an explicit fetcher (tests use a fake).
    #[must_use]
    pub fn with_fetcher(fetcher: Arc<dyn TileFetcher>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            fetcher,
            cache: CachePaths::locate(),
            textures: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
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

    /// The cache root, when the OS offers one (an explicit root otherwise).
    #[must_use]
    pub fn cache(&self) -> Option<&CachePaths> {
        self.cache.as_ref()
    }

    /// Replace the cache root (tests point this at a scratch directory).
    pub fn set_cache(&mut self, cache: Option<CachePaths>) {
        self.cache = cache;
        self.pruned = false;
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
            let _removed = self.pending.remove(&done.tile);
            match done.image {
                Ok(image) => {
                    let name = format!("osm/{}/{}/{}", done.tile.zoom, done.tile.x, done.tile.y);
                    let texture = context.load_texture(name, image, TextureOptions::LINEAR);
                    let _replaced = self.textures.insert(done.tile, texture);
                    self.failures = 0;
                }
                Err(_error) => {
                    let _inserted = self.failed.insert(done.tile);
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
        for tile in wanted {
            if self.textures.contains_key(tile)
                || self.pending.contains(tile)
                || self.failed.contains(tile)
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
                    let _inserted = self.failed.insert(tile);
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

/// Whether this context belongs to a window somebody is looking at.
///
/// `eframe` fills `inner_rect` before the first frame; a headless `RawInput` (the
/// screenshot renderer and every render test) leaves it `None`. The distinction
/// is what keeps the promise that a headless render never touches the network —
/// by construction, rather than by every caller remembering to disable tiles.
fn has_viewport(context: &egui::Context) -> bool {
    context.input(|input| input.viewport().inner_rect.is_some())
}

/// Draw the map's base layer: tiles when they are available, nothing when the
/// schematic grid underneath is the fallback.
///
/// The grid is drawn by the caller before this, so the offline canvas is never
/// empty. Returns the state the caller should describe in its zoom note.
pub fn draw_map(
    ui: &Ui,
    painter: &Painter,
    view: MapView,
    state: &mut MapTiles,
    options: MapOptions,
) -> Background {
    state.pump(ui.ctx());
    let visible = view.visible_tiles();
    let mut background = state.background(options);
    if background.may_fetch() && has_viewport(ui.ctx()) {
        state.request(&visible, ui.ctx());
        state.pump(ui.ctx());
        background = state.background(options);
    }
    for tile in &visible {
        if let Some(texture) = state.textures.get(tile) {
            painter.image(
                texture.id(),
                view.tile_rect(*tile),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    background
}

/// The attribution OSM requires, drawn over the map on every frame.
///
/// The plate is not decoration: OSM tiles are bright, and `theme::MUTE` on a
/// white field would fail the contrast the rest of the product holds itself to.
pub fn draw_attribution(painter: &Painter, rect: Rect) {
    let text = format!("{ATTRIBUTION} · {ATTRIBUTION_URL}");
    let galley =
        painter.layout_no_wrap(text, egui::FontId::proportional(size::CAPTION), theme::INK);
    let text_size = Vec2::new(galley.rect.width(), galley.rect.height());
    // Bottom-left: the zoom note owns the bottom-right corner, and two captions
    // in one corner would cover each other (the first screenshot showed exactly
    // that overlap).
    let origin = Pos2::new(
        rect.left() + space::S,
        rect.bottom() - space::S - text_size.y,
    );
    let plate = Rect::from_min_size(origin, text_size).expand(space::XS);
    painter.rect_filled(plate, theme::RADIUS, theme::RAIL.gamma_multiply(0.9));
    painter.galley(origin, galley, theme::INK);
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
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            let pixels = [
                255_u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ];
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

    #[test]
    fn a_headless_context_never_requests_a_tile() {
        // `--screenshot` and every render test use a context with no viewport.
        // Drawing there must not start a fetch, or a headless artifact would
        // depend on the network it is supposed to run without.
        let fetcher = Arc::new(FakeFetcher::ok());
        let mut tiles = MapTiles::with_fetcher(fetcher.clone());
        tiles.set_cache(None);
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
            ..egui::RawInput::default()
        };
        let mut background = Background::Loading;
        let output = context.run_ui(input, |ui| {
            let (rect, _response) =
                ui.allocate_exact_size(Vec2::new(400.0, 200.0), egui::Sense::hover());
            let painter = ui.painter();
            let view = MapView::new(TOKYO.0, TOKYO.1, 13.0, rect);
            background = draw_map(
                ui,
                painter,
                view,
                &mut tiles,
                MapOptions {
                    enabled: true,
                    sample: false,
                },
            );
        });
        output.drop_without_applying_deltas();
        assert_eq!(background, Background::Loading);
        std::thread::sleep(Duration::from_millis(20));
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
}
