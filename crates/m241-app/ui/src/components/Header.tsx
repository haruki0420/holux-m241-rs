import type { ConnectionTone, DeviceProbeController } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";

const LABEL: Record<ConnectionTone, string> = {
  idle: "未接続",
  probing: "読み取り中",
  connected: "接続済み",
  error: "読み取り失敗",
};

const TONE: Record<ConnectionTone, string> = {
  idle: "bg-ink-3",
  probing: "bg-accent",
  connected: "bg-accent",
  error: "bg-error",
};

/**
 * The top bar carries the two facts that never scroll away: which tool this is,
 * and what the serial side is doing right now. The state word is always
 * present, so the dot is never the only signal.
 */
export function Header({ controller }: { controller: DeviceProbeController }) {
  const { state, tone, activePort } = controller;
  const port =
    state.kind === "connected"
      ? state.info.port
      : state.kind === "probing"
        ? state.port
        : activePort;

  return (
    <header className="flex h-14 shrink-0 items-center justify-between gap-6 border-b border-hair bg-card px-4">
      <div className="flex min-w-0 items-center gap-3">
        <Mark />
        <span className="text-md font-medium text-ink">Holux M-241</span>
      </div>

      <div
        role="status"
        className="flex min-w-0 items-center gap-2 text-xs text-ink-2"
      >
        <span aria-hidden="true" className={cn("size-1.5 shrink-0 rounded-full", TONE[tone])} />
        <span className={cn("shrink-0", tone === "error" ? "text-error" : "text-ink-2")}>
          {LABEL[tone]}
        </span>
        {port === "" ? null : (
          <span className="mono min-w-0 truncate text-ink-3">{port}</span>
        )}
      </div>
    </header>
  );
}

/** A contour ring with a trail and the fix dot: the logger's own geometry. */
function Mark() {
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
      className="shrink-0 text-ink"
    >
      <circle cx="10" cy="10" r="7.25" stroke="currentColor" strokeWidth="1.5" />
      <path
        d="M2.8 12.4c2.3-1.6 4.6-1.9 6.9-1 2.2.9 4.5.6 6.8-1"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <circle cx="14.5" cy="6.1" r="1.6" fill="var(--accent)" />
    </svg>
  );
}
