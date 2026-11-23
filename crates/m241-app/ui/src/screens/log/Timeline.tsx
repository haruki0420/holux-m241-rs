import { Pause, Play } from "lucide-react";
import { useMemo } from "react";

import { formatUtcDateTime } from "../../lib/format";
import type { PointDto } from "../../lib/ipc";

/**
 * The scrubber along the bottom of the map.
 *
 * It moves the cursor, not the camera: the map follows only while 追従 is on.
 * The range input is the keyboard path to every point (arrow keys move one
 * point, Home/End jump to the ends).
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
    <div className="timeline z-map" role="group" aria-label="再生位置">
      <button
        type="button"
        aria-label={playing ? "一時停止" : "再生"}
        title={playing ? "一時停止" : "再生"}
        onClick={onTogglePlay}
        disabled={disabled}
        className="flex size-8 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-raise hover:text-ink disabled:cursor-not-allowed disabled:opacity-40"
      >
        {playing ? (
          <Pause size={14} strokeWidth={1.5} aria-hidden="true" />
        ) : (
          <Play size={14} strokeWidth={1.5} aria-hidden="true" />
        )}
      </button>

      <input
        type="range"
        className="timeline-range min-w-0 flex-1"
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

      <div className="w-[132px] shrink-0 text-right">
        <p className="mono text-xs text-ink">
          {current?.utc == null ? "—" : `${formatUtcDateTime(current.utc).slice(11)} UTC`}
        </p>
        <p className="mono text-[11px] text-ink-3">
          {points.length === 0 ? "0 点" : `${position + 1} / ${points.length} 点`}
        </p>
      </div>
    </div>
  );
}
