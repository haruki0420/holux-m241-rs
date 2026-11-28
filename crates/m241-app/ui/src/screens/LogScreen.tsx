import { FolderOpen, Sparkles, TriangleAlert, Usb } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { DadsBanner } from "../components/DadsBanner";
import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import type { LogController } from "../hooks/useLogDocument";
import { pickOpenFile, pickSaveFile } from "../lib/dialog";
import { Dock } from "./log/Dock";
import { LogToolbar } from "./log/LogToolbar";
import { MapCanvas } from "./log/MapCanvas";
import { Timeline } from "./log/Timeline";

const TILES_STORAGE_KEY = "m241.log.tiles";

/**
 * Tab 1: ログ (Log viewer, device download, MapLibre map, KPI summary, day table,
 * parser diagnostics disclosure, speed/altitude chart, track points table, export).
 */
export function LogScreen({
  log,
  device,
  onNavigateGeotag,
  onNavigateFirmware,
}: {
  log: LogController;
  device: DeviceProbeController;
  onNavigateGeotag: () => void;
  onNavigateFirmware: () => void;
}) {
  const [tiles, setTiles] = useState(() => {
    try {
      return window.localStorage.getItem(TILES_STORAGE_KEY) !== "off";
    } catch {
      return true;
    }
  });
  const [drawerOpen, setDrawerOpen] = useState(true);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    try {
      window.localStorage.setItem(TILES_STORAGE_KEY, tiles ? "on" : "off");
    } catch {
      // Ignore storage failures in restricted webviews.
    }
  }, [tiles]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          if (cancelled) {
            return;
          }
          const payload = event.payload;
          if (payload.type === "over") {
            setDragging(true);
          } else if (payload.type === "leave") {
            setDragging(false);
          } else if (payload.type === "drop") {
            setDragging(false);
            const path = payload.paths[0];
            if (path !== undefined) {
              log.openFile(path);
            }
          }
        });
      } catch {
        // Ignore when running outside Tauri webview.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [log.openFile]);

  const handleOpenFileDialog = useCallback(async () => {
    const selected = await pickOpenFile({
      title: "トラックログを開く",
      filters: [
        { name: "ログファイル", extensions: ["bin", "gpx", "kml", "kmz"] },
        { name: "すべて", extensions: ["*"] },
      ],
    });
    if (selected) {
      log.openFile(selected);
    }
  }, [log]);

  const handleDownloadDevice = useCallback(async () => {
    if (device.activePort === "") {
      await log.downloadFromDevice("", "m241_log.bin");
      return;
    }
    const stamp = new Date().toISOString().slice(0, 10);
    const destination = await pickSaveFile({
      title: "M-241 から取得したログの保存先を選択",
      defaultPath: `m241_log_${stamp}.bin`,
      filters: [{ name: "M-241 バイナリログ (.bin)", extensions: ["bin"] }],
    });
    if (destination) {
      await log.downloadFromDevice(device.activePort, destination);
    }
  }, [device.activePort, log]);

  const state = log.state;

  if (state.kind === "empty") {
    return (
      <LogStart
        log={log}
        activePort={device.activePort}
        dragging={dragging}
        onDownloadDevice={() => void handleDownloadDevice()}
        onOpenFileDialog={() => void handleOpenFileDialog()}
      />
    );
  }
  if (state.kind === "loading") {
    return <LogLoading label={state.label} />;
  }
  if (state.kind === "error") {
    return (
      <LogFailure
        log={log}
        activePort={device.activePort}
        message={state.message}
        hint={state.hint}
        dragging={dragging}
        onDownloadDevice={() => void handleDownloadDevice()}
        onOpenFileDialog={() => void handleOpenFileDialog()}
      />
    );
  }

  return (
    <div className="flex min-h-0 flex-1" data-dragging={dragging ? "true" : "false"}>
      <div className="relative flex min-h-0 min-w-0 flex-1">
        <MapCanvas
          summary={state.summary}
          points={log.points}
          photos={log.photos}
          selectedIndex={log.selectedIndex}
          popupPoint={log.popupPoint}
          follow={log.follow}
          playing={log.playing}
          tiles={tiles}
          fitTarget={log.fitTarget}
          onOpenPopup={log.openPopup}
          onClosePopup={log.closePopup}
          onToggleFollow={log.toggleFollow}
        />

        <LogToolbar
          log={log}
          activePort={device.activePort}
          tiles={tiles}
          drawerOpen={drawerOpen}
          onToggleTiles={() => setTiles((on) => !on)}
          onToggleDrawer={() => setDrawerOpen((open) => !open)}
          onDownloadDevice={() => void handleDownloadDevice()}
          onOpenFileDialog={() => void handleOpenFileDialog()}
        />

        {log.downloadError ? (
          <div className="pointer-events-auto absolute left-3 top-16 z-popover max-w-md shadow-md">
            <DadsBanner type="error" title="本体からの取得に失敗しました">
              <p className="font-bold">{log.downloadError.message}</p>
              {log.downloadError.hint ? (
                <p className="mt-1 text-[var(--dads-ink)]">{log.downloadError.hint}</p>
              ) : null}
            </DadsBanner>
          </div>
        ) : null}

        <Timeline
          points={log.points}
          selectedIndex={log.selectedIndex}
          playing={log.playing}
          onSeek={log.seek}
          onTogglePlay={log.togglePlaying}
        />
      </div>

      {drawerOpen ? (
        <Dock
          log={log}
          onNavigateGeotag={onNavigateGeotag}
          onNavigateFirmware={onNavigateFirmware}
        />
      ) : null}
    </div>
  );
}

