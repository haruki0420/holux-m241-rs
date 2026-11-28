const decimal = new Intl.NumberFormat("ja-JP");
const oneDecimal = new Intl.NumberFormat("ja-JP", {
  minimumFractionDigits: 1,
  maximumFractionDigits: 1,
});

/** `10399` → `10,399` (figures are grouped everywhere, including byte counts). */
export function formatCount(value: number): string {
  return decimal.format(value);
}

/** A byte count with human-readable unit. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }
  if (bytes < 1024) {
    return `${formatCount(bytes)} B`;
  }
  const kib = bytes / 1024;
  if (kib < 1024) {
    return `${oneDecimal.format(kib)} KiB`;
  }
  const mib = kib / 1024;
  return `${oneDecimal.format(mib)} MiB`;
}

/** `true` → `有効`, `false` → `停止中`, `null` → `未取得`. */
export function loggingLabel(value: boolean | null): string {
  if (value === null) {
    return "未取得";
  }
  return value ? "有効" : "停止中";
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/** The UTC date and time of a Unix second, in `YYYY-MM-DD HH:MM:SS` style. */
export function formatUtcDateTime(seconds: number): string {
  const date = new Date(seconds * 1000);
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(
    date.getUTCDate(),
  )} ${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(date.getUTCSeconds())}`;
}

/** The UTC clock alone: `09:32:00`. */
export function formatUtcTime(seconds: number): string {
  const date = new Date(seconds * 1000);
  return `${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(
    date.getUTCSeconds(),
  )}`;
}

/** The UTC date alone: `2025-11-16`. */
export function formatUtcDate(seconds: number): string {
  const date = new Date(seconds * 1000);
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}`;
}

/** `2025-11-16` → `11月16日`. */
export function formatShortDate(isoDate: string): string {
  const parts = isoDate.split("-");
  const month = Number(parts[1] ?? "1");
  const day = Number(parts[2] ?? "1");
  return `${month}月${day}日`;
}

/** A duration in seconds as `h:mm:ss`, or `m:ss` under an hour. */
export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) {
    return "—";
  }
  const total = Math.round(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  if (hours > 0) {
    return `${hours}:${pad(minutes)}:${pad(rest)}`;
  }
  return `${minutes}:${pad(rest)}`;
}

/** A distance in metres as `2.45 km` above a kilometre, `840 m` below. */
export function formatDistance(metres: number | null): string {
  if (metres === null || !Number.isFinite(metres) || metres < 0) {
    return "—";
  }
  if (metres < 1000) {
    return `${Math.round(metres)} m`;
  }
  const kilometres = metres / 1000;
  return `${kilometres < 10 ? oneDecimal.format(kilometres) : Math.round(kilometres)} km`;
}

/** `35.681236, 139.767125`, signed and at the device's own precision. */
export function formatLatLon(lat: number | null, lon: number | null): string {
  if (lat === null || lon === null || !Number.isFinite(lat) || !Number.isFinite(lon)) {
    return "—";
  }
  return `${lat.toFixed(6)}, ${lon.toFixed(6)}`;
}

/** A height in metres, rounded: `42 m`. */
export function formatHeight(metres: number | null): string {
  if (metres === null || !Number.isFinite(metres)) {
    return "—";
  }
  return `${Math.round(metres)} m`;
}

/** A speed with one decimal: `18.4 km/h`. */
export function formatSpeed(kmh: number | null): string {
  if (kmh === null || !Number.isFinite(kmh)) {
    return "—";
  }
  return `${oneDecimal.format(kmh)} km/h`;
}

/** A bare zoom level for the map's readout: `12.4`. */
export function formatZoom(zoom: number): string {
  return oneDecimal.format(zoom);
}

/** Format a byte offset as uppercase hex string without risking static opcode patterns. */
export function formatHexOffset(offset: number): string {
  return `0x${offset.toString(16).toUpperCase()}`;
}

/** Last path segment for compact file display. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Human-readable gap in seconds (`±12秒`, `±3分10秒`). */
export function formatGapSeconds(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) {
    return "—";
  }
  const abs = Math.abs(Math.round(seconds));
  if (abs < 60) {
    return `±${abs}秒`;
  }
  const mins = Math.floor(abs / 60);
  const secs = abs % 60;
  return secs === 0 ? `±${mins}分` : `±${mins}分${secs}秒`;
}
