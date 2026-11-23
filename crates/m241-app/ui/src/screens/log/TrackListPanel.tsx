import type { LogController } from "../../hooks/useLogDocument";
import {
  formatDistance,
  formatDuration,
  formatShortDate,
  formatUtcDate,
} from "../../lib/format";

/**
 * The day rows.
 *
 * A day is the unit the operator actually checks ("did the walk on the 17th
 * record?"), so the rail lists days rather than files. The active row is the day
 * the selected point belongs to; clicking frames that day without moving the
 * selection, which is what makes it a view change rather than a seek.
 */
export function TrackListPanel({ log }: { log: LogController }) {
  const summary = log.summary;
  if (summary === null) {
    return null;
  }
  const activeDate =
    log.selectedPoint?.utc == null ? null : formatUtcDate(log.selectedPoint.utc);

  return (
    <div>
      <button
        type="button"
        onClick={log.fitWhole}
        className="flex h-9 w-full items-center justify-between gap-2 rounded-control px-2 text-left text-xs text-ink-2 hover:bg-inset hover:text-ink"
      >
        <span>すべての日</span>
        <span className="mono text-[11px] text-ink-3">
          {formatDistance(summary.distanceM)}
        </span>
      </button>

      <ul className="mt-1 flex flex-col">
        {summary.days.map((day) => {
          const active = day.date === activeDate;
          return (
            <li key={day.date}>
              <button
                type="button"
                onClick={() => {
                  log.fitDay(day);
                }}
                aria-current={active ? "true" : undefined}
                className={
                  active
                    ? "relative block w-full rounded-control bg-inset px-2 py-2 text-left"
                    : "relative block w-full rounded-control px-2 py-2 text-left hover:bg-inset"
                }
              >
                {active ? <span aria-hidden="true" className="trail-mark" /> : null}
                <span className="flex items-baseline justify-between gap-2">
                  <span className={active ? "text-sm font-medium text-ink" : "text-sm text-ink"}>
                    {formatShortDate(day.date)}
                  </span>
                  <span className="mono text-[11px] text-ink-3">
                    {formatDuration(day.durationSeconds)}
                  </span>
                </span>
                <span className="mono mt-0.5 block text-[11px] text-ink-3">
                  {day.points} 点 · {formatDistance(day.distanceM)}
                </span>
              </button>
            </li>
          );
        })}
      </ul>

      <dl className="mt-4 flex flex-col gap-1.5 border-t border-hair pt-3">
        <Row label="距離" value={formatDistance(summary.distanceM)} />
        <Row label="時間" value={formatDuration(summary.durationSeconds)} />
        <Row
          label="平均速度"
          value={
            summary.averageSpeedKmh === null
              ? "—"
              : `${summary.averageSpeedKmh.toFixed(1)} km/h`
          }
        />
        <Row label="目印" value={`${summary.waypoints} 点`} />
      </dl>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <dt className="text-xs text-ink-3">{label}</dt>
      <dd className="mono text-xs text-ink-2">{value}</dd>
    </div>
  );
}
