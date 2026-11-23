import type { ConnectionTone } from "../hooks/useDeviceProbe";

const LABEL: Record<ConnectionTone, string> = {
  idle: "未接続",
  probing: "確認中",
  connected: "接続済み",
  error: "未接続",
};

const STYLE: Record<ConnectionTone, string> = {
  idle: "border-line bg-surface-2 text-ink-2",
  probing: "border-accent-line bg-accent-soft text-accent",
  connected: "border-accent-line bg-accent-soft text-accent",
  error: "border-error-line bg-error-soft text-error",
};

const DOT: Record<ConnectionTone, string> = {
  idle: "bg-ink-3",
  probing: "animate-pulse bg-accent",
  connected: "bg-accent",
  error: "bg-error",
};

export function Header({ tone }: { tone: ConnectionTone }) {
  return (
    <header className="flex h-14 shrink-0 items-center justify-between border-b border-line bg-surface px-6">
      <div className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className="grid h-6 w-6 place-items-center rounded-card bg-accent text-caption font-semibold text-white"
        >
          M
        </span>
        <span className="text-body font-semibold">Holux M-241</span>
        <span className="ml-2 text-caption text-ink-3">GPS ロガー・ツールキット</span>
      </div>
      <div
        role="status"
        className={`inline-flex items-center gap-2 rounded-full border px-2 py-1 text-caption font-semibold ${STYLE[tone]}`}
      >
        <span aria-hidden="true" className={`h-1.5 w-1.5 rounded-full ${DOT[tone]}`} />
        {LABEL[tone]}
      </div>
    </header>
  );
}
