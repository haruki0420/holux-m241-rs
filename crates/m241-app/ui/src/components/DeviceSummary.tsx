import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";
import { readout, SAMPLE } from "../lib/sample";

/**
 * The instrument's readout, parked at the bottom of the rail. It holds the
 * facts an operator checks without leaving the screen: which port, which
 * firmware, how much of the log is used. Before a read the values are dashes,
 * so the block still says what it will hold.
 */
export function DeviceSummary({ controller }: { controller: DeviceProbeController }) {
  const { state, sample } = controller;
  const info = sample ? SAMPLE : state.kind === "connected" ? state.info : null;
  const values = readout(info);
  const port = state.kind === "probing" ? state.port : values.port;

  return (
    <div className="mt-auto">
      <div className="border-t border-hair pt-3">
        <div className="flex items-center justify-between gap-2 px-2">
          <p className="text-xs text-ink-3">端末</p>
          {sample ? <span className="tag">見本</span> : null}
        </div>

        <dl className="mt-2 flex flex-col gap-2 px-2">
          <Row label="ポート" value={port} />
          <Row label="ファーム" value={values.firmware} />
          <Row label="ログ" value={values.log} />
        </dl>

        <p className="mt-3 px-2 text-xs text-ink-3">読み取り専用</p>
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  const empty = value === "—";
  return (
    <div className="flex items-baseline justify-between gap-2">
      <dt className="shrink-0 text-xs text-ink-3">{label}</dt>
      <dd className={cn("mono min-w-0 truncate text-xs", empty ? "text-ink-3" : "text-ink-2")}>
        {value}
      </dd>
    </div>
  );
}
