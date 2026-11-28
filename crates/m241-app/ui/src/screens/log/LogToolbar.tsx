import { Download, FolderOpen, PanelRightClose, PanelRightOpen, Sparkles, Usb, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { pickSaveFile } from "../../lib/dialog";
import { formatCount, formatDistance, formatDuration } from "../../lib/format";
import type { LogExportFormat } from "../../lib/ipc";

const EXPORT_FORMATS: ReadonlyArray<{ id: LogExportFormat; label: string; desc: string }> = [
  { id: "gpx", label: "GPX", desc: "汎用トラック (.gpx)" },
  { id: "kml", label: "KML", desc: "Google Earth (.kml)" },
  { id: "kmz", label: "KMZ", desc: "写真同梱アーカイブ (.kmz)" },
  { id: "html", label: "HTML", desc: "単体レポート (.html)" },
  { id: "rtf", label: "RTF", desc: "文書レポート (.rtf)" },
  { id: "csv", label: "CSV", desc: "表計算データ (.csv)" },
  { id: "nmea", label: "NMEA", desc: "NMEA 文 (.nmea)" },
  { id: "json", label: "JSON", desc: "JSON データ (.json)" },
];

/**
 * Floating top action bar inside the Log screen.
 */
export function LogToolbar({
  log,
  activePort,
  tiles,
  drawerOpen,
  onToggleTiles,
  onToggleDrawer,
  onDownloadDevice,
  onOpenFileDialog,
}: {
  log: LogController;
  activePort: string;
  tiles: boolean;
  drawerOpen: boolean;
  onToggleTiles: () => void;
  onToggleDrawer: () => void;
  onDownloadDevice: () => void;
  onOpenFileDialog: () => void;
}) {
  const summary = log.summary;
  const [exportOpen, setExportOpen] = useState(false);
  const [format, setFormat] = useState<LogExportFormat>("gpx");
  const exportRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!exportOpen) {
      return;
    }
    const handleOutside = (event: MouseEvent) => {
      if (exportRef.current && !exportRef.current.contains(event.target as Node)) {
        setExportOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setExportOpen(false);
      }
    };
    document.addEventListener("mousedown", handleOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [exportOpen]);

  const handleSaveExport = async () => {
    if (!summary) return;
    const baseName = summary.name.replace(/\.[^./\\]+$/, "") || "m241_track";
    const destination = await pickSaveFile({
      title: `${format.toUpperCase()} として書き出す`,
      defaultPath: `${baseName}.${format}`,
      filters: [{ name: format.toUpperCase(), extensions: [format] }],
    });
    if (!destination) {
      return;
    }
    await log.writeExport(format, destination);
  };

  return (
    <div className="pointer-events-none absolute inset-x-3 top-3 z-popover flex flex-wrap items-center justify-between gap-2">
      {/* Left action cluster */}
      <div className="pointer-events-auto flex flex-wrap items-center gap-1.5 rounded-lg border border-[var(--dads-border-control)] bg-white/95 p-1.5 shadow-md backdrop-blur">
        <button
          type="button"
          className="dads-btn"
          data-variant="solid-fill"
          data-size="sm"
          disabled={log.downloading || activePort === ""}
          onClick={onDownloadDevice}
          title={
            activePort !== ""
              ? `${activePort} からログを取得`
              : "上部ヘッダーでポートを選択または手動入力してください"
          }
        >
          <Usb size={14} strokeWidth={2} aria-hidden="true" />
          <span>{log.downloading ? "取得中…" : "本体から取得"}</span>
        </button>

        <button
          type="button"
          className="dads-btn"
          data-variant="outline"
          data-size="sm"
          onClick={onOpenFileDialog}
        >
          <FolderOpen size={14} strokeWidth={2} aria-hidden="true" />
          <span>ファイルを開く</span>
        </button>

        <button
          type="button"
          className="dads-btn"
          data-variant="neutral"
          data-size="sm"
          onClick={log.openSample}
        >
          <Sparkles size={14} strokeWidth={2} aria-hidden="true" />
          <span>サンプルを開く</span>
        </button>

        {summary ? (
          <div className="relative" ref={exportRef}>
            <button
              type="button"
              className="dads-btn"
              data-variant="neutral"
              data-size="sm"
              aria-expanded={exportOpen}
              aria-haspopup="dialog"
              onClick={() => setExportOpen((open) => !open)}
            >
              <Download size={14} strokeWidth={2} aria-hidden="true" />
              <span>エクスポート ▾</span>
            </button>

            {exportOpen ? (
              <div
                role="dialog"
                aria-label="エクスポート形式の選択"
                className="absolute left-0 mt-2 w-72 space-y-3 rounded-lg border-2 border-[var(--dads-ink)] bg-white p-3.5 text-[13px] shadow-xl z-50"
              >
                <div className="font-bold text-[14px]">エクスポート形式を選択</div>
                <div className="max-h-56 space-y-1.5 overflow-y-auto pr-1">
                  {EXPORT_FORMATS.map((entry) => (
                    <label
                      key={entry.id}
                      className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 hover:bg-[var(--dads-surface-50)]"
                    >
                      <input
                        type="radio"
                        name="log-export-fmt"
                        value={entry.id}
                        checked={format === entry.id}
                        onChange={() => setFormat(entry.id)}
                      />
                      <span className="font-bold">{entry.label}</span>
                      <span className="text-[12px] text-[var(--dads-ink-sub)]">{entry.desc}</span>
                    </label>
                  ))}
                </div>
                <button
                  type="button"
                  className="dads-btn w-full"
                  data-variant="solid-fill"
                  data-size="sm"
                  disabled={log.exporting}
                  onClick={() => void handleSaveExport()}
                >
                  {log.exporting ? "書き出し中…" : "保存先を選んで書き出す"}
                </button>
                {log.exportOutcome ? (
                  <p
                    role="status"
                    className="rounded border border-[var(--dads-success)] bg-[var(--dads-success-bg)] p-2 text-[12px] font-bold text-[var(--dads-success)] break-all"
                  >
                    保存完了: {log.exportOutcome.destination} ({formatCount(log.exportOutcome.bytes)} B)
                  </p>
                ) : null}
                {log.exportError ? (
                  <p
                    role="alert"
                    className="rounded border border-[var(--dads-error)] bg-[var(--dads-error-bg)] p-2 text-[12px] whitespace-pre-line text-[var(--dads-error)]"
                  >
                    {log.exportError}
                  </p>
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}

        <button
          type="button"
          role="switch"
          aria-checked={tiles}
          className="dads-btn"
          data-variant="neutral"
          data-size="sm"
          onClick={onToggleTiles}
        >
          <span>地図タイル: {tiles ? "オン" : "オフ"}</span>
        </button>
      </div>

      {/* Right summary chip & drawer toggle */}
      {summary ? (
        <div className="pointer-events-auto flex items-center gap-1.5 rounded-lg border border-[var(--dads-border-control)] bg-white/95 p-1.5 shadow-md backdrop-blur">
          <div className="hidden items-center gap-2 px-2 text-[12px] xl:flex">
            <span className="max-w-44 truncate font-bold text-[var(--dads-ink)]" title={summary.name}>
              {summary.name}
            </span>
            {summary.sample ? (
              <span className="rounded border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-1.5 py-0.5 text-[11px] font-bold">
                サンプル
              </span>
            ) : null}
            <span className="mono text-[var(--dads-ink-sub)]">
              {formatCount(summary.points)} 点 · {formatDistance(summary.distanceM)} ·{" "}
              {formatDuration(summary.durationSeconds)}
            </span>
          </div>

          <button
            type="button"
            className="dads-btn"
            data-variant={drawerOpen ? "solid-fill" : "outline"}
            data-size="sm"
            onClick={onToggleDrawer}
            aria-expanded={drawerOpen}
          >
            {drawerOpen ? (
              <PanelRightClose size={14} strokeWidth={2} aria-hidden="true" />
            ) : (
              <PanelRightOpen size={14} strokeWidth={2} aria-hidden="true" />
            )}
            <span>詳細パネル</span>
          </button>

          <button
            type="button"
            className="dads-btn"
            data-variant="neutral"
            data-size="sm"
            onClick={log.close}
            aria-label="ログを閉じる"
            title="ログを閉じる"
          >
            <X size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      ) : null}
    </div>
  );
}