function LogStart({
  log,
  activePort,
  dragging,
  onDownloadDevice,
  onOpenFileDialog,
}: {
  log: LogController;
  activePort: string;
  dragging: boolean;
  onDownloadDevice: () => void;
  onOpenFileDialog: () => void;
}) {
  const [manualPath, setManualPath] = useState("");

  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-y-auto bg-white">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <div className="relative mx-auto flex w-full max-w-4xl flex-1 flex-col justify-center gap-8 px-8 py-10 lg:flex-row lg:items-center lg:justify-between">
        <div className="w-full max-w-lg space-y-5 rounded-lg border border-[var(--dads-border-control)] bg-white p-6 shadow-md">
          <div>
            <h1 className="text-[20px] font-bold text-[var(--dads-ink)]">トラックログ</h1>
            <p className="mt-1 text-[13px] leading-relaxed text-[var(--dads-ink-sub)]">
              M-241 本体から直接ログを取得するか、保存済みのログファイルを開いて軌跡・日時・速度・診断情報を確認します。
            </p>
          </div>

          <div className="flex flex-wrap gap-2.5">
            <button
              type="button"
              className="dads-btn"
              data-variant="solid-fill"
              data-size="md"
              onClick={onDownloadDevice}
              disabled={log.downloading || activePort === ""}
              title={
                activePort !== ""
                  ? `${activePort} からログを取得`
                  : "上部ヘッダーでポートを選択または手動入力してください"
              }
            >
              <Usb size={16} strokeWidth={2} aria-hidden="true" />
              <span>{log.downloading ? "取得中…" : "本体から取得"}</span>
            </button>
            <button
              type="button"
              className="dads-btn"
              data-variant="outline"
              data-size="md"
              onClick={onOpenFileDialog}
            >
              <FolderOpen size={16} strokeWidth={2} aria-hidden="true" />
              <span>ファイルを開く</span>
            </button>
            <button
              type="button"
              className="dads-btn"
              data-variant="neutral"
              data-size="md"
              onClick={log.openSample}
            >
              <Sparkles size={16} strokeWidth={2} aria-hidden="true" />
              <span>サンプルを開く</span>
            </button>
          </div>

          {activePort === "" ? (
            <p className="text-[12px] text-[var(--dads-ink-meta)]">
              ※ 本体から取得する場合は、上部ヘッダーでシリアルポートを選択または手動入力してください。
            </p>
          ) : (
            <p className="text-[12px] text-[var(--dads-ink-sub)]">
              現在の取得対象ポート: <span className="mono font-bold">{activePort}</span>
            </p>
          )}

          {log.downloadError ? (
            <DadsBanner type="error" title="本体からの取得に失敗しました">
              <p className="font-bold">{log.downloadError.message}</p>
              {log.downloadError.hint ? (
                <p className="mt-1 text-[var(--dads-ink)]">{log.downloadError.hint}</p>
              ) : null}
            </DadsBanner>
          ) : null}

          <div className="border-t border-[var(--dads-border-divider)] pt-4 space-y-1.5">
            <label htmlFor="log-manual-path" className="block text-[12px] font-bold text-[var(--dads-ink-sub)]">
              パスを直接入力して開く（またはウィンドウへドロップ）
            </label>
            <div className="flex gap-2">
              <input
                id="log-manual-path"
                className="dads-field mono min-w-0 flex-1"
                placeholder="/path/to/track.bin"
                value={manualPath}
                onChange={(event) => setManualPath(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && manualPath.trim() !== "") {
                    log.openFile(manualPath);
                  }
                }}
              />
              <button
                type="button"
                className="dads-btn shrink-0"
                data-variant="outline"
                data-size="sm"
                disabled={manualPath.trim() === ""}
                onClick={() => log.openFile(manualPath)}
              >
                開く
              </button>
            </div>
            {dragging ? (
              <DadsBanner type="info" className="mt-2">
                ファイルを離すと読み込みます。
              </DadsBanner>
            ) : null}
          </div>
        </div>

        <aside className="w-full max-w-xs shrink-0 space-y-4 rounded-lg border border-[var(--dads-border-divider)] bg-white/95 p-5">
          <Fact label="対応形式">
            <span className="mono font-bold">.bin / .gpx / .kml / .kmz</span>
          </Fact>
          <Fact label="書き出し形式">
            <span className="mono">GPX / KML / KMZ / HTML / RTF / CSV / NMEA / JSON</span>
          </Fact>
          <Fact label="地図タイル">
            OpenStreetMap タイルは設定スイッチでいつでもオフにできます（オフ時はネットワーク通信なし）。
          </Fact>
        </aside>
      </div>
    </div>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <dt className="text-[12px] font-bold text-[var(--dads-ink-sub)]">{label}</dt>
      <dd className="mt-1 text-[13px] text-[var(--dads-ink)]">{children}</dd>
    </div>
  );
}

