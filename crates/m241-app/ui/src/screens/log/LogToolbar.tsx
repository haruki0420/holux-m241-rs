import { Download, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { formatDistance, formatDuration, formatCount } from "../../lib/format";
import type { LogExportFormat } from "../../lib/ipc";

/** The formats `export_log` accepts, in the order the menu lists them. */
const FORMATS: ReadonlyArray<{ id: LogExportFormat; label: string }> = [
  { id: "gpx", label: "GPX" },
  { id: "kml", label: "KML" },
  { id: "csv", label: "CSV" },
  { id: "nmea", label: "NMEA" },
  { id: "json", label: "JSON" },
  { id: "kmz", label: "KMZ" },
  { id: "html", label: "HTML" },
  { id: "rtf", label: "RTF" },
];

/**
 * The strip above the map: which record is open, how big it is, and the two
 * switches that change the view. The export menu lives here because a track is
 * something the operator takes away, not a screen of its own.
 */
export function LogToolbar({
  log,
  tiles,
  onToggleTiles,
}: {
  log: LogController;
  tiles: boolean;
  onToggleTiles: () => void;
}) {
  const summary = log.summary;
  const [exportOpen, setExportOpen] = useState(false);
  const exportRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!exportOpen) {
      return;
    }
    const close = (event: MouseEvent) => {
      if (exportRef.current !== null && !exportRef.current.contains(event.target as Node)) {
        setExportOpen(false);
      }
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setExportOpen(false);
      }
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [exportOpen]);

  if (summary === null) {
    return null;
  }

  return (
    <div className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-hair bg-card px-4">
      <div className="flex min-w-0 items-center gap-3">
        <h1 className="min-w-0 truncate text-sm font-medium text-ink" title={summary.name}>
          {summary.name}
        </h1>
        {summary.sample ? <span className="tag shrink-0">見本</span> : null}
        <p className="mono hidden min-w-0 truncate text-xs text-ink-3 md:block">
          {formatCount(summary.points)} 点 · {formatDistance(summary.distanceM)} ·{" "}
          {formatDuration(summary.durationSeconds)}
        </p>
      </div>

      <div className="flex shrink-0 items-center gap-1">
        <button
          type="button"
          role="switch"
          aria-checked={tiles}
          aria-label="地図タイル"
          onClick={onToggleTiles}
          className="btn-quiet"
        >
          <span aria-hidden="true" className="switch" data-on={tiles ? "true" : "false"}>
            <span className="switch-knob" />
          </span>
          <span>地図タイル</span>
          <span className="mono text-[11px] text-ink-3">{tiles ? "オン" : "オフ"}</span>
        </button>

        <div className="relative" ref={exportRef}>
          <button
            type="button"
            className="btn-quiet"
            aria-expanded={exportOpen}
            aria-haspopup="dialog"
            onClick={() => setExportOpen((open) => !open)}
          >
            <Download size={14} strokeWidth={1.5} aria-hidden="true" />
            書き出し
          </button>
          {exportOpen ? <ExportMenu log={log} onDone={() => setExportOpen(false)} /> : null}
        </div>

        <button
          type="button"
          aria-label="ログを閉じる"
          title="ログを閉じる"
          className="btn-quiet"
          onClick={log.close}
        >
          <X size={14} strokeWidth={1.5} aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}

function ExportMenu({ log, onDone }: { log: LogController; onDone: () => void }) {
  const summary = log.summary;
  const [format, setFormat] = useState<LogExportFormat>("gpx");
  const [destination, setDestination] = useState(() =>
    summary === null ? "" : swapExtension(summary.source, "gpx"),
  );

  if (summary === null) {
    return null;
  }

  return (
    <div
      role="dialog"
      aria-label="書き出し"
      className="export-menu z-popover"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          onDone();
        }
      }}
    >
      <p className="text-sm font-medium text-ink">書き出し</p>
      <p className="mt-1 text-xs text-ink-3">保存先は絶対パスで指定します</p>

      <label className="mt-3 block text-xs text-ink-3" htmlFor="export-format">
        形式
      </label>
      <select
        id="export-format"
        className="field mt-1 h-8 text-xs"
        value={format}
        onChange={(event) => {
          const next = FORMATS.find((entry) => entry.id === event.target.value);
          if (next !== undefined) {
            setFormat(next.id);
            if (summary.source !== "sample") {
              setDestination(swapExtension(summary.source, next.id));
            }
          }
        }}
      >
        {FORMATS.map((entry) => (
          <option key={entry.id} value={entry.id}>
            {entry.label}
          </option>
        ))}
      </select>

      <label className="mt-2 block text-xs text-ink-3" htmlFor="export-destination">
        保存先
      </label>
      <input
        id="export-destination"
        className="field mt-1 h-8 text-xs"
        value={destination}
        placeholder="/home/user/track.gpx"
        onChange={(event) => setDestination(event.target.value)}
      />

      <button
        type="button"
        className="btn btn-primary mt-3 h-8 w-full text-xs"
        disabled={log.exporting || destination.trim() === ""}
        onClick={() => {
          void log.writeExport(format, destination);
        }}
      >
        {log.exporting ? "書き出し中" : "書き出す"}
      </button>

      {log.exportOutcome === null ? null : (
        <p role="status" className="mt-2 break-all text-xs text-ink-2">
          {log.exportOutcome.destination} に {log.exportOutcome.bytes} B 書き出しました
        </p>
      )}
      {log.exportError === null ? null : (
        <p role="alert" className="mt-2 text-xs whitespace-pre-line text-error">
          {log.exportError}
        </p>
      )}
    </div>
  );
}

/** Replace a source's extension, keeping a path without one workable. */
function swapExtension(source: string, extension: string): string {
  if (source === "sample") {
    return "";
  }
  return source.replace(/\.[^./\\]+$/, "") + `.${extension}`;
}
