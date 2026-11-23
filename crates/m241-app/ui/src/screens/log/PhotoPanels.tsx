import { Camera, ChevronLeft, ChevronRight, FolderOpen } from "lucide-react";
import { useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { formatUtcDateTime } from "../../lib/format";

/** The folder loader shared by the three media panels. */
export function PhotoFolder({ log }: { log: LogController }) {
  const [path, setPath] = useState("");
  const [editing, setEditing] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const loaded = log.photoDirectory !== null && log.photos.length + log.media.length > 0;
  const showForm = !loaded || editing;

  if (!showForm) {
    return (
      <div className="flex items-baseline justify-between gap-2 border-t border-hair pt-3">
        <p className="min-w-0 truncate text-xs text-ink-3" title={log.photoDirectory ?? ""}>
          {baseName(log.photoDirectory ?? "")} · {log.photos.length} 枚
        </p>
        <button type="button" className="btn-quiet shrink-0" onClick={() => setEditing(true)}>
          変更
        </button>
      </div>
    );
  }

  return (
    <div className="border-t border-hair pt-3">
      <label className="block text-xs text-ink-3" htmlFor="photo-folder">
        写真フォルダ
      </label>
      <input
        id="photo-folder"
        className="field mt-1 h-8 text-xs"
        placeholder="/home/user/photos"
        value={path}
        onChange={(event) => {
          setPath(event.target.value);
          setSubmitted(false);
        }}
      />
      <button
        type="button"
        className="btn btn-primary mt-2 h-8 w-full text-xs"
        disabled={log.photoLoading || path.trim() === ""}
        onClick={() => {
          setSubmitted(true);
          void log.readPhotoFolder(path).then((ok) => {
            if (ok) {
              setEditing(false);
            }
          });
        }}
      >
        <FolderOpen size={14} strokeWidth={1.5} aria-hidden="true" />
        {log.photoLoading ? "読み込み中" : "読み込む"}
      </button>
      <p className="mt-2 text-[11px] text-ink-3">撮影時刻で最も近い記録点に結び付けます</p>
      {log.photoError === null ? null : (
        <p role="alert" className="mt-2 text-xs whitespace-pre-line text-error">
          {log.photoError}
        </p>
      )}
      {submitted && log.photoSkipped.length > 0 ? (
        <p className="mt-2 text-[11px] text-ink-3">
          読み込めなかったファイル {log.photoSkipped.length} 件
        </p>
      ) : null}
    </div>
  );
}

/** The large photo, with the time and the point that matched it. */
export function PhotoViewPanel({ log }: { log: LogController }) {
  if (log.photos.length === 0) {
    return (
      <div>
        <p className="text-sm text-ink-2">写真はまだ読み込んでいません</p>
        <p className="mt-1 text-xs text-ink-3">
          写真フォルダを読み込むと、撮影時刻の近い記録点に結び付けます
        </p>
        <div className="mt-4">
          <PhotoFolder log={log} />
        </div>
      </div>
    );
  }

  const index = log.photoIndex ?? 0;
  const photo = log.photos[index];
  if (photo === undefined) {
    return null;
  }

  return (
    <div>
      <div className="relative overflow-hidden rounded-control border border-hair bg-inset">
        {photo.thumbnail === null ? (
          <div className="flex aspect-[4/3] w-full flex-col items-center justify-center gap-2 text-ink-3">
            <Camera size={20} strokeWidth={1.5} aria-hidden="true" />
            <p className="text-xs">表示できません</p>
          </div>
        ) : (
          <img
            src={photo.thumbnail}
            alt={photo.name}
            className="aspect-[4/3] w-full object-cover"
          />
        )}
      </div>

      <div className="mt-2 flex items-center justify-between gap-2">
        <button
          type="button"
          aria-label="前の写真"
          className="btn-quiet"
          disabled={log.photos.length < 2}
          onClick={() => log.movePhoto(-1)}
        >
          <ChevronLeft size={14} strokeWidth={1.5} aria-hidden="true" />
        </button>
        <span className="mono text-[11px] text-ink-3">
          {index + 1} / {log.photos.length}
        </span>
        <button
          type="button"
          aria-label="次の写真"
          className="btn-quiet"
          disabled={log.photos.length < 2}
          onClick={() => log.movePhoto(1)}
        >
          <ChevronRight size={14} strokeWidth={1.5} aria-hidden="true" />
        </button>
      </div>

      <p className="mono mt-2 truncate text-xs text-ink" title={photo.path}>
        {photo.name}
      </p>

      <dl className="mt-3 flex flex-col gap-1.5 border-t border-hair pt-3">
        <Row
          label="撮影時刻"
          value={
            photo.capturedAt === null
              ? "—"
              : `${formatUtcDateTime(photo.capturedAt)} UTC`
          }
        />
        <Row
          label="時刻の出どころ"
          value={photo.timeSource === "exif" ? "Exif" : photo.timeSource === "file" ? "ファイル更新" : "—"}
        />
        <Row
          label="結び付けた点"
          value={photo.pointIndex === null ? "なし" : `${photo.pointIndex + 1} 点目`}
        />
        <Row
          label="座標"
          value={
            photo.lat === null || photo.lon === null
              ? "—"
              : `${photo.lat.toFixed(5)}, ${photo.lon.toFixed(5)}`
          }
        />
      </dl>

      <div className="mt-3 border-t border-hair pt-3">
        <p className="text-xs text-ink-3">コメント</p>
        <p className="mt-1 text-xs text-ink-2">コメントはありません</p>
      </div>

      <div className="mt-4">
        <PhotoFolder log={log} />
      </div>
    </div>
  );
}

/** The thumbnail grid, in capture order. */
export function PhotoListPanel({ log }: { log: LogController }) {
  if (log.photos.length === 0) {
    return (
      <div>
        <p className="text-sm text-ink-2">写真はまだ読み込んでいません</p>
        <p className="mt-1 text-xs text-ink-3">
          読み込むと、撮影時刻順の一覧から地図上の点へ移動できます
        </p>
        <div className="mt-4">
          <PhotoFolder log={log} />
        </div>
      </div>
    );
  }

  return (
    <div>
      <ul className="grid grid-cols-3 gap-1.5">
        {log.photos.map((photo, index) => {
          const active = index === (log.photoIndex ?? -1);
          return (
            <li key={`${photo.path}-${index}`}>
              <button
                type="button"
                onClick={() => log.selectPhoto(index)}
                aria-current={active ? "true" : undefined}
                aria-label={`${photo.name}${
                  photo.capturedAt === null ? "" : ` ${formatUtcDateTime(photo.capturedAt)}`
                }`}
                className={
                  active
                    ? "block aspect-square w-full overflow-hidden rounded-tag outline outline-2 outline-accent"
                    : "block aspect-square w-full overflow-hidden rounded-tag border border-hair hover:outline hover:outline-1 hover:outline-hair-strong"
                }
              >
                {photo.thumbnail === null ? (
                  <span className="flex size-full items-center justify-center bg-inset text-ink-3">
                    <Camera size={16} strokeWidth={1.5} aria-hidden="true" />
                  </span>
                ) : (
                  <img
                    src={photo.thumbnail}
                    alt=""
                    loading="lazy"
                    className="size-full object-cover"
                  />
                )}
              </button>
            </li>
          );
        })}
      </ul>
      <div className="mt-4">
        <PhotoFolder log={log} />
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <dt className="shrink-0 text-xs text-ink-3">{label}</dt>
      <dd className="mono min-w-0 truncate text-right text-xs text-ink-2">{value}</dd>
    </div>
  );
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
