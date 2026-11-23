const decimal = new Intl.NumberFormat("ja-JP");
const oneDecimal = new Intl.NumberFormat("ja-JP", {
  minimumFractionDigits: 1,
  maximumFractionDigits: 1,
});

/** `10399` → `10,399` (figures are grouped everywhere, including byte counts). */
export function formatCount(value: number): string {
  return decimal.format(value);
}

/**
 * A byte count with the unit the operator expects.
 *
 * The card shows this string and keeps the exact byte count in the caption:
 * "207.9 KiB" is readable, "212,860 B" is checkable.
 */
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
