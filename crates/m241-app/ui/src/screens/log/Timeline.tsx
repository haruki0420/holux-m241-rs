import { Pause, Play } from "lucide-react";
import { useMemo } from "react";

import { formatHeight, formatSpeed, formatUtcDateTime } from "../../lib/format";
import type { PointDto } from "../../lib/ipc";

/**
 * Floating bottom playback bar over the map.
 */
export function Timeline({
  points,
  selectedIndex,
  playing,
  onSeek,
  onTogglePlay,
}: {
  points: PointDto[];
  selectedIndex: number | null;
  playing: boolean;
  onSeek: (index: number) => void;
  onTogglePlay: () => void;
}) {
  const positions = useMemo(() => {
    const map = new Map<number, number>();
    points.forEach((point, position) => map.set(point.index, position));
    return map;
  }, [points]);

  const position = selectedIndex === null ? 0 : (positions.get(selectedIndex) ?? 0);
  const current = points[position] ?? null;
  const disabled = points.length < 2;

  return (
    <div
      role="group"
      aria-label="再生位置"
      className="z-map absolute inset-x-3 bottom-3 flex items-center gap-3 rounded-lg border border-[var(--dads-border-control)] bg-white/95 px-3 py-2 text-[13px] shadow-lg backdrop-blur"
    >
      <button
        type="button"
        className="dads-btn shrink-0"
        data-variant="outline"
        data-size="xs"
        aria-label={playing ? "一時停止" : "再生"}
        onClick={onTogglePlay}
        disabled={disabled}
      >
        {playing ? (
          <>
            <Pause size={13} strokeWidth={2} aria-hidden="true" />
            <span>停止</span>
          </>
        ) : (
          <>
            <Play size={13} strokeWidth={2} aria-hidden="true" />
            <span>再生</span>
          </>
        )}
      </button>

      <input
        type="range"
        className="min-w-0 flex-1 cursor-pointer"
        min={0}
        max={Math.max(points.length - 1, 0)}
        step={1}
        value={position}
        disabled={disabled}
        aria-label="再生位置"
        aria-valuetext={
          current?.utc == null ? "位置" : `${formatUtcDateTime(current.utc)} UTC`
        }
        onChange={(event) => {
          const next = points[Number(event.target.value)];
          if (next !== undefined) {
            onSeek(next.index);
          }
        }}
      />

      <div className="mono shrink-0 whitespace-nowrap text-right text-[12px] font-bold text-[var(--dads-ink)]">
        <span>{current?.utc == null ? "—" : `${formatUtcDateTime(current.utc)} UTC`}</span>
        <span className="ml-2 text-[var(--dads-ink-sub)]">
          {formatSpeed(current?.speedKmh ?? null)} · {formatHeight(current?.heightM ?? null)}
        </span>
        <span className="ml-2 text-[var(--dads-ink-meta)]">
          ({points.length === 0 ? "0 点" : `${position + 1}/${points.length}`})
        </span>
      </div>
    </div>
  );
}
