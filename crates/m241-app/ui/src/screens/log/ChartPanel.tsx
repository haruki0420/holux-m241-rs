import { useMemo, type PointerEvent as ReactPointerEvent } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { formatUtcTime } from "../../lib/format";
import type { PointDto } from "../../lib/ipc";

const WIDTH = 240;
const PAD_X = 2;
const PAD_Y = 6;

/**
 * A gap longer than this is compressed to a fixed sliver on the time axis.
 *
 * The sample (and a real log left running overnight) can span a day with two
 * hour-long sessions in it. Plotting 24 hours literally squeezes both sessions
 * into a few pixels; compressing the overnight gap keeps each session readable
 * while the axis still says which day it starts and ends on.
 */
const GAP_SECONDS = 600;

type Sample = { x: number; value: number; breakBefore: boolean };

type Axis = {
  xByIndex: Map<number, number>;
  breakAfter: Set<number>;
  start: number | null;
  end: number | null;
};

/** The compressed time axis shared by both charts and the cursor. */
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

/** One line series, with a break wherever the value or the time axis breaks. */
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

/** The polyline, starting a new subpath at each break. */
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

/** The accent wash under each continuous run. */
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
  // Speed starts at zero because "0 km/h" is a value an operator reads; a flat
  // altitude still needs a band to sit in.
  if (zeroBased) {
    return { min: 0, max: Math.max(max, 1) };
  }
  if (max - min < 1) {
    return { min: min - 1, max: max + 1 };
  }
  return { min, max };
}

/**
 * Speed and altitude over time, drawn twice rather than overlaid.
 *
 * Two units on one axis is the kind of chart that needs a legend to read; the
 * panel instead gives speed the accent (the series the operator checks against
 * the log) and altitude a quiet ink line, sharing one compressed time axis and
 * one cursor. Clicking or dragging either chart moves the cursor.
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
    <div>
      <dl className="grid grid-cols-2 gap-x-3 gap-y-2 border-b border-hair pb-3">
        <div>
          <dt className="text-[11px] text-ink-3">速度</dt>
          <dd className="mono mt-0.5 text-sm text-ink">
            {selected?.speedKmh == null ? "—" : `${selected.speedKmh.toFixed(1)} km/h`}
          </dd>
        </div>
        <div>
          <dt className="text-[11px] text-ink-3">高度</dt>
          <dd className="mono mt-0.5 text-sm text-ink">
            {selected?.heightM == null ? "—" : `${Math.round(selected.heightM)} m`}
          </dd>
        </div>
      </dl>

      <Chart
        label="速度"
        unit="km/h"
        samples={speed}
        range={speedRange}
        height={82}
        cursor={cursor}
        accent
        onSeek={(index) => log.seek(index)}
        onSample={seekFromPointer}
      />
      <Chart
        label="高度"
        unit="m"
        samples={altitude}
        range={altitudeRange}
        height={64}
        cursor={cursor}
        onSeek={(index) => log.seek(index)}
        onSample={seekFromPointer}
      />

      <div className="mt-1 flex justify-between text-[11px] text-ink-3">
        <span className="mono">
          {axis.start === null ? "—" : formatUtcTime(axis.start)}
        </span>
        <span className="mono">{axis.end === null ? "—" : formatUtcTime(axis.end)}</span>
      </div>
      <p className="mt-1 text-[11px] text-ink-3">10 分以上の間隔は詰めて表示しています</p>

      <dl className="mt-3 flex flex-col gap-1.5 border-t border-hair pt-3">
        <Row
          label="平均 / 最高"
          value={
            log.summary?.averageSpeedKmh == null
              ? "—"
              : `${log.summary.averageSpeedKmh.toFixed(1)} / ${
                  log.summary.maxSpeedKmh == null
                    ? "—"
                    : log.summary.maxSpeedKmh.toFixed(1)
                } km/h`
          }
        />
        <Row
          label="高度の幅"
          value={
            log.summary?.minAltitudeM == null || log.summary.maxAltitudeM == null
              ? "—"
              : `${Math.round(log.summary.minAltitudeM)}–${Math.round(
                  log.summary.maxAltitudeM,
                )} m`
          }
        />
      </dl>
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
  accent = false,
  onSeek,
  onSample,
}: {
  label: string;
  unit: string;
  samples: Sample[];
  range: { min: number; max: number };
  height: number;
  cursor: number | null;
  accent?: boolean;
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
    <section className="mt-3" aria-label={`${label}の推移`}>
      <div className="flex items-baseline justify-between text-[11px] text-ink-3">
        <span>{label}</span>
        <span className="mono">
          {Math.round(range.min)}–{Math.round(range.max)} {unit}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${WIDTH} ${height}`}
        className="mt-1 block h-auto w-full touch-none"
        role="img"
        aria-label={`${label}の推移。クリックで再生位置を移動できます`}
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
            stroke="var(--hair)"
            strokeWidth={1}
          />
        ))}
        {area === "" ? null : (
          <path
            d={area}
            fill={accent ? "var(--accent)" : "var(--ink-3)"}
            opacity={accent ? 0.1 : 0.06}
          />
        )}
        {line === "" ? null : (
          <path
            d={line}
            fill="none"
            stroke={accent ? "var(--accent)" : "var(--ink-3)"}
            strokeWidth={accent ? 1.6 : 1.2}
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
              stroke="var(--ink-3)"
              strokeWidth={1}
              opacity={0.55}
            />
            {cursorY === null ? null : (
              <circle
                cx={cursor}
                cy={cursorY}
                r={2}
                fill={accent ? "var(--accent)" : "var(--ink-3)"}
              />
            )}
          </g>
        )}
        {samples.length === 0 ? (
          <text
            x={WIDTH / 2}
            y={height / 2}
            textAnchor="middle"
            fill="var(--ink-3)"
            fontSize={10}
          >
            記録なし
          </text>
        ) : null}
      </svg>
    </section>
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
