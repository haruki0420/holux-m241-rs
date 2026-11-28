import { useEffect, useMemo, useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import {
  formatCount,
  formatHeight,
  formatLatLon,
  formatSpeed,
  formatUtcTime,
} from "../../lib/format";

const PAGE_SIZE = 100;

/**
 * Paginated track points table backed by `log_points`.
 */
export function TrackPointsPanel({ log }: { log: LogController }) {
  const [waypointsOnly, setWaypointsOnly] = useState(false);
  const [page, setPage] = useState(0);

  const filtered = useMemo(() => {
    if (!waypointsOnly) {
      return log.points;
    }
    return log.points.filter((pt) => pt.waypoint);
  }, [log.points, waypointsOnly]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));

  useEffect(() => {
    if (page >= totalPages) {
      setPage(0);
    }
  }, [page, totalPages]);

  const slice = useMemo(() => {
    const start = page * PAGE_SIZE;
    return filtered.slice(start, start + PAGE_SIZE);
  }, [filtered, page]);

  const jumpToSelected = () => {
    if (log.selectedIndex === null) return;
    const pos = filtered.findIndex((pt) => pt.index === log.selectedIndex);
    if (pos >= 0) {
      setPage(Math.floor(pos / PAGE_SIZE));
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <label className="flex cursor-pointer items-center gap-2 text-[12px] font-bold">
          <input
            type="checkbox"
            className="dads-checkbox"
            checked={waypointsOnly}
            onChange={(event) => {
              setWaypointsOnly(event.target.checked);
              setPage(0);
            }}
          />
          <span>ウェイポイントのみ</span>
        </label>

        {log.selectedIndex !== null ? (
          <button
            type="button"
            className="dads-btn"
            data-variant="outline"
            data-size="xs"
            onClick={jumpToSelected}
          >
            選択点 (#{log.selectedIndex + 1}) へ
          </button>
        ) : null}
      </div>

      <div className="flex items-center justify-between text-[12px] text-[var(--dads-ink-sub)]">
        <span className="mono">
          全 {formatCount(filtered.length)} 点（{page + 1} / {totalPages} ページ）
        </span>
        <div className="flex items-center gap-1">
          <button
            type="button"
            className="dads-btn"
            data-variant="neutral"
            data-size="xs"
            disabled={page === 0}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
          >
            前へ
          </button>
          <button
            type="button"
            className="dads-btn"
            data-variant="neutral"
            data-size="xs"
            disabled={page >= totalPages - 1}
            onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
          >
            次へ
          </button>
        </div>
      </div>

      {slice.length === 0 ? (
        <p className="text-[12px] text-[var(--dads-ink-meta)]">該当する点がありません。</p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-[var(--dads-border-control)]">
          <table className="dads-table text-[11px]">
            <thead>
              <tr>
                <th>#</th>
                <th>時刻(UTC)</th>
                <th>座標</th>
                <th className="text-right">標高 / 速度</th>
              </tr>
            </thead>
            <tbody>
              {slice.map((point) => {
                const selected = point.index === log.selectedIndex;
                return (
                  <tr
                    key={point.index}
                    onClick={() => log.openPopup(point.index)}
                    className={
                      selected
                        ? "cursor-pointer bg-[var(--dads-blue-tint)] font-bold"
                        : "cursor-pointer hover:bg-[var(--dads-surface-50)]"
                    }
                  >
                    <td className="mono whitespace-nowrap">
                      <span>{point.index + 1}</span>
                      {point.waypoint ? (
                        <span className="ml-1 inline-block rounded bg-[var(--dads-blue)] px-1 text-[10px] font-bold text-white">
                          WP
                        </span>
                      ) : null}
                    </td>
                    <td className="mono whitespace-nowrap">
                      {point.utc === null ? "—" : formatUtcTime(point.utc)}
                    </td>
                    <td className="mono whitespace-nowrap">
                      {formatLatLon(point.lat, point.lon)}
                    </td>
                    <td className="mono whitespace-nowrap text-right">
                      {formatHeight(point.heightM)} / {formatSpeed(point.speedKmh)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