function LogLoading({ label }: { label: string }) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 bg-white"
      role="status"
      aria-busy="true"
    >
      <div aria-hidden="true" className="flex w-44 items-center gap-1">
        <span className="h-px flex-1 bg-[var(--dads-border-divider)]" />
        <span className="h-1 w-12 rounded-full bg-[var(--dads-blue)]" />
        <span className="h-px flex-1 bg-[var(--dads-border-divider)]" />
      </div>
      <p className="text-[14px] font-bold text-[var(--dads-ink)]">読み込み中…</p>
      <p className="mono max-w-sm truncate text-[12px] text-[var(--dads-ink-sub)]" title={label}>
        {label}
      </p>
    </div>
  );
}

function LogFailure({
  log,
  activePort,
  message,
  hint,
  dragging,
  onDownloadDevice,
  onOpenFileDialog,
}: {
  log: LogController;
  activePort: string;
  message: string;
  hint: string | null;
  dragging: boolean;
  onDownloadDevice: () => void;
  onOpenFileDialog: () => void;
}) {
  return (
    <div className="relative flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto bg-white px-6 py-10">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <div className="relative w-full max-w-lg space-y-4 rounded-lg border-2 border-[var(--dads-ink)] bg-white p-6 shadow-lg">
        <div className="flex items-center gap-2 text-[16px] font-bold text-[var(--dads-error)]">
          <TriangleAlert size={18} strokeWidth={2} aria-hidden="true" />
          <span>ログを読み込めませんでした</span>
        </div>

        <DadsBanner type="error" title="エラー詳細">
          <p className="font-bold">{message}</p>
          {hint ? <p className="mt-1 text-[var(--dads-ink)]">{hint}</p> : null}
        </DadsBanner>

        {log.downloadError && log.downloadError.message !== message ? (
          <DadsBanner type="error" title="本体からの取得エラー">
            <p className="font-bold">{log.downloadError.message}</p>
            {log.downloadError.hint ? (
              <p className="mt-1 text-[var(--dads-ink)]">{log.downloadError.hint}</p>
            ) : null}
          </DadsBanner>
        ) : null}

        {dragging ? (
          <DadsBanner type="info">ファイルを離すと読み込みます。</DadsBanner>
        ) : null}

        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className="dads-btn"
            data-variant="solid-fill"
            data-size="sm"
            disabled={log.downloading || activePort === ""}
            title={
              activePort !== ""
                ? `${activePort} からログを取得`
                : "上部ヘッダーでポートを選択または手動入力してください"
            }
            onClick={onDownloadDevice}
          >
            本体から再取得
          </button>
          <button
            type="button"
            className="dads-btn"
            data-variant="outline"
            data-size="sm"
            onClick={onOpenFileDialog}
          >
            別のファイルを開く
          </button>
          <button
            type="button"
            className="dads-btn"
            data-variant="neutral"
            data-size="sm"
            onClick={log.openSample}
          >
            サンプルを開く
          </button>
          <button
            type="button"
            className="dads-btn"
            data-variant="text"
            data-size="sm"
            onClick={log.close}
          >
            最初に戻る
          </button>
        </div>
      </div>
    </div>
  );
}
