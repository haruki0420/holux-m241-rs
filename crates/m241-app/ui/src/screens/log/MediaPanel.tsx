import { File, FileVideo } from "lucide-react";

import type { LogController } from "../../hooks/useLogDocument";
import { formatBytes, formatUtcDateTime } from "../../lib/format";
import { PhotoFolder } from "./PhotoPanels";

/**
 * Videos and other files from the photo folder.
 *
 * Nothing here is drawn on the map: a video has no capture position this tool
 * reads, and listing a file it cannot place is more honest than inventing one.
 */
export function MediaPanel({ log }: { log: LogController }) {
  if (log.media.length === 0) {
    return (
      <div>
        <p className="text-sm text-ink-2">メディアはありません</p>
        <p className="mt-1 text-xs text-ink-3">
          写真フォルダに動画やその他のファイルがあれば、ここに一覧します
        </p>
        <div className="mt-4">
          <PhotoFolder log={log} />
        </div>
      </div>
    );
  }

  return (
    <div>
      <ul className="flex flex-col">
        {log.media.map((item) => (
          <li
            key={item.name}
            className="flex items-start gap-2 border-b border-hair py-2 last:border-b-0"
          >
            <span className="mt-0.5 shrink-0 text-ink-3">
              {item.kind === "video" ? (
                <FileVideo size={16} strokeWidth={1.5} aria-hidden="true" />
              ) : (
                <File size={16} strokeWidth={1.5} aria-hidden="true" />
              )}
            </span>
            <div className="min-w-0">
              <p className="truncate text-xs text-ink" title={item.name}>
                {item.name}
              </p>
              <p className="mono mt-0.5 text-[11px] text-ink-3">
                {item.kind === "video" ? "動画" : "その他"} · {formatBytes(item.bytes)}
                {item.modifiedAt === null
                  ? ""
                  : ` · ${formatUtcDateTime(item.modifiedAt)} UTC`}
              </p>
            </div>
          </li>
        ))}
      </ul>
      <div className="mt-4">
        <PhotoFolder log={log} />
      </div>
    </div>
  );
}
