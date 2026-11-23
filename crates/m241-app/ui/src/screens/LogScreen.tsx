import { FolderOpen, TriangleAlert } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import type { LogController } from "../hooks/useLogDocument";
import { Dock } from "./log/Dock";
import { LogToolbar } from "./log/LogToolbar";
import { MapCanvas } from "./log/MapCanvas";
import { Timeline } from "./log/Timeline";

const TILES_STORAGE_KEY = "m241.log.tiles";

/**
 * The Log screen.
 *
 * The map is the screen; the right dock is the five panels that answer questions
 * about it (when, how fast, where the photos were taken). Before a document is
 * open the whole area is the invitation to open one, because five empty panels
 * would be five empty panels.
 */
export function LogScreen({ log }: { log: LogController }) {
  // Tiles are on by default (the README records the network use). The switch is
  // persisted so an offline operator turns them off once.
  const [tiles, setTiles] = useState(() => {
    try {
      return window.localStorage.getItem(TILES_STORAGE_KEY) !== "off";
    } catch {
      return true;
    }
  });
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    try {
      window.localStorage.setItem(TILES_STORAGE_KEY, tiles ? "on" : "off");
    } catch {
      // A webview without storage keeps the switch for this run only.
    }
  }, [tiles]);

  // Dropping a file anywhere on the screen opens it. The Tauri webview owns the
  // drag event; a browser preview has no bridge and simply gets no drop target.
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
        // Running outside Tauri (the browser preview); no drop target.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [log.openFile]);

  const state = log.state;

  if (state.kind === "empty") {
    return <LogStart log={log} dragging={dragging} />;
  }
  if (state.kind === "loading") {
    return <LogLoading label={state.label} />;
  }
  if (state.kind === "error") {
    return (
      <LogFailure
        log={log}
        message={state.message}
        hint={state.hint}
        dragging={dragging}
      />
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-dragging={dragging ? "true" : "false"}>
      <LogToolbar log={log} tiles={tiles} onToggleTiles={() => setTiles((on) => !on)} />
      <div className="flex min-h-0 flex-1">
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
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
          <Timeline
            points={log.points}
            selectedIndex={log.selectedIndex}
            playing={log.playing}
            onSeek={log.seek}
            onTogglePlay={log.togglePlaying}
          />
        </div>
        <Dock log={log} />
      </div>
    </div>
  );
}

/** The invitation shown before a document is open. */
function LogStart({ log, dragging }: { log: LogController; dragging: boolean }) {
  const [path, setPath] = useState("");
  const open = () => {
    log.openFile(path);
  };

  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <div className="relative mx-auto flex w-full max-w-4xl flex-1 flex-col justify-center gap-10 px-10 py-10 lg:flex-row lg:items-center lg:justify-between lg:gap-16">
        <div className="w-full max-w-md">
          <h1 className="text-xl font-medium text-ink">ログ</h1>
          <p className="mt-2 text-sm text-ink-2">
            吸い出した記録を開き、地図で日付・速度・写真を確かめます
          </p>

          <div className="mt-8">
            <label htmlFor="log-path" className="text-xs text-ink-3">
              ログファイル
            </label>
            <div className="mt-1 flex gap-2">
              <input
                id="log-path"
                className="field"
                placeholder="/home/user/log.bin"
                value={path}
                onChange={(event) => setPath(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    open();
                  }
                }}
              />
              <button
                type="button"
                className="btn btn-primary shrink-0"
                onClick={open}
                disabled={path.trim() === ""}
              >
                <FolderOpen size={16} strokeWidth={1.5} aria-hidden="true" />
                開く
              </button>
            </div>
            <p className="mt-2 text-xs text-ink-3">
              .bin / .gpx / .kml / .kmz · この画面にドロップしても開けます
            </p>
            {dragging ? (
              <p className="mt-2 text-xs font-medium text-accent">離すと開きます</p>
            ) : null}

            <button type="button" className="btn-quiet -ml-2 mt-3" onClick={log.openSample}>
              見本で試す
            </button>
          </div>
        </div>

        <aside className="w-full max-w-xs shrink-0 border-t border-hair pt-5 lg:border-l lg:border-t-0 lg:py-1 lg:pl-8">
          <dl className="flex flex-col gap-4">
            <Fact label="対応形式">
              <span className="mono">.bin / .gpx / .kml / .kmz</span>
            </Fact>
            <Fact label="読み取り専用">記録を書き換えません</Fact>
            <Fact label="地図タイル">
              既定でオン。オフにするとタイルを要求せず、軌跡だけを描きます
            </Fact>
          </dl>
        </aside>
      </div>
    </div>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <dt className="text-xs text-ink-3">{label}</dt>
      <dd className="mt-1 text-sm text-ink-2">{children}</dd>
    </div>
  );
}

/** The quiet loading state: a label and a static trail, no loop. */
function LogLoading({ label }: { label: string }) {
  return (
    <div
      className="relative flex min-h-0 flex-1 flex-col items-center justify-center gap-3"
      role="status"
      aria-busy="true"
    >
      <div aria-hidden="true" className="flex w-40 items-center gap-1">
        <span className="h-px flex-1 bg-hair" />
        <span className="h-0.5 w-10 bg-accent" />
        <span className="h-px flex-1 bg-hair" />
      </div>
      <p className="text-sm text-ink">読み込み中</p>
      <p className="mono max-w-xs truncate text-xs text-ink-3" title={label}>
        {label}
      </p>
    </div>
  );
}

/** The failure state: what happened, what to do, and the way back. */
function LogFailure({
  log,
  message,
  hint,
  dragging,
}: {
  log: LogController;
  message: string;
  hint: string | null;
  dragging: boolean;
}) {
  return (
    <div className="relative flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-8 py-10">
      <div aria-hidden="true" className="contour absolute inset-0" />
      <div className="relative w-full max-w-md" role="alert">
        <p className="flex items-center gap-2 text-sm font-medium text-ink">
          <TriangleAlert size={16} strokeWidth={1.5} aria-hidden="true" className="text-error" />
          ログを開けません
        </p>
        <div className="mt-3 rounded-control bg-error-wash p-3">
          <p className="text-xs text-ink-3">読み込みの結果</p>
          <p className="mt-1 text-sm break-words text-ink">{message}</p>
        </div>
        {hint === null ? null : (
          <div className="mt-3">
            <p className="text-xs text-ink-3">対処</p>
            <p className="mt-1 text-sm text-ink-2">{hint}</p>
          </div>
        )}
        {dragging ? (
          <p className="mt-3 text-xs font-medium text-accent">離すと開きます</p>
        ) : null}
        <div className="mt-5 flex gap-2">
          <button type="button" className="btn btn-primary" onClick={log.close}>
            別のログを開く
          </button>
          <button type="button" className="btn-quiet" onClick={log.openSample}>
            見本で試す
          </button>
        </div>
      </div>
    </div>
  );
}
