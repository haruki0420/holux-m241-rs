import { useMemo, type PointerEvent as ReactPointerEvent } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { formatUtcTime } from "../../lib/format";
import type { PointDto } from "../../lib/ipc";

const WIDTH = 260;
const PAD_X = 4;
const PAD_Y = 8;
const GAP_SECONDS = 600;

type Sample = { x: number; value: number; breakBefore: boolean };

type Axis = {
  xByIndex: Map<number, number>;
  breakAfter: Set<number>;
  start: number | null;
  end: number | null;
};

function buildAxis(points: readonly PointDto[]): Axis {
  const timed = points.filter((point) => point.utc !== null);
  const xByIndex = new Map<number, number>();
  const breakAfter = new Set<number>();
  if (timed.length === 0) {
    return { xByIndex, breakAfter, start: null, end: null };
  }
  let cumulative = 0;
  let previous: number | null = null;
  for (const point of timed) {
    if (point.utc === null) {
      continue;
    }
    if (previous !== null) {
      const gap = point.utc - previous;
      if (gap > GAP_SECONDS) {
        breakAfter.add(previous);
        cumulative += GAP_SECONDS;
      } else if (gap > 0) {
        cumulative += gap;
      }
    }
    previous = point.utc;
    xByIndex.set(point.index, cumulative);
  }
  const total = Math.max(cumulative, 1);
  for (const [index, value] of xByIndex) {
    xByIndex.set(index, PAD_X + (value / total) * (WIDTH - PAD_X * 2));
  }
  return {
    xByIndex,
    breakAfter,
    start: timed[0]?.utc ?? null,
    end: timed[timed.length - 1]?.utc ?? null,
  };
}

function series(
  points: readonly PointDto[],
  axis: Axis,
  pick: (point: PointDto) => number | null,
): Sample[] {
  const samples: Sample[] = [];
  let breakBefore = false;
  let previousIndex: number | null = null;
  for (const point of points) {
    const x = axis.xByIndex.get(point.index);
    if (x === undefined) {
      continue;
    }
    const value = pick(point);
    if (value === null || !Number.isFinite(value)) {
      breakBefore = true;
      continue;
    }
    const gap =
      previousIndex !== null &&
      breakBefore === false &&
      axis.breakAfter.has(previousIndex);
    samples.push({ x, value, breakBefore: breakBefore || gap });
    breakBefore = false;
    previousIndex = point.index;
  }
  return samples;
}

function yOf(value: number, min: number, max: number, height: number): number {
  const span = Math.max(max - min, 1e-6);
  return PAD_Y + (1 - (value - min) / span) * (height - PAD_Y * 2);
}

function linePath(
  samples: Sample[],
  min: number,
  max: number,
  height: number,
): string {
  return samples
    .map(
      (sample, index) =>
        `${index === 0 || sample.breakBefore ? "M" : "L"}${sample.x.toFixed(2)},${yOf(
          sample.value,
          min,
          max,
          height,
        ).toFixed(2)}`,
    )
    .join(" ");
}

function areaPath(
  samples: Sample[],
  min: number,
  max: number,
  height: number,
): string {
  const bottom = height - PAD_Y;
  const parts: string[] = [];
  let run: Sample[] = [];
  const flush = () => {
    if (run.length < 2) {
      run = [];
      return;
    }
    const first = run[0];
    const last = run[run.length - 1];
    if (first === undefined || last === undefined) {
      run = [];
      return;
    }
    const segment = linePath(run, min, max, height);
    parts.push(`${segment} L${last.x.toFixed(2)},${bottom} L${first.x.toFixed(2)},${bottom} Z`);
    run = [];
  };
  for (const sample of samples) {
    if (sample.breakBefore) {
      flush();
    }
    run.push(sample);
  }
  flush();
  return parts.join(" ");
}

function rangeOf(
  samples: Sample[],
  fallbackMax: number,
  zeroBased: boolean,
): { min: number; max: number } {
  if (samples.length === 0) {
    return zeroBased ? { min: 0, max: fallbackMax } : { min: -1, max: fallbackMax };
  }
  let min = Infinity;
  let max = -Infinity;
  for (const sample of samples) {
    min = Math.min(min, sample.value);
    max = Math.max(max, sample.value);
  }
  if (!Number.isFinite(min) || !Number.isFinite(max)) {
    return zeroBased ? { min: 0, max: fallbackMax } : { min: -1, max: fallbackMax };
  }
  if (zeroBased) {
    return { min: 0, max: Math.max(max, 1) };
  }
  if (max - min < 1) {
    return { min: min - 1, max: max + 1 };
  }
  return { min, max };
}

/**
 * Interactive altitude & speed chart using DADS colors.
 * Clicking or dragging moves the active cursor across the track.
 */
