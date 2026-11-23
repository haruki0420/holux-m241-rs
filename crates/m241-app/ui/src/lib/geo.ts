import type { Bounds, PointDto } from "./ipc";

/** Points further apart in time than this start a new track line segment. */
export const SEGMENT_GAP_SECONDS = 600;

/** A position pair in GeoJSON order: `[longitude, latitude]`. */
export type Position = [number, number];

/**
 * Split a track into the segments the map draws.
 *
 * A device that was switched off between two sessions leaves a time gap; drawing
 * one line through it would invent a straight jump the logger never travelled.
 * The same rule applies at a point without a position. The line stays one layer,
 * but its geometry is a set of LineStrings rather than a single one.
 */
export function trackSegments(points: readonly PointDto[]): Position[][] {
  const segments: Position[][] = [];
  let current: Position[] = [];
  let previous: PointDto | null = null;
  for (const point of points) {
    if (point.lat === null || point.lon === null) {
      if (current.length > 1) {
        segments.push(current);
      }
      current = [];
      previous = null;
      continue;
    }
    const position: Position = [point.lon, point.lat];
    const timeGap =
      previous?.utc != null && point.utc != null
        ? point.utc - previous.utc
        : null;
    const broken =
      previous !== null &&
      (timeGap === null || timeGap > SEGMENT_GAP_SECONDS || timeGap < 0);
    if (broken && current.length > 1) {
      segments.push(current);
      current = [];
    }
    current.push(position);
    previous = point;
  }
  if (current.length > 1) {
    segments.push(current);
  }
  return segments;
}

/** The bounding box of the positioned points, or `null` for a track without any. */
export function boundsOf(points: readonly PointDto[]): Bounds | null {
  let bounds: Bounds | null = null;
  for (const point of points) {
    if (point.lat === null || point.lon === null) {
      continue;
    }
    bounds =
      bounds === null
        ? {
            west: point.lon,
            south: point.lat,
            east: point.lon,
            north: point.lat,
          }
        : {
            west: Math.min(bounds.west, point.lon),
            south: Math.min(bounds.south, point.lat),
            east: Math.max(bounds.east, point.lon),
            north: Math.max(bounds.north, point.lat),
          };
  }
  return bounds;
}

/** Metres per screen pixel in Web Mercator at `latitude` and `zoom`. */
export function metresPerPixel(latitude: number, zoom: number): number {
  return (156_543.033_92 * Math.cos((latitude * Math.PI) / 180)) / 2 ** zoom;
}

const CELL_DEGREES = 0.002;

/**
 * A coarse lat/lon bucket index over the track's positioned points.
 *
 * Hover and click both ask "which point is within 20 px of the cursor", which is
 * a distance test in screen space but a data question in degrees. Bucketing the
 * points once keeps a mouse move over a 10,000-point track from re-projecting
 * every point; the bucket size is about 220 m at this latitude, so a 20 px
 * window at zoom 12 (about 30 m) touches one or two buckets.
 */
export class PointLocator {
  private readonly buckets = new Map<string, number[]>();
  private readonly points: readonly PointDto[];

  constructor(points: readonly PointDto[]) {
    this.points = points;
    for (const point of points) {
      if (point.lat === null || point.lon === null) {
        continue;
      }
      const key = bucketKey(point.lat, point.lon);
      const bucket = this.buckets.get(key);
      if (bucket === undefined) {
        this.buckets.set(key, [point.index]);
      } else {
        bucket.push(point.index);
      }
    }
  }

  /**
   * The closest positioned point within `toleranceMetres` of `lat`/`lon`, or
   * `null`. The index is the point's index in the full track.
   */
  nearest(lat: number, lon: number, toleranceMetres: number): number | null {
    const latDegrees = toleranceMetres / 111_320;
    const cos = Math.max(Math.cos((lat * Math.PI) / 180), 0.01);
    const lonDegrees = latDegrees / cos;
    const latStart = Math.floor((lat - latDegrees) / CELL_DEGREES);
    const latEnd = Math.floor((lat + latDegrees) / CELL_DEGREES);
    const lonStart = Math.floor((lon - lonDegrees) / CELL_DEGREES);
    const lonEnd = Math.floor((lon + lonDegrees) / CELL_DEGREES);
    let best: number | null = null;
    let bestDistance = Infinity;
    for (let latCell = latStart; latCell <= latEnd; latCell += 1) {
      for (let lonCell = lonStart; lonCell <= lonEnd; lonCell += 1) {
        const bucket = this.buckets.get(`${latCell}:${lonCell}`);
        if (bucket === undefined) {
          continue;
        }
        for (const index of bucket) {
          const point = this.points[index];
          if (point === undefined || point.lat === null || point.lon === null) {
            continue;
          }
          const dLat = (point.lat - lat) * 111_320;
          const dLon = (point.lon - lon) * 111_320 * cos;
          const distance = Math.hypot(dLat, dLon);
          if (distance < bestDistance) {
            bestDistance = distance;
            best = point.index;
          }
        }
      }
    }
    return bestDistance <= toleranceMetres ? best : null;
  }
}

function bucketKey(lat: number, lon: number): string {
  return `${Math.floor(lat / CELL_DEGREES)}:${Math.floor(lon / CELL_DEGREES)}`;
}

/** The points a day row covers: its first and last index in the track. */
export function dayRange(
  points: readonly PointDto[],
  day: { firstUtc: number | null; lastUtc: number | null },
): { from: number; to: number } | null {
  if (day.firstUtc === null || day.lastUtc === null) {
    return null;
  }
  let from: number | null = null;
  let to: number | null = null;
  for (const point of points) {
    if (point.utc === null) {
      continue;
    }
    if (point.utc < day.firstUtc || point.utc > day.lastUtc) {
      continue;
    }
    from = from === null ? point.index : from;
    to = point.index;
  }
  if (from === null || to === null) {
    return null;
  }
  return { from, to };
}

/** The hours and minutes between two instants, for dense captions. */
export function elapsedLabel(from: number, to: number): string {
  const seconds = Math.max(0, Math.round(to - from));
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  if (hours > 0) {
    return `${hours}時間${minutes % 60}分`;
  }
  return `${minutes}分`;
}
