import { invoke } from "@tauri-apps/api/core";

/** One serial port, mirroring `m241_app::device::PortInfo`. */
export type PortInfo = {
  name: string;
  description: string | null;
  kind: "usb" | "pci" | "bluetooth" | "unknown" | string;
  likely: boolean;
};

/** The identity and log usage, mirroring `m241_app::device::DeviceInfo`. */
export type DeviceInfo = {
  port: string;
  release: string;
  build: string;
  model: string;
  sdk: string | null;
  knownRelease: boolean;
  logUsedBytes: number;
  logEnabled: boolean | null;
};

/** A geographic box, as MapLibre's `fitBounds` takes it. */
export type Bounds = {
  west: number;
  south: number;
  east: number;
  north: number;
};

/** One UTC day of the track. */
export type DaySummary = {
  date: string;
  points: number;
  fixes: number;
  distanceM: number;
  durationSeconds: number | null;
  firstUtc: number | null;
  lastUtc: number | null;
};

/** What the parser skipped, flattened. */
export type ReportDto = {
  blocks: number;
  emptyBlocks: number;
  damagedBlocks: number;
  records: number;
  badChecksums: number;
  truncatedRecords: number;
  journalItems: number;
  layoutMismatchedBlocks: number;
  unparsedBytes: number;
};

/** The summary `open_log` returns; it deliberately carries no point list. */
export type LogSummary = {
  source: string;
  name: string;
  format: string;
  sample: boolean;
  points: number;
  fixes: number;
  waypoints: number;
  startedAt: number | null;
  endedAt: number | null;
  bounds: Bounds | null;
  distanceM: number;
  durationSeconds: number | null;
  averageSpeedKmh: number | null;
  maxSpeedKmh: number | null;
  minAltitudeM: number | null;
  maxAltitudeM: number | null;
  days: DaySummary[];
  report: ReportDto;
};

/** One point as the map and the timeline draw it. */
export type PointDto = {
  index: number;
  utc: number | null;
  lat: number | null;
  lon: number | null;
  heightM: number | null;
  speedKmh: number | null;
  waypoint: boolean;
};

/** One photo matched to the track. */
export type PhotoDto = {
  path: string;
  name: string;
  capturedAt: number | null;
  timeSource: "exif" | "file" | null;
  pointIndex: number | null;
  lat: number | null;
  lon: number | null;
  thumbnail: string | null;
};

/** One video or other file beside the photos. */
export type MediaItemDto = {
  name: string;
  bytes: number;
  modifiedAt: number | null;
  kind: "video" | "other" | string;
};

/** What `load_photos` returns. */
export type PhotoLoad = {
  directory: string;
  photos: PhotoDto[];
  media: MediaItemDto[];
  skipped: string[];
};

/** The formats `export_log` can write. */
export type LogExportFormat =
  | "gpx"
  | "kml"
  | "csv"
  | "nmea"
  | "json"
  | "kmz"
  | "html"
  | "rtf";

/** What `export_log` returns. */
export type ExportOutcome = {
  destination: string;
  format: string;
  bytes: number;
};

/** `list_ports` — read-only enumeration; never opens a port. */
export function listPorts(): Promise<PortInfo[]> {
  return invoke<PortInfo[]>("list_ports");
}

/**
 * `device_info` — the one IPC round trip that touches hardware.
 *
 * The Rust side opens the port at the NMEA baud and sends only read requests.
 * Nothing in this module can write, erase or change a setting.
 */
export function readDeviceInfo(port: string): Promise<DeviceInfo> {
  return invoke<DeviceInfo>("device_info", { port });
}

/** `open_log` — read a saved document from an absolute path. */
export function openLog(path: string): Promise<LogSummary> {
  return invoke<LogSummary>("open_log", { path });
}

/** `open_sample` — load the built-in synthetic track. */
export function openSample(): Promise<LogSummary> {
  return invoke<LogSummary>("open_sample");
}

/** `log_points` — the `[from, to)` slice of the loaded track. */
export function logPoints(
  path: string,
  from: number,
  to: number,
): Promise<PointDto[]> {
  return invoke<PointDto[]>("log_points", { path, from, to });
}

/** `load_photos` — read a folder and match its photos to the loaded track. */
export function loadPhotos(path: string, directory: string): Promise<PhotoLoad> {
  return invoke<PhotoLoad>("load_photos", { path, directory });
}

/** `export_log` — write the loaded track to an absolute destination. */
export function exportLog(
  path: string,
  format: LogExportFormat,
  destination: string,
): Promise<ExportOutcome> {
  return invoke<ExportOutcome>("export_log", { path, format, destination });
}

/**
 * Split the two-line error the Rust side returns.
 *
 * Line 1 is the device's or OS's own message; line 2 is `対処: <next action>`.
 * The screen shows them in different places because they answer different
 * questions ("what happened" vs "what do I do").
 */
export function splitError(raw: unknown): { message: string; hint: string | null } {
  const text =
    typeof raw === "string" ? raw : raw instanceof Error ? raw.message : String(raw);
  const lines = text.split("\n");
  const message = lines[0] ?? text;
  const rest = lines.slice(1).join("\n").trim();
  if (rest === "") {
    return { message, hint: null };
  }
  const hint = rest.startsWith("対処: ") ? rest.slice("対処: ".length) : rest;
  return { message, hint };
}
