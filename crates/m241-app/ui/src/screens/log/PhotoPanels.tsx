import { Camera, ChevronLeft, ChevronRight, File, FileVideo, FolderOpen } from "lucide-react";
import { useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { pickDirectory } from "../../lib/dialog";
import { baseName, formatBytes, formatUtcDateTime } from "../../lib/format";

/**
 * Load photos onto the Log map (`load_photos`) and inspect matched pins & media.
 */
export function PhotoPanels({ log }: { log: LogController }) {
  const [directory, setDirectory] = useState(log.photoDirectory ?? "");
  const [cameraOffset, setCameraOffset] = useState(
    String(log.photoOptions.cameraOffsetMinutes),
  );
  const [logOffsetDays, setLogOffsetDays] = useState(
    String(log.photoOptions.logOffsetDays),
  );
  const [groupDistance, setGroupDistance] = useState(
    String(log.photoOptions.groupDistanceMeters),
  );
  const [maxGapSeconds, setMaxGapSeconds] = useState(
    String(log.photoOptions.maxGapSeconds),
  );

  const runLoad = async (targetDir: string) => {
    await log.readPhotoFolder(targetDir, {
      cameraOffsetMinutes: integerOr(cameraOffset, 540),
      logOffsetDays: integerOr(logOffsetDays, 0),
      groupDistanceMeters: Math.max(integerOr(groupDistance, 0), 0),
      maxGapSeconds: Math.max(integerOr(maxGapSeconds, 300), 1),
    });
  };

  const handleBrowse = async () => {
    const picked = await pickDirectory({ title: "写真フォルダを選択" });
    if (picked) {
      setDirectory(picked);
      await runLoad(picked);
    }
  };

  const activeIndex = log.photoIndex ?? 0;
  const activePhoto = log.photos[activeIndex] ?? null;

  return (
    <div className="space-y-4">
      <div className="space-y-2 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-3 text-[12px]">
        <div className="flex items-center justify-between gap-2">
          <span className="font-bold">地図へ写真ピンを読み込む</span>
          <button
            type="button"
            className="dads-btn"
            data-variant="outline"
            data-size="xs"
            onClick={() => void handleBrowse()}
            disabled={log.photoLoading}
          >
            <FolderOpen size={13} strokeWidth={2} aria-hidden="true" />
            <span>フォルダ選択</span>
          </button>
        </div>

        <input
          type="text"
          aria-label="写真フォルダパス"
          placeholder="/home/user/Pictures"
          value={directory}
          onChange={(event) => setDirectory(event.target.value)}
          className="dads-field mono w-full text-[12px]"
        />

        <div className="grid grid-cols-2 gap-2">
          <label className="space-y-1">
            <span className="block text-[11px] font-bold text-[var(--dads-ink-sub)]">
              カメラ時差 (分)
            </span>
            <input
              type="number"
              value={cameraOffset}
              onChange={(event) => setCameraOffset(event.target.value)}
              className="dads-field mono w-full text-[12px]"
            />
          </label>
          <label className="space-y-1">
            <span className="block text-[11px] font-bold text-[var(--dads-ink-sub)]">
              ログ日付補正 (日)
            </span>
            <input
              type="number"
              value={logOffsetDays}
              onChange={(event) => setLogOffsetDays(event.target.value)}
              className="dads-field mono w-full text-[12px]"
            />
          </label>
          <label className="space-y-1">
            <span className="block text-[11px] font-bold text-[var(--dads-ink-sub)]">
              許容差 (秒)
            </span>
            <input
              type="number"
              min={1}
              value={maxGapSeconds}
              onChange={(event) => setMaxGapSeconds(event.target.value)}
              className="dads-field mono w-full text-[12px]"
            />
          </label>
          <label className="space-y-1">
            <span className="block text-[11px] font-bold text-[var(--dads-ink-sub)]">
              グループ距離 (m)
            </span>
            <input
              type="number"
              min={0}
              value={groupDistance}
              onChange={(event) => setGroupDistance(event.target.value)}
              className="dads-field mono w-full text-[12px]"
            />
          </label>
        </div>

        <button
          type="button"
          className="dads-btn w-full"
          data-variant="solid-fill"
          data-size="sm"
          disabled={log.photoLoading || directory.trim() === ""}
          onClick={() => void runLoad(directory)}
        >
          {log.photoLoading ? "読み込み中…" : "写真を読み込んで照合"}
        </button>

        {log.photoError ? (
          <p role="alert" className="text-[12px] whitespace-pre-line font-bold text-[var(--dads-error)]">
            {log.photoError}
          </p>
        ) : null}
      </div>

      {log.photoDirectory ? (
        <p className="text-[12px] text-[var(--dads-ink-sub)]">
          読込元: <span className="mono font-bold">{baseName(log.photoDirectory)}</span> · 写真{" "}
          {log.photos.length} 枚 · 他メディア {log.media.length} 件
          {log.photoSkipped.length > 0 ? ` · スキップ ${log.photoSkipped.length} 件` : ""}
        </p>
      ) : null}

      {activePhoto ? (
        <div className="space-y-2 rounded-lg border border-[var(--dads-border-control)] bg-white p-3">
          <div className="overflow-hidden rounded border border-[var(--dads-border-divider)] bg-[var(--dads-surface-50)]">
            {activePhoto.thumbnail ? (
              <img
                src={activePhoto.thumbnail}
                alt={activePhoto.name}
                className="aspect-[4/3] w-full object-cover"
              />
            ) : (
              <div className="flex aspect-[4/3] flex-col items-center justify-center gap-1 text-[var(--dads-ink-meta)]">
                <Camera size={20} strokeWidth={1.5} aria-hidden="true" />
                <span className="text-xs">サムネイルなし</span>
              </div>
            )}
          </div>

          <div className="flex items-center justify-between">
            <button
              type="button"
              className="dads-btn"
              data-variant="neutral"
              data-size="xs"
              disabled={log.photos.length < 2}
              onClick={() => log.movePhoto(-1)}
              aria-label="前の写真"
            >
              <ChevronLeft size={14} strokeWidth={2} aria-hidden="true" />
            </button>
            <span className="mono text-[12px] font-bold">
              {activeIndex + 1} / {log.photos.length}
            </span>
            <button
              type="button"
              className="dads-btn"
              data-variant="neutral"
              data-size="xs"
              disabled={log.photos.length < 2}
              onClick={() => log.movePhoto(1)}
              aria-label="次の写真"
            >
              <ChevronRight size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>

          <div className="text-[12px]">
            <p className="mono truncate font-bold" title={activePhoto.path}>
              {activePhoto.name}
            </p>
            <p className="mono text-[var(--dads-ink-sub)]">
              {activePhoto.capturedAt === null
                ? "時刻不明"
                : `${formatUtcDateTime(activePhoto.capturedAt)} UTC`}
            </p>
            <p className="mono text-[var(--dads-ink-sub)]">
              {activePhoto.lat !== null && activePhoto.lon !== null
                ? `${activePhoto.lat.toFixed(5)}, ${activePhoto.lon.toFixed(5)}`
                : "座標未割当"}
            </p>
          </div>
        </div>
      ) : null}

      {log.photos.length > 0 ? (
        <ul className="grid grid-cols-3 gap-1.5">
          {log.photos.map((photo, index) => {
            const active = index === activeIndex;
            return (
              <li key={`${photo.path}-${index}`}>
                <button
                  type="button"
                  onClick={() => log.selectPhoto(index)}
                  aria-current={active ? "true" : undefined}
                  className={
                    active
                      ? "block aspect-square w-full overflow-hidden rounded border-2 border-[var(--dads-blue)] cursor-pointer"
                      : "block aspect-square w-full overflow-hidden rounded border border-[var(--dads-border-control)] hover:border-[var(--dads-ink)] cursor-pointer"
                  }
                >
                  {photo.thumbnail ? (
                    <img
                      src={photo.thumbnail}
                      alt={photo.name}
                      loading="lazy"
                      className="size-full object-cover"
                    />
                  ) : (
                    <span className="flex size-full items-center justify-center bg-[var(--dads-surface-50)] text-[var(--dads-ink-meta)]">
                      <Camera size={16} strokeWidth={1.5} aria-hidden="true" />
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      ) : null}

      {log.media.length > 0 ? (
        <div className="space-y-1.5 border-t border-[var(--dads-border-divider)] pt-3">
          <h4 className="text-[12px] font-bold text-[var(--dads-ink)]">
            その他メディア ({log.media.length} 件)
          </h4>
          <ul className="space-y-1 text-[12px]">
            {log.media.map((item) => (
              <li key={item.name} className="flex items-center gap-2 py-1">
                {item.kind === "video" ? (
                  <FileVideo size={14} strokeWidth={1.5} aria-hidden="true" />
                ) : (
                  <File size={14} strokeWidth={1.5} aria-hidden="true" />
                )}
                <span className="min-w-0 flex-1 truncate">{item.name}</span>
                <span className="mono text-[11px] text-[var(--dads-ink-meta)]">
                  {formatBytes(item.bytes)}
                </span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

function integerOr(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : fallback;
}
