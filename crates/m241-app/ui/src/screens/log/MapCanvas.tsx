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

/** OSM raster tile endpoint, used only when the tile switch is on. */
const OSM_TILES = ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"];
const OSM_ATTRIBUTION =
  '© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors';

/** DADS palette tokens for MapLibre paint expressions. */
const DADS_BLUE = "#1e4e8c";
const DADS_SUCCESS = "#197a4b";
const DADS_ERROR = "#ec0000";
const DADS_WARNING = "#c77700";
const WHITE = "#ffffff";

const HIT_PIXELS = 20;
const FIT_PADDING = { top: 72, bottom: 80, left: 56, right: 56 };

const RASTER_PAINT = {
  "raster-opacity": 0.9,
  "raster-saturation": -0.5,
  "raster-brightness-max": 0.9,
} as const;

type Basemap = { readonly tiles: boolean };

function baseStyle({
  tiles,
}: Basemap): NonNullable<Parameters<maplibregl.Map["setStyle"]>[0]> {
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

function isPositioned(point: PointDto): point is Positioned {
  return point.lat !== null && point.lon !== null;
}

type Collection = {
  type: "FeatureCollection";
  features: Array<{
    type: "Feature";
    geometry:
      | { type: "LineString"; coordinates: Position[] }
      | { type: "Point"; coordinates: Position };
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
    features: entries
      .filter((entry) => isPositioned(entry.point))
      .map((entry) => ({
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

  useEffect(() => {
    if (pendingPopupFocusRef.current && popupAt !== null) {
      pendingPopupFocusRef.current = false;
      popupCloseRef.current?.focus();
    }
  }, [popupAt]);

  const photoForPoint = useMemo(() => {
    const map = new Map<number, PhotoDto>();
    for (const photo of photos) {
      if (photo.pointIndex !== null) {
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
        center:
          summary.bounds === null
            ? [139.767125, 35.681236]
            : [
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
    map.getCanvas().tabIndex = -1;
    map.getCanvas().setAttribute("aria-hidden", "true");
    map.addControl(
      new maplibregl.ScaleControl({ maxWidth: 110, unit: "metric" }),
      "bottom-right",
    );
    map.addControl(
      new maplibregl.AttributionControl({ compact: false }),
      "bottom-right",
    );

    map.on("load", () => {
      map.addSource("track", {
        type: "geojson",
        data: lineCollection([]) as never,
      });
      map.addSource("photos", { type: "geojson", data: emptyCollection() as never });
      map.addSource("waypoints", { type: "geojson", data: emptyCollection() as never });
      map.addSource("ends", { type: "geojson", data: emptyCollection() as never });
      map.addSource("selected", { type: "geojson", data: emptyCollection() as never });

      const add = (layer: Parameters<maplibregl.Map["addLayer"]>[0]) =>
        map.addLayer(layer);
      add({
        id: "track-casing",
        type: "line",
        source: "track",
        layout: { "line-join": "round", "line-cap": "round" },
        paint: { "line-color": WHITE, "line-width": 7, "line-opacity": 0.92 },
      });
      add({
        id: "track-line",
        type: "line",
        source: "track",
        layout: { "line-join": "round", "line-cap": "round" },
        paint: { "line-color": DADS_BLUE, "line-width": 4.2, "line-opacity": 0.95 },
      });
      add({
        id: "waypoint-dot",
        type: "circle",
        source: "waypoints",
        paint: {
          "circle-radius": 4.5,
          "circle-color": DADS_BLUE,
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 1.5,
        },
      });
      add({
        id: "end-dot",
        type: "circle",
        source: "ends",
        paint: {
          "circle-radius": 5.5,
          "circle-color": ["match", ["get", "role"], "start", DADS_SUCCESS, DADS_ERROR],
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 2,
        },
      });
      add({
        id: "photo-dot",
        type: "circle",
        source: "photos",
        paint: {
          "circle-radius": 6.5,
          "circle-color": DADS_WARNING,
          "circle-stroke-color": WHITE,
          "circle-stroke-width": 2,
        },
      });
      add({
        id: "selected-dot",
        type: "circle",
        source: "selected",
        paint: {
          "circle-radius": 7,
          "circle-color": DADS_BLUE,
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
  }, [summary.source]);

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

  useEffect(() => {
    const map = mapRef.current;
    if (!ready || map === null || !follow || selectedIndex === null) {
      return;
    }
    if (suppressFollowOnceRef.current) {
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
              paint: { ...RASTER_PAINT },
            },
            "track-casing",
          );
        } else {
          map.addLayer({
            id: "osm",
            type: "raster",
            source: "osm",
            paint: { ...RASTER_PAINT },
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
    <div className="relative min-h-0 flex-1 overflow-hidden bg-[var(--dads-surface-50)]">
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
        <div className="pointer-events-auto absolute right-3 top-16 flex flex-col gap-1.5">
          <MapButton label="拡大" onClick={() => mapRef.current?.zoomIn({ duration: 160 })}>
            <Plus size={16} strokeWidth={2} aria-hidden="true" />
          </MapButton>
          <MapButton label="縮小" onClick={() => mapRef.current?.zoomOut({ duration: 160 })}>
            <Minus size={16} strokeWidth={2} aria-hidden="true" />
          </MapButton>
          <MapButton label="全体表示" onClick={() => fitAll(420)}>
            <Maximize size={15} strokeWidth={2} aria-hidden="true" />
          </MapButton>
          <button
            type="button"
            aria-pressed={follow}
            onClick={onToggleFollow}
            className="map-control"
            title="再生中の点に追従"
          >
            <span aria-hidden="true" className="size-2 rounded-full bg-current" />
            <span>追従</span>
          </button>
        </div>

        {hover === null ? null : (
          <div className="map-readout">
            <p className="mono font-bold text-[var(--dads-ink)]">
              {hover.point === null
                ? formatLatLon(hover.lat, hover.lon)
                : formatLatLon(hover.point.lat, hover.point.lon)}
            </p>
            {hover.point === null || hover.point.utc === null ? null : (
              <p className="mono text-[var(--dads-ink-sub)]">
                {formatUtcTime(hover.point.utc)} UTC
                {hover.point.speedKmh === null ? "" : ` · ${formatSpeed(hover.point.speedKmh)}`}
              </p>
            )}
            <p className="mono text-[var(--dads-ink-meta)]">ズーム {formatZoom(hover.zoom)}</p>
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
                <p className="mono text-xs font-bold text-[var(--dads-ink)]">
                  {formatUtcDateTime(popupPoint.utc)} UTC
                </p>
              )}
              {popupPoint.waypoint ? (
                <span className="mt-1 inline-block rounded border border-[var(--dads-blue)] bg-[var(--dads-blue-tint)] px-1.5 py-0.5 text-[11px] font-bold text-[var(--dads-blue)]">
                  ウェイポイント
                </span>
              ) : null}
            </div>
            <button
              ref={popupCloseRef}
              type="button"
              onClick={onClosePopup}
              aria-label="閉じる"
              className="-mr-1 -mt-1 flex size-6 items-center justify-center rounded text-[var(--dads-ink-sub)] hover:bg-[var(--dads-surface-50)] hover:text-[var(--dads-ink)] cursor-pointer"
            >
              <X size={16} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
          <p className="mono mt-2 text-xs text-[var(--dads-ink-sub)]">
            {formatLatLon(popupPoint.lat, popupPoint.lon)}
          </p>
          <div className="mt-2 grid grid-cols-2 gap-1.5 text-xs">
            <span className="text-[var(--dads-ink-meta)]">標高</span>
            <span className="mono text-right font-bold text-[var(--dads-ink)]">
              {formatHeight(popupPoint.heightM)}
            </span>
            <span className="text-[var(--dads-ink-meta)]">速度</span>
            <span className="mono text-right font-bold text-[var(--dads-ink)]">
              {formatSpeed(popupPoint.speedKmh)}
            </span>
          </div>
          {popupPhoto == null ? null : (
            <div className="mt-2 flex items-center gap-2 border-t border-[var(--dads-border-divider)] pt-2">
              {popupPhoto.thumbnail ? (
                <img
                  src={popupPhoto.thumbnail}
                  alt={popupPhoto.name}
                  className="size-11 rounded object-cover"
                />
              ) : null}
              <p className="min-w-0 truncate text-xs font-bold text-[var(--dads-ink)]">
                {popupPhoto.name}
              </p>
            </div>
          )}
        </div>
      )}

      {!tiles ? (
        <p className="pointer-events-none absolute bottom-20 left-3 rounded border border-[var(--dads-border-control)] bg-white/90 px-2.5 py-1 text-xs font-bold text-[var(--dads-ink-sub)]">
          地図タイル オフ（軌跡のみ表示）
        </p>
      ) : null}

      {!ready ? (
        <p className="pointer-events-none absolute bottom-20 left-3 text-xs text-[var(--dads-ink-meta)]">
          地図を準備しています…
        </p>
      ) : null}
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

function popupPlacementFor(
  at: { x: number; y: number } | null,
  size: { width: number; height: number },
  visible: boolean,
): { style: CSSProperties } | null {
  if (!visible || at === null) {
    return null;
  }
  const width = 248;
  const margin = 12;
  const clampedX = Math.min(
    Math.max(at.x, width / 2 + margin),
    Math.max(width / 2 + margin, size.width - width / 2 - margin),
  );
  const above = at.y > 190;
  return {
    style: {
      left: `${clampedX}px`,
      top: `${above ? at.y - 14 : at.y + 14}px`,
      transform: above ? "translate(-50%, -100%)" : "translate(-50%, 0)",
    },
  };
}

function FallbackTrack({
  points,
  bounds,
  reason,
}: {
  points: PointDto[];
  bounds: Bounds | null;
  reason: string;
}) {
  const effective = bounds ?? boundsOf(points);
  const paths = useMemo(() => {
    if (effective === null) {
      return [];
    }
    const lonSpan = Math.max(effective.east - effective.west, 1e-5);
    const latSpan = Math.max(effective.north - effective.south, 1e-5);
    const project = ([lon, lat]: Position): [number, number] => [
      24 + ((lon - effective.west) / lonSpan) * 552,
      24 + (1 - (lat - effective.south) / latSpan) * 352,
    ];
    return trackSegments(points).map((segment) =>
      segment
        .map((position, index) => {
          const [x, y] = project(position);
          return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
        })
        .join(" "),
    );
  }, [points, effective]);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col items-center justify-center bg-white p-6">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <svg
        viewBox="0 0 600 400"
        className="relative max-h-[70vh] w-full max-w-3xl"
        role="img"
        aria-label="軌跡の概略図"
      >
        {paths.map((d, index) => (
          <path
            key={index}
            d={d}
            fill="none"
            stroke={DADS_BLUE}
            strokeWidth={3}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        ))}
      </svg>
      <p className="relative mt-4 text-xs text-[var(--dads-ink-sub)]">
        地図描画を初期化できませんでした（{reason}）。軌跡の概略図を表示しています。
      </p>
    </div>
  );
}
