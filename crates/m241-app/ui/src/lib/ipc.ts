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

/** The summary `open_log` / `open_sample` / `download_log` returns. */
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

/** Outcome of `download_log`. */
export type DownloadLogOutcome = {
  destination: string;
  bytes: number;
  summary: LogSummary;
};

/** One point as the map, chart and table draw it. */
export type PointDto = {
  index: number;
  utc: number | null;
  lat: number | null;
  lon: number | null;
  heightM: number | null;
  speedKmh: number | null;
  waypoint: boolean;
};

/** Options for `load_photos`. */
export type PhotoOptionsDto = {
  cameraOffsetMinutes: number;
  logOffsetDays: number;
  groupDistanceMeters: number;
  maxGapSeconds: number;
};

/** One photo matched to the track. */
export type PhotoDto = {
  path: string;
  name: string;
  capturedAt: number | null;
  timeSource: "exif" | "file" | string | null;
  pointIndex: number | null;
  lat: number | null;
  lon: number | null;
  thumbnail: string | null;
  /** Cluster id when distance grouping is on; `null` when it is off. */
  groupId: number | null;
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
  | "kmz"
  | "html"
  | "rtf"
  | "csv"
  | "nmea"
  | "json";

/** What `export_log` returns. */
export type ExportOutcome = {
  destination: string;
  format: string;
  bytes: number;
};

/** Options for `geotag_preview` and `geotag_apply`. */
export type GeotagOptionsDto = {
  cameraOffsetMinutes: number;
  logOffsetDays: number;
  groupDistanceMeters: number;
  maxGapSeconds: number;
  keepBackup: boolean;
  rewriteDatetimeOriginal: boolean;
};

/** One item in `geotag_preview`. */
export type GeotagItemDto = {
  path: string;
  name: string;
  capturedAt: number | null;
  timeSource: string | null;
  pointIndex: number | null;
  lat: number | null;
  lon: number | null;
  gapSeconds: number | null;
  groupId: number | null;
  thumbnail: string | null;
  skippedReason: string | null;
};

/** Result of `geotag_preview`. */
export type GeotagPreviewDto = {
  directory: string;
  matched: number;
  skipped: number;
  items: GeotagItemDto[];
};

/** Result of `geotag_apply`. */
export type GeotagApplyOutcome = {
  written: number;
  skipped: number;
  backupsCreated: number;
  errors: string[];
};

/** Result of `fw_dump`. */
export type FwDumpOutcome = {
  destination: string;
  bytes: number;
  sha256: string;
  profile: string | null;
  wnroPatched: boolean | null;
};

/** Result of `fw_patch`. */
export type FwPatchOutcome = {
  outputPath: string;
  bytes: number;
  sha256: string;
  changedOffsets: number[];
};

/** Firmware flash mode. */
export type FlashMode = "strict" | "recover" | "vendor" | "unverified";

/** Result of `fw_inspect`. */
export type FwInspectDto = {
  imageBytes: number;
  imageSha256: string;
  backupBytes: number | null;
  backupSha256: string | null;
  profile: string | null;
  wnroPatched: boolean | null;
  diffCount: number | null;
  outsidePatchOffsets: boolean | null;
  armVectorOk: boolean;
  isVendorV113: boolean;
  isVendorV112: boolean;
  recommendedMode: FlashMode;
  warnings: string[];
};

/** Result of `fw_probe_da`. */
export type FwProbeDaOutcome = {
  port: string;
  flashId: string;
  flashManufacturer: number;
  flashDevice: number;
  flashSizeBytes: number;
  daVersion: string;
};

/** Result of `fw_flash`. */
export type FwFlashOutcome = {
  port: string;
  mode: string;
  bytesWritten: number;
  imageSha256: string;
  verifiedReadBack: boolean;
  restoredPerUnitBytes: number;
  elapsedMs: number;
};

/** `list_ports` — read-only enumeration; never opens a port. */
export function listPorts(): Promise<PortInfo[]> {
  return invoke<PortInfo[]>("list_ports");
}

/** `device_info` — read-only device probe at NMEA baud. */
export function readDeviceInfo(port: string): Promise<DeviceInfo> {
  return invoke<DeviceInfo>("device_info", { port });
}

/** `download_log` — fetch binary track log from device to destination file. */
export function downloadLog(
  port: string,
  destination: string,
): Promise<DownloadLogOutcome> {
  return invoke<DownloadLogOutcome>("download_log", { port, destination });
}

/** `open_log` — read a saved document from an absolute path. */
export function openLog(path: string): Promise<LogSummary> {
  return invoke<LogSummary>("open_log", { path });
}

/** `open_sample` — load the built-in synthetic track. */
export function openSample(): Promise<LogSummary> {
  return invoke<LogSummary>("open_sample");
}

/** `log_points` — the `[from, to)` slice of the currently loaded track. */
export function logPoints(from: number, to: number): Promise<PointDto[]> {
  return invoke<PointDto[]>("log_points", { from, to });
}

/** `load_photos` — read a folder and match its photos to the loaded track. */
export function loadPhotos(
  directory: string,
  options: PhotoOptionsDto,
): Promise<PhotoLoad> {
  return invoke<PhotoLoad>("load_photos", { directory, options });
}

/** `export_log` — write the currently loaded track to an absolute destination. */
export function exportLog(
  destination: string,
  format: string,
): Promise<ExportOutcome> {
  return invoke<ExportOutcome>("export_log", { destination, format });
}

/** `geotag_preview` — preview photo geotagging against the loaded track. */
export function geotagPreview(
  directory: string,
  options: GeotagOptionsDto,
): Promise<GeotagPreviewDto> {
  return invoke<GeotagPreviewDto>("geotag_preview", { directory, options });
}

/** `geotag_apply` — write GPS EXIF tags to matched photos. */
export function geotagApply(
  directory: string,
  options: GeotagOptionsDto,
): Promise<GeotagApplyOutcome> {
  return invoke<GeotagApplyOutcome>("geotag_apply", { directory, options });
}

/** `fw_dump` — read device firmware backup to destination file. */
export function fwDump(
  port: string,
  destination: string,
  assumeBrom: boolean,
): Promise<FwDumpOutcome> {
  return invoke<FwDumpOutcome>("fw_dump", { port, destination, assumeBrom });
}

/** `fw_patch` — apply WNRO rollover patch to backup firmware image. */
export function fwPatch(
  inputPath: string,
  outputPath: string,
): Promise<FwPatchOutcome> {
  return invoke<FwPatchOutcome>("fw_patch", { inputPath, outputPath });
}

/** `fw_inspect` — inspect firmware image and optional backup before flashing. */
export function fwInspect(
  imagePath: string,
  backupPath: string | null,
): Promise<FwInspectDto> {
  return invoke<FwInspectDto>("fw_inspect", { imagePath, backupPath });
}

/** `fw_probe_da` — verify BootROM + Download Agent handshake with PROBE confirmation. */
export function fwProbeDa(
  port: string,
  daPath: string,
  assumeBrom: boolean,
  confirmWord: string,
): Promise<FwProbeDaOutcome> {
  return invoke<FwProbeDaOutcome>("fw_probe_da", {
    port,
    daPath,
    assumeBrom,
    confirmWord,
  });
}

/** `fw_flash` — write firmware image to device flash with WRITE confirmation. */
export function fwFlash(params: {
  port: string;
  imagePath: string;
  backupPath: string;
  daPath: string;
  mode: FlashMode;
  assumeBrom: boolean;
  confirmWord: string;
}): Promise<FwFlashOutcome> {
  return invoke<FwFlashOutcome>("fw_flash", params);
}

/**
 * Split the two-line error the Rust side returns.
 *
 * Line 1 is the device's or OS's own message; line 2 is `対処: <next action>`.
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
