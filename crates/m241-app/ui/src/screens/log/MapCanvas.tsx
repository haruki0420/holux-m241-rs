import * as maplibregl from "maplibre-gl";
import { Maximize, Minus, Plus, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";

import {
  boundsOf,
  metresPerPixel,
  PointLocator,
  trackSegments,
  type Position,
} from "../../lib/geo";
import {
  formatHeight,
  formatLatLon,
  formatSpeed,
  formatUtcDateTime,
  formatUtcTime,
  formatZoom,
} from "../../lib/format";
import type { Bounds, LogSummary, PhotoDto, PointDto } from "../../lib/ipc";

/** OSM's raster tile endpoint, used only while the tile switch is on. */
const OSM_TILES = ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"];
/** The mandatory OSM credit; the map shows it in MapLibre's attribution control. */
const OSM_ATTRIBUTION =
  '© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors';

/** Cartographic tokens, matching styles.css. MapLibre paint needs literals. */
const ACCENT = "#145f75";
const INK = "#171a1c";
const WHITE = "#ffffff";

/** How close, in screen pixels, a click or hover counts as "on the track". */
const HIT_PIXELS = 20;

/**
 * Room for the chrome that floats over the map: the 56 px header, the 48 px
 * toolbar below it, and the timeline at the bottom. `fitBounds` fits the track
 * into the uncovered rectangle, not the whole canvas, so nothing is framed
 * under the header.
 */
const FIT_PADDING = { top: 120, bottom: 72, left: 56, right: 56 };

/**
 * A quiet basemap: the track is the only saturated element on the map.
 *
 * OSM's default raster is full colour — pink motorways, green parks, coloured
 * POI icons — and at 5 px the accent track loses the "one light element"
 * contest (DESIGN.md: the map is a pale surface). Desaturating almost to grey
 * and capping the brightness keeps roads and labels readable while leaving the
 * accent as the only colour that means something.
 */
const RASTER_PAINT = {
  "raster-opacity": 0.9,
  "raster-saturation": -0.7,
  "raster-brightness-max": 0.85,
} as const;

type Basemap = { readonly tiles: boolean };

/** The map's empty style: either a raster OSM basemap or no network at all. */
function baseStyle({ tiles }: Basemap): NonNullable<Parameters<maplibregl.Map["setStyle"]>[0]> {
  return {
    version: 8,
    sources: tiles
      ? {
          osm: {
            type: "raster",
            tiles: OSM_TILES,
            tileSize: 256,
            maxzoom: 19,
            attribution: OSM_ATTRIBUTION,
          },
        }
      : {},
    layers: [
      {
        id: "background",
        type: "background",
        paint: { "background-color": "rgba(0,0,0,0)" },
      },
      ...(tiles
        ? [
            {
              id: "osm",
              type: "raster" as const,
              source: "osm",
              paint: { ...RASTER_PAINT },
            },
          ]
        : []),
    ],
  };
}

type Positioned = PointDto & { lat: number; lon: number };

/** A point that carries a position. The locator and the map both need one. */
function isPositioned(point: PointDto): point is Positioned {
  return point.lat !== null && point.lon !== null;
}

type Collection = {
  type: "FeatureCollection";
  features: Array<{
    type: "Feature";
    geometry: { type: "LineString"; coordinates: Position[] } | { type: "Point"; coordinates: Position };
    properties: Record<string, string | number>;
  }>;
};

function emptyCollection(): Collection {
  return { type: "FeatureCollection", features: [] };
}

function lineCollection(points: readonly PointDto[]): Collection {
  return {
    type: "FeatureCollection",
    features: trackSegments(points).map((coordinates) => ({
      type: "Feature",
      geometry: { type: "LineString", coordinates },
      properties: {},
    })),
  };
}

function dotCollection(
  entries: ReadonlyArray<{ point: PointDto; role?: string }>,
): Collection {
  return {
    type: "FeatureCollection",
    features: entries.filter((entry) => isPositioned(entry.point)).map((entry) => ({
      type: "Feature",
      geometry: {
        type: "Point",
        coordinates: [entry.point.lon as number, entry.point.lat as number] as Position,
      },
      properties: { role: entry.role ?? "point" },
    })),
  };
}

export function MapCanvas({
  summary,
  points,
  photos,
  selectedIndex,
  popupPoint,
  follow,
  playing,
  tiles,
  fitTarget,
  onOpenPopup,
  onClosePopup,
  onToggleFollow,
}: {
  summary: LogSummary;
  points: PointDto[];
  photos: PhotoDto[];
  selectedIndex: number | null;
  popupPoint: PointDto | null;
  follow: boolean;
  playing: boolean;
  tiles: boolean;
  fitTarget: { bounds: Bounds; key: number } | null;
  onOpenPopup: (index: number) => void;
  onClosePopup: () => void;
  onToggleFollow: () => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const pointMapRef = useRef(new Map<number, PointDto>());
  const locatorRef = useRef<PointLocator | null>(null);
  const popupIndexRef = useRef<number | null>(null);
  const popupCloseRef = useRef<HTMLButtonElement | null>(null);
  const pendingPopupFocusRef = useRef(false);
  const followRef = useRef(follow);
  const pointsRef = useRef<PointDto[]>(points);
  const playingRef = useRef(playing);
  const [ready, setReady] = useState(false);
  const [mapFailure, setMapFailure] = useState<string | null>(null);
  const [hover, setHover] = useState<{
    lat: number;
    lon: number;
    zoom: number;
    point: PointDto | null;
  } | null>(null);
  const [popupAt, setPopupAt] = useState<{ x: number; y: number } | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

  useEffect(() => {
    pointMapRef.current = new Map(points.map((point) => [point.index, point]));
    pointsRef.current = points;
    locatorRef.current = new PointLocator(points);
  }, [points]);

  useEffect(() => {
    followRef.current = follow;
  }, [follow]);

  useEffect(() => {
    playingRef.current = playing;
  }, [playing]);

  useEffect(() => {
    popupIndexRef.current = popupPoint?.index ?? null;
    pendingPopupFocusRef.current = popupPoint !== null;
  }, [popupPoint]);

  // A dialog that opens without moving focus leaves the keyboard on the map
  // behind it. The popup is positioned by `popupAt` one render later, so the
  // focus waits for the first placement; `pendingPopupFocusRef` keeps later
  // placements (every map move) from stealing the caret back.
  useEffect(() => {
    if (pendingPopupFocusRef.current && popupAt !== null) {
      pendingPopupFocusRef.current = false;
      popupCloseRef.current?.focus();
    }
  }, [popupAt]);

  const photoForPoint = useMemo(() => {
    const map = new Map<number, PhotoDto>();
    for (const photo of photos) {
      if (photo.pointIndex !== null && photo.thumbnail !== null) {
        map.set(photo.pointIndex, photo);
      }
    }
    return map;
  }, [photos]);

  const fitAll = useCallback(
    (duration: number) => {
      const map = mapRef.current;
      const bounds = boundsOf(pointsRef.current) ?? summary.bounds;
      if (map === null || bounds === null) {
        return;
      }
      map.fitBounds(
        [
          [bounds.west, bounds.south],
          [bounds.east, bounds.north],
        ],
        { padding: FIT_PADDING, maxZoom: 15, duration },
      );
    },
    [summary.bounds],
  );

  // Create the map once. The style is built from the initial tile switch; later
  // changes add or remove the raster source instead of rebuilding the style,
  // which would drop the track layers.
  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    let map: maplibregl.Map;
    try {
      map = new maplibregl.Map({
        container,
        style: baseStyle({ tiles }),
        center: summary.bounds === null ? [139.767125, 35.681236] : [
          (summary.bounds.west + summary.bounds.east) / 2,
          (summary.bounds.south + summary.bounds.north) / 2,
        ],
        zoom: 12,
        maxZoom: 19,
        minZoom: 1,
        attributionControl: false,
        dragRotate: false,
        pitchWithRotate: false,
        fadeDuration: 0,
      });
    } catch (error) {
      setMapFailure(error instanceof Error ? error.message : String(error));
      return;
    }
    mapRef.current = map;
    // The wrapper is the one keyboard surface (`role="application"` with the key
    // help in its label). MapLibre's canvas is a second tab stop with no visible
    // outline, so it is removed from the tab order and from the accessibility
    // tree; the wrapper speaks for the map.
    map.getCanvas().tabIndex = -1;
    map.getCanvas().setAttribute("aria-hidden", "true");
    map.addControl(new maplibregl.ScaleControl({ maxWidth: 110, unit: "metric" }), "bottom-right");
    // Not compact: the OSM credit has to be readable without a click.
    map.addControl(new maplibregl.AttributionControl({ compact: false }), "bottom-right");

    map.on("load", () => {
      map.addSource("track", {
        type: "geojson",
        data: lineCollection([]) as never,
      });
      map.addSource("photos", { type: "geojson", data: emptyCollection() as never });
      map.addSource("waypoints", { type: "geojson", data: emptyCollection() as never });
      map.addSource("ends", { type: "geojson", data: emptyCollection() as never });
      map.addSource("selected", { type: "geojson", data: emptyCollection() as never });

      const add = (layer: Parameters<maplibregl.Map["addLayer"]>[0]) => map.addLayer(layer);
      add({
        id: "track-casing",
        type: "line",
        source: "track",
        layout: { "line-join": "round", "line-cap": "round" },
        paint: { "line-color": WHITE, "line-width": 7, "line-opacity": 0.9 },
      });
      add({
        id: "track-line",
        type: "line",
        source: "track",
        layout: { "line-join": "round", "line-cap": "round" },
        paint: { "line-color": ACCENT, "line-width": 5, "line-opacity": 0.94 },
      });
      add({
        id: "photo-dot",
        type: "circle",
        source: "photos",
        paint: {
          "circle-radius": 5,
          "circle-color": WHITE,
          "circle-stroke-color": ACCENT,
          "circle-stroke-width": 2,
        },
      });
      add({
        id: "waypoint-dot",
        type: "circle",
        source: "waypoints",
        paint: {
          "circle-radius": 4,
          "circle-color": ACCENT,
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 1.5,
        },
      });
      add({
        id: "end-dot",
        type: "circle",
        source: "ends",
        paint: {
          "circle-radius": 4,
          "circle-color": ["match", ["get", "role"], "start", INK, ACCENT],
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 1.5,
        },
      });
      add({
        id: "selected-dot",
        type: "circle",
        source: "selected",
        paint: {
          "circle-radius": 6.5,
          "circle-color": ACCENT,
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 2.5,
        },
      });

      setReady(true);
    });

    const nearestAt = (lat: number, lon: number): number | null => {
      const locator = locatorRef.current;
      if (locator === null) {
        return null;
      }
      const tolerance = metresPerPixel(lat, map.getZoom()) * HIT_PIXELS;
      return locator.nearest(lat, lon, tolerance);
    };

    map.on("mousemove", (event) => {
      const { lat, lng } = event.lngLat;
      const index = nearestAt(lat, lng);
      const point = index === null ? null : (pointMapRef.current.get(index) ?? null);
      setHover({ lat, lon: lng, zoom: map.getZoom(), point });
      map.getCanvas().style.cursor = index === null ? "" : "pointer";
    });

    map.on("click", (event) => {
      const index = nearestAt(event.lngLat.lat, event.lngLat.lng);
      if (index === null) {
        onClosePopup();
      } else {
        onOpenPopup(index);
      }
    });

    const placePopup = () => {
      const index = popupIndexRef.current;
      const point = index === null ? null : pointMapRef.current.get(index);
      if (point === null || point === undefined || !isPositioned(point)) {
        setPopupAt(null);
        return;
      }
      const projected = map.project([point.lon, point.lat]);
      setPopupAt({ x: projected.x, y: projected.y });
    };
    map.on("move", placePopup);
    map.on("zoom", placePopup);
    map.on("resize", placePopup);
    placePopup();

    return () => {
      map.remove();
      mapRef.current = null;
      setReady(false);
    };
    // The map is created once per document; the effects below handle the data.
  }, [summary.source]);

  // Fit the whole track once the style has loaded, then leave the camera alone.
  //
  // The first selection (point 0, set when the document opens) also fires the
  // follow effect below. If it won, it would ease the camera to point 0 at zoom
  // 14 and the fit would be thrown away — the review saw the second session
  // clipped off the top. The fit therefore suppresses exactly one follow pass;
  // every later selection change still follows.
  const fittedRef = useRef<string | null>(null);
  const suppressFollowOnceRef = useRef(false);
  useEffect(() => {
    if (!ready || summary.source === fittedRef.current) {
      return;
    }
    fittedRef.current = summary.source;
    suppressFollowOnceRef.current = true;
    fitAll(0);
  }, [ready, summary.source, fitAll]);

  // Frame a day when the dock asks for it. The key makes two clicks on the same
  // day two requests, so the second one still moves the camera.
  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null || fitTarget === null) {
      return;
    }
    map.fitBounds(
      [
        [fitTarget.bounds.west, fitTarget.bounds.south],
        [fitTarget.bounds.east, fitTarget.bounds.north],
      ],
      { padding: FIT_PADDING, maxZoom: 16, duration: 420 },
    );
  }, [fitTarget, ready]);

  useEffect(() => {
    const map = mapRef.current;
    const source = map?.getSource("track");
    if (!ready || map === null || source === undefined) {
      return;
    }
    (source as maplibregl.GeoJSONSource).setData(lineCollection(points) as never);

    const positioned = points.filter(isPositioned);
    const waypoints = positioned.filter((point) => point.waypoint);
    const ends = [positioned[0], positioned[positioned.length - 1]]
      .filter((point): point is Positioned => point !== undefined)
      .map((point, index, all) => ({
        point,
        role: all.length > 1 && index === 0 ? "start" : "end",
      }));
    (map.getSource("waypoints") as maplibregl.GeoJSONSource | undefined)?.setData(
      dotCollection(waypoints.map((point) => ({ point }))) as never,
    );
    (map.getSource("ends") as maplibregl.GeoJSONSource | undefined)?.setData(
      dotCollection(ends) as never,
    );
  }, [points, ready]);

  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null) {
      return;
    }
    (map.getSource("photos") as maplibregl.GeoJSONSource | undefined)?.setData(
      dotCollection(
        photos
          .filter((photo) => photo.lat !== null && photo.lon !== null)
          .map((photo) => ({
            point: {
              index: photo.pointIndex ?? -1,
              utc: photo.capturedAt,
              lat: photo.lat,
              lon: photo.lon,
              heightM: null,
              speedKmh: null,
              waypoint: false,
            },
            role: "photo",
          })),
      ) as never,
    );
  }, [photos, ready]);

  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null) {
      return;
    }
    const selected =
      selectedIndex === null ? null : (pointMapRef.current.get(selectedIndex) ?? null);
    (map.getSource("selected") as maplibregl.GeoJSONSource | undefined)?.setData(
      dotCollection(selected === null ? [] : [{ point: selected }]) as never,
    );
  }, [selectedIndex, ready]);

  // Place the popup when it opens; the map's own move/zoom handler keeps it
  // anchored while the camera moves.
  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null) {
      return;
    }
    if (popupPoint === null || !isPositioned(popupPoint)) {
      setPopupAt(null);
      return;
    }
    const projected = map.project([popupPoint.lon, popupPoint.lat]);
    setPopupAt({ x: projected.x, y: projected.y });
  }, [popupPoint, ready]);

  // Keep the current point on screen while following. Playback jumps instead of
  // animating because a 300 ms ease per point would lag the cursor.
  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null || !follow || selectedIndex === null) {
      return;
    }
    if (suppressFollowOnceRef.current) {
      // The document was just fitted; this selection is the fit's own point 0,
      // not the operator asking to follow a point.
      suppressFollowOnceRef.current = false;
      return;
    }
    const point = pointMapRef.current.get(selectedIndex);
    if (point === undefined || !isPositioned(point)) {
      return;
    }
    map.easeTo({
      center: [point.lon, point.lat],
      zoom: Math.max(map.getZoom(), 14),
      duration: playingRef.current ? 0 : 280,
    });
  }, [selectedIndex, follow, ready]);

  // Add or remove the raster basemap without touching the track layers.
  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null) {
      return;
    }
    if (tiles) {
      if (map.getSource("osm") === undefined) {
        map.addSource("osm", {
          type: "raster",
          tiles: OSM_TILES,
          tileSize: 256,
          maxzoom: 19,
          attribution: OSM_ATTRIBUTION,
        } as Parameters<maplibregl.Map["addSource"]>[1]);
        if (map.getLayer("track-casing") !== undefined) {
          map.addLayer(
            {
              id: "osm",
              type: "raster",
              source: "osm",
              paint: { "raster-opacity": 0.9 },
            },
            "track-casing",
          );
        } else {
          map.addLayer({
            id: "osm",
            type: "raster",
            source: "osm",
            paint: { "raster-opacity": 0.9 },
          });
        }
      }
    } else {
      if (map.getLayer("osm") !== undefined) {
        map.removeLayer("osm");
      }
      if (map.getSource("osm") !== undefined) {
        map.removeSource("osm");
      }
    }
  }, [tiles, ready]);

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    const observer = new ResizeObserver(() => {
      setSize({ width: container.clientWidth, height: container.clientHeight });
      mapRef.current?.resize();
    });
    observer.observe(container);
    setSize({ width: container.clientWidth, height: container.clientHeight });
    return () => observer.disconnect();
  }, []);

  const onKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      const map = mapRef.current;
      if (map === null) {
        return;
      }
      const step = 120;
      switch (event.key) {
        case "ArrowLeft":
          map.panBy([step, 0], { duration: 120 });
          break;
        case "ArrowRight":
          map.panBy([-step, 0], { duration: 120 });
          break;
        case "ArrowUp":
          map.panBy([0, step], { duration: 120 });
          break;
        case "ArrowDown":
          map.panBy([0, -step], { duration: 120 });
          break;
        case "+":
        case "=":
          map.zoomIn({ duration: 160 });
          break;
        case "-":
        case "_":
          map.zoomOut({ duration: 160 });
          break;
        case "Escape":
          onClosePopup();
          break;
        default:
          return;
      }
      event.preventDefault();
    },
    [onClosePopup],
  );

  if (mapFailure !== null) {
    return <FallbackTrack points={points} bounds={summary.bounds} reason={mapFailure} />;
  }

  const popupPlacement = popupPlacementFor(popupAt, size, popupPoint !== null);
  const popupPhoto = popupPoint === null ? undefined : photoForPoint.get(popupPoint.index);

  return (
    <div className="relative min-h-0 flex-1 overflow-hidden bg-paper">
      <div
        aria-hidden="true"
        className="contour absolute inset-0"
        style={{ opacity: tiles ? 0.35 : 1 }}
      />
      <div
        ref={containerRef}
        role="application"
        aria-label="軌跡の地図。矢印キーで移動、+ と - で拡大縮小できます"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onMouseLeave={() => {
          setHover(null);
        }}
        className="h-full w-full"
        data-testid="log-map"
      />

      <div className="pointer-events-none absolute inset-0">
        <div className="pointer-events-auto absolute right-3 top-[120px] flex flex-col gap-1">
          <MapButton label="拡大" onClick={() => mapRef.current?.zoomIn({ duration: 160 })}>
            <Plus size={16} strokeWidth={1.5} aria-hidden="true" />
          </MapButton>
          <MapButton label="縮小" onClick={() => mapRef.current?.zoomOut({ duration: 160 })}>
            <Minus size={16} strokeWidth={1.5} aria-hidden="true" />
          </MapButton>
          <MapButton label="全点表示" onClick={() => fitAll(420)}>
            <Maximize size={16} strokeWidth={1.5} aria-hidden="true" />
          </MapButton>
          <button
            type="button"
            aria-pressed={follow}
            onClick={onToggleFollow}
            className="map-control"
            title="追従"
          >
            <span aria-hidden="true" className="size-1.5 rounded-full bg-current" />
            <span className="text-xs">追従</span>
          </button>
        </div>

        {hover === null ? null : (
          <div className="map-readout">
            <p className="mono">
              {hover.point === null
                ? formatLatLon(hover.lat, hover.lon)
                : formatLatLon(hover.point.lat, hover.point.lon)}
            </p>
            {hover.point === null || hover.point.utc === null ? null : (
              <p className="mono text-ink-2">
                {formatUtcTime(hover.point.utc)} UTC
                {hover.point.speedKmh === null ? "" : ` · ${formatSpeed(hover.point.speedKmh)}`}
              </p>
            )}
            <p className="mono text-ink-3">ズーム {formatZoom(hover.zoom)}</p>
          </div>
        )}
      </div>

      {popupPoint === null || popupPlacement === null ? null : (
        <div
          role="dialog"
          aria-label="選択した点"
          className="map-popup"
          style={popupPlacement.style}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              onClosePopup();
            }
          }}
        >
          <div className="flex items-start justify-between gap-3">
            <div>
              {popupPoint.utc === null ? null : (
                <p className="mono text-xs text-ink">{formatUtcDateTime(popupPoint.utc)} UTC</p>
              )}
              {popupPoint.waypoint ? <p className="mt-1 text-xs text-ink-3">目印</p> : null}
            </div>
            <button
              ref={popupCloseRef}
              type="button"
              onClick={onClosePopup}
              aria-label="閉じる"
              className="-mr-1 -mt-1 flex size-6 items-center justify-center rounded-tag text-ink-3 hover:bg-raise hover:text-ink"
            >
              <X size={16} strokeWidth={1.5} aria-hidden="true" />
            </button>
          </div>
          <p className="mono mt-2 text-xs text-ink-2">
            {formatLatLon(popupPoint.lat, popupPoint.lon)}
          </p>
          <div className="mt-2 grid grid-cols-2 gap-2 text-xs">
            <span className="text-ink-3">標高</span>
            <span className="mono text-right text-ink-2">{formatHeight(popupPoint.heightM)}</span>
            <span className="text-ink-3">速度</span>
            <span className="mono text-right text-ink-2">{formatSpeed(popupPoint.speedKmh)}</span>
          </div>
          {popupPhoto?.thumbnail == null ? null : (
            <div className="mt-2 flex items-center gap-2 border-t border-hair pt-2">
              <img
                src={popupPhoto.thumbnail}
                alt={popupPhoto.name}
                className="size-11 rounded-tag object-cover"
              />
              <p className="min-w-0 truncate text-xs text-ink-3">{popupPhoto.name}</p>
            </div>
          )}
        </div>
      )}

      <p className="pointer-events-none absolute bottom-3 left-3 max-w-[240px] text-xs text-ink-3">
        {ready ? null : "地図を準備しています"}
      </p>
    </div>
  );

}

function MapButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="map-control"
    >
      {children}
    </button>
  );
}

/** Where to put the popup so it stays inside the map panel. */
function popupPlacementFor(
  at: { x: number; y: number } | null,
  size: { width: number; height: number },
  hasPopup: boolean,
): { style: CSSProperties } | null {
  if (at === null || !hasPopup) {
    return null;
  }
  const half = 120;
  const x = Math.min(Math.max(at.x, half + 8), Math.max(size.width - half - 8, half + 8));
  const below = at.y < 210;
  const y = below
    ? Math.min(at.y + 14, Math.max(size.height - 24, 0))
    : Math.max(at.y - 14, 24);
  return {
    style: {
      left: `${x}px`,
      top: `${y}px`,
      transform: below ? "translate(-50%, 0)" : "translate(-50%, -100%)",
    },
  };
}

/**
 * The last resort when WebGL is unavailable: the same track drawn as SVG.
 *
 * The map is the screen's main surface, so a webview without WebGL should still
 * show where the walk went. This fallback has no tiles, no pan and no zoom; it
 * says why in one line.
 */
function FallbackTrack({
  points,
  bounds,
  reason,
}: {
  points: PointDto[];
  bounds: LogSummary["bounds"];
  reason: string;
}) {
  const box = boundsOf(points) ?? bounds;
  const width = 800;
  const height = 560;
  const positions: Position[] = points.filter(isPositioned).map((point) => [
    point.lon,
    point.lat,
  ]);
  const spanLon = Math.max((box?.east ?? 0) - (box?.west ?? 0), 1e-6);
  const spanLat = Math.max((box?.north ?? 0) - (box?.south ?? 0), 1e-6);
  const project = ([lon, lat]: Position): [number, number] => [
    24 + (((lon - (box?.west ?? 0)) / spanLon) * (width - 48) || 0),
    24 + ((((box?.north ?? 0) - lat) / spanLat) * (height - 48) || 0),
  ];
  const path = positions
    .map((position, index) => {
      const [x, y] = project(position);
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden bg-paper">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <svg
        role="img"
        aria-label="軌跡の概形"
        viewBox={`0 0 ${width} ${height}`}
        className="relative m-auto max-h-full w-full max-w-[900px] p-6"
      >
        <path
          d={path}
          fill="none"
          stroke={ACCENT}
          strokeWidth={2}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      </svg>
      <p className="relative m-3 text-xs text-ink-3">
        地図を表示できません（{reason}）。軌跡の概形のみ表示しています
      </p>
    </div>
  );
}