export function ChartPanel({ log }: { log: LogController }) {
  const points = log.points;
  const axis = useMemo(() => buildAxis(points), [points]);
  const speed = useMemo(
    () => series(points, axis, (point) => point.speedKmh),
    [points, axis],
  );
  const altitude = useMemo(
    () => series(points, axis, (point) => point.heightM),
    [points, axis],
  );
  const speedRange = rangeOf(speed, 10, true);
  const altitudeRange = rangeOf(altitude, 100, false);
  const cursor =
    log.selectedIndex === null ? null : (axis.xByIndex.get(log.selectedIndex) ?? null);
  const timestamps = useMemo(
    () => points.filter((point) => point.utc !== null),
    [points],
  );

  const seekFromPointer = (event: ReactPointerEvent<SVGSVGElement>): number | null => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width === 0 || timestamps.length === 0) {
      return null;
    }
    const x = ((event.clientX - rect.left) / rect.width) * WIDTH;
    let best: number | null = null;
    let bestDistance = Infinity;
    for (const [index, sampleX] of axis.xByIndex) {
      const distance = Math.abs(sampleX - x);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = index;
      }
    }
    return best;
  };

  const selected = log.selectedPoint;

  return (
    <div className="space-y-3">
      <dl className="grid grid-cols-2 gap-3 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-3">
        <div>
          <dt className="text-[11px] font-bold text-[var(--dads-ink-sub)]">現在速度</dt>
          <dd className="mono mt-0.5 text-sm font-bold text-[var(--dads-ink)]">
            {selected?.speedKmh == null ? "—" : `${selected.speedKmh.toFixed(1)} km/h`}
          </dd>
        </div>
        <div>
          <dt className="text-[11px] font-bold text-[var(--dads-ink-sub)]">現在高度</dt>
          <dd className="mono mt-0.5 text-sm font-bold text-[var(--dads-ink)]">
            {selected?.heightM == null ? "—" : `${Math.round(selected.heightM)} m`}
          </dd>
        </div>
      </dl>

      <Chart
        label="高度 (m)"
        unit="m"
        samples={altitude}
        range={altitudeRange}
        height={96}
        cursor={cursor}
        stroke="var(--dads-blue)"
        onSeek={(index) => log.seek(index)}
        onSample={seekFromPointer}
      />

      <Chart
        label="速度 (km/h)"
        unit="km/h"
        samples={speed}
        range={speedRange}
        height={88}
        cursor={cursor}
        stroke="var(--dads-warning-border)"
        onSeek={(index) => log.seek(index)}
        onSample={seekFromPointer}
      />

      <div className="flex justify-between text-[11px] text-[var(--dads-ink-meta)]">
        <span className="mono">
          {axis.start === null ? "—" : `${formatUtcTime(axis.start)} UTC`}
        </span>
        <span className="mono">
          {axis.end === null ? "—" : `${formatUtcTime(axis.end)} UTC`}
        </span>
      </div>
      <p className="text-[11px] text-[var(--dads-ink-meta)]">
        クリックまたはドラッグで再生位置を移動できます（10分以上の休止間隔は圧縮表示）。
      </p>
    </div>
  );
}

function Chart({
  label,
  unit,
  samples,
  range,
  height,
  cursor,
  stroke,
  onSeek,
  onSample,
}: {
  label: string;
  unit: string;
  samples: Sample[];
  range: { min: number; max: number };
  height: number;
  cursor: number | null;
  stroke: string;
  onSeek: (index: number) => void;
  onSample: (event: ReactPointerEvent<SVGSVGElement>) => number | null;
}) {
  const line = linePath(samples, range.min, range.max, height);
  const area = areaPath(samples, range.min, range.max, height);
  const span = Math.max(range.max - range.min, 1e-6);
  const cursorSample =
    cursor === null || samples.length === 0
      ? null
      : samples.reduce((best, sample) =>
          Math.abs(sample.x - cursor) < Math.abs(best.x - cursor) ? sample : best,
        );
  const cursorY =
    cursorSample === null
      ? null
      : PAD_Y + (1 - (cursorSample.value - range.min) / span) * (height - PAD_Y * 2);

  return (
    <section aria-label={`${label}の推移`} className="rounded-lg border border-[var(--dads-border-divider)] bg-white p-2.5">
      <div className="flex items-baseline justify-between text-[12px]">
        <span className="font-bold text-[var(--dads-ink)]">{label}</span>
        <span className="mono text-[11px] text-[var(--dads-ink-sub)]">
          {Math.round(range.min)}–{Math.round(range.max)} {unit}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${WIDTH} ${height}`}
        className="mt-1.5 block h-auto w-full cursor-crosshair touch-none"
        role="img"
        aria-label={`${label}の推移グラフ`}
        onPointerDown={(event) => {
          const index = onSample(event);
          if (index !== null) {
            onSeek(index);
          }
        }}
        onPointerMove={(event) => {
          if (event.buttons === 1) {
            const index = onSample(event);
            if (index !== null) {
              onSeek(index);
            }
          }
        }}
      >
        {[0, 0.5, 1].map((fraction) => (
          <line
            key={fraction}
            x1={PAD_X}
            x2={WIDTH - PAD_X}
            y1={PAD_Y + fraction * (height - PAD_Y * 2)}
            y2={PAD_Y + fraction * (height - PAD_Y * 2)}
            stroke="var(--dads-border-divider)"
            strokeWidth={0.8}
          />
        ))}
        {area === "" ? null : <path d={area} fill={stroke} opacity={0.12} />}
        {line === "" ? null : (
          <path
            d={line}
            fill="none"
            stroke={stroke}
            strokeWidth={1.6}
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        )}
        {cursor === null ? null : (
          <g>
            <line
              x1={cursor}
              x2={cursor}
              y1={PAD_Y}
              y2={height - PAD_Y}
              stroke="var(--dads-ink)"
              strokeWidth={1}
              opacity={0.7}
            />
            {cursorY === null ? null : (
              <circle cx={cursor} cy={cursorY} r={2.5} fill={stroke} />
            )}
          </g>
        )}
        {samples.length === 0 ? (
          <text
            x={WIDTH / 2}
            y={height / 2}
            textAnchor="middle"
            fill="var(--dads-ink-meta)"
            fontSize={10}
          >
            記録なし
          </text>
        ) : null}
      </svg>
    </section>
  );
}
