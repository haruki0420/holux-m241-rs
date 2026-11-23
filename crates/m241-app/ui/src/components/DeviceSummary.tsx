import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { formatBytes } from "../lib/format";

/**
 * The status summary parked at the bottom of the rail.
 *
 * It answers one question — is a logger connected, and if so which one — in the
 * space the previous draft spent on three dashes. Before a read there is nothing
 * to report, so it says `未接続` once instead of drawing empty rows.
 */
export function DeviceSummary({ controller }: { controller: DeviceProbeController }) {
  const { state } = controller;

  return (
    <div className="mt-auto border-t border-hair pt-3">
      <p className="px-2 text-xs text-ink-3">端末</p>

      {state.kind === "connected" ? (
        <dl className="mt-2 flex flex-col gap-1.5 px-2">
          <Row label="ポート" value={state.info.port} />
          <Row label="ファーム" value={state.info.release} />
          <Row label="ログ" value={formatBytes(state.info.logUsedBytes)} />
        </dl>
      ) : state.kind === "probing" ? (
        <p className="mt-2 flex items-center gap-2 px-2 text-sm text-ink-2">
          <span aria-hidden="true" className="size-1.5 shrink-0 rounded-full bg-accent" />
          読み取り中
        </p>
      ) : state.kind === "error" ? (
        <p className="mt-2 px-2 text-sm text-error">読み取り失敗</p>
      ) : (
        <p className="mt-2 px-2 text-sm text-ink-2">未接続</p>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <dt className="shrink-0 text-xs text-ink-3">{label}</dt>
      <dd className="mono min-w-0 truncate text-xs text-ink-2">{value}</dd>
    </div>
  );
}
