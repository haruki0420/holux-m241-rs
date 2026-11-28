import { ChevronDown, RefreshCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { ConnectionTone, DeviceProbeController } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";
import { formatBytes, formatCount, loggingLabel } from "../lib/format";

export type ScreenTabId = "log" | "geotag" | "firmware";

const TABS: ReadonlyArray<{ id: ScreenTabId; label: string }> = [
  { id: "log", label: "ログ" },
  { id: "geotag", label: "ジオタグ" },
  { id: "firmware", label: "ファームウェア" },
];

const STATUS_LABEL: Record<ConnectionTone, string> = {
  idle: "未接続",
  probing: "確認中",
  connected: "接続済み",
  error: "接続失敗",
};

const STATUS_DOT: Record<ConnectionTone, string> = {
  idle: "bg-[var(--dads-ink-meta)]",
  probing: "bg-[var(--dads-blue)]",
  connected: "bg-[var(--dads-success)]",
  error: "bg-[var(--dads-error)]",
};

/**
 * Global 56px DADS header:
 * - Left: brand mark + 3 top-level underline tabs (ログ / ジオタグ / ファームウェア)
 * - Right: serial port dropdown + manual port toggle + rescan + device_info check + status badge
 */
export function Header({
  controller,
  currentTab,
  onSelectTab,
}: {
  controller: DeviceProbeController;
  currentTab: ScreenTabId;
  onSelectTab: (tab: ScreenTabId) => void;
}) {
  const {
    ports,
    portsLoading,
    refresh,
    selectedPort,
    selectPort,
    manualMode,
    toggleManualMode,
    manualPort,
    setManualPort,
    activePort,
    state,
    connect,
    tone,
  } = controller;

  const [popoverOpen, setPopoverOpen] = useState(false);
  const popoverRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!popoverOpen) {
      return;
    }
    const handleOutside = (event: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(event.target as Node)) {
        setPopoverOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setPopoverOpen(false);
      }
    };
    document.addEventListener("mousedown", handleOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [popoverOpen]);

  return (
    <header className="z-header flex h-14 shrink-0 items-center justify-between gap-3 border-b border-[var(--dads-border-control)] bg-white px-4">
      {/* Left: Brand + 3 top-level tabs */}
      <div className="flex h-full min-w-0 items-center gap-4">
        <div className="flex shrink-0 items-center gap-2">
          <Mark />
          <span className="hidden text-[15px] font-bold text-[var(--dads-ink)] sm:inline">
            Holux M-241
          </span>
        </div>

        <nav
          role="tablist"
          aria-label="画面切替"
          className="flex h-full items-stretch gap-1"
        >
          {TABS.map((tab) => {
            const active = tab.id === currentTab;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => onSelectTab(tab.id)}
                className={cn(
                  "flex items-center border-b-[3px] px-3.5 text-[14px] whitespace-nowrap transition-colors cursor-pointer",
                  active
                    ? "border-[var(--dads-blue)] font-bold text-[var(--dads-blue)]"
                    : "border-transparent font-medium text-[var(--dads-ink-sub)] hover:text-[var(--dads-blue)]",
                )}
              >
                {tab.label}
              </button>
            );
          })}
        </nav>
      </div>

      {/* Right: Serial port picker / manual input + rescan + probe + status */}
      <div className="flex min-w-0 items-center gap-1.5">
        {manualMode ? (
          <input
            type="text"
            aria-label="シリアルポートを手動入力"
            placeholder="/dev/rfcomm0 または COM3"
            value={manualPort}
            onChange={(event) => setManualPort(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void connect();
              }
            }}
            className="dads-field mono h-8 w-44 text-[12px]"
          />
        ) : (
          <select
            aria-label="シリアルポート選択"
            value={selectedPort}
            onChange={(event) => selectPort(event.target.value)}
            disabled={portsLoading && ports.length === 0}
            className="dads-field mono h-8 w-44 text-[12px]"
          >
            {ports.length === 0 ? (
              <option value="">
                {portsLoading ? "ポート検索中…" : "(ポート未検出)"}
              </option>
            ) : (
              ports.map((port) => (
                <option key={port.name} value={port.name}>
                  {port.likely ? `★ ${port.name}` : port.name}
                  {port.description ? ` (${port.description})` : ""}
                </option>
              ))
            )}
          </select>
        )}

        <button
          type="button"
          className="dads-btn"
          data-variant="neutral"
          data-size="xs"
          onClick={toggleManualMode}
          title={manualMode ? "検出されたポート一覧に切り替え" : "Bluetooth SPP 等の手動ポート入力に切り替え"}
        >
          {manualMode ? "一覧" : "手動入力"}
        </button>

        <button
          type="button"
          className="dads-btn"
          data-variant="outline"
          data-size="xs"
          onClick={() => void refresh()}
          disabled={portsLoading}
          aria-label="シリアルポートを再スキャン"
          title="シリアルポートを再スキャン"
        >
          <RefreshCw
            size={13}
            strokeWidth={2}
            aria-hidden="true"
            className={portsLoading ? "animate-spin" : ""}
          />
          <span className="hidden md:inline">再スキャン</span>
        </button>

        <button
          type="button"
          className="dads-btn"
          data-variant="solid-fill"
          data-size="xs"
          onClick={() => void connect()}
          disabled={state.kind === "probing" || activePort === ""}
        >
          {state.kind === "probing" ? "確認中…" : "接続確認"}
        </button>

        {/* Connection status badge + details popover */}
        <div className="relative" ref={popoverRef}>
          <button
            type="button"
            className="dads-btn"
            data-variant="neutral"
            data-size="xs"
            aria-expanded={popoverOpen}
            aria-haspopup="dialog"
            onClick={() => setPopoverOpen((open) => !open)}
          >
            <span
              aria-hidden="true"
              className={cn("size-2.5 shrink-0 rounded-full", STATUS_DOT[tone])}
            />
            <span
              className={cn(
                "font-bold",
                tone === "error"
                  ? "text-[var(--dads-error)]"
                  : tone === "connected"
                    ? "text-[var(--dads-success)]"
                    : "text-[var(--dads-ink)]",
              )}
            >
              {STATUS_LABEL[tone]}
            </span>
            {state.kind === "connected" ? (
              <span className="mono hidden text-[11px] text-[var(--dads-ink-sub)] lg:inline">
                {state.info.port}
              </span>
            ) : null}
            <ChevronDown size={13} strokeWidth={2} aria-hidden="true" />
          </button>

          {popoverOpen ? (
            <div
              role="dialog"
              aria-label="接続端末の状態"
              className="absolute right-0 mt-2 w-80 space-y-3 rounded-lg border-2 border-[var(--dads-ink)] bg-white p-4 text-[13px] shadow-xl z-50"
            >
              <div className="flex items-center justify-between border-b border-[var(--dads-border-divider)] pb-2">
                <span className="text-[14px] font-bold">接続端末の状態</span>
                <span className="inline-flex items-center gap-1.5 text-[12px] font-bold">
                  <span
                    aria-hidden="true"
                    className={cn("size-2 rounded-full", STATUS_DOT[tone])}
                  />
                  <span>{STATUS_LABEL[tone]}</span>
                </span>
              </div>

              {state.kind === "connected" ? (
                <dl className="space-y-1.5 tabular-nums">
                  <InfoRow label="ポート" value={state.info.port} />
                  <InfoRow label="モデル" value={state.info.model} />
                  <InfoRow
                    label="ファームウェア"
                    value={`${state.info.release} (${state.info.build})`}
                  />
                  <InfoRow
                    label="SDK / 既知リリース"
                    value={`${state.info.sdk ?? "—"} / ${state.info.knownRelease ? "既知" : "未登録"}`}
                  />
                  <InfoRow
                    label="ログ記録"
                    value={loggingLabel(state.info.logEnabled)}
                  />
                  <InfoRow
                    label="ログ使用量"
                    value={`${formatBytes(state.info.logUsedBytes)} (${formatCount(state.info.logUsedBytes)} B)`}
                  />
                </dl>
              ) : state.kind === "error" ? (
                <div className="space-y-2">
                  <div className="rounded border border-[var(--dads-error)] bg-[var(--dads-error-bg)] p-2.5 text-[12px]">
                    <p className="font-bold text-[var(--dads-error)]">{state.message}</p>
                    {state.hint ? (
                      <p className="mt-1 text-[var(--dads-ink)]">{state.hint}</p>
                    ) : null}
                  </div>
                </div>
              ) : (
                <p className="text-[12px] leading-relaxed text-[var(--dads-ink-sub)]">
                  ポートを選択（または Bluetooth SPP の `/dev/rfcomm0` 等を手動入力）し、「接続確認」を押すと端末情報を読み取ります。
                </p>
              )}
            </div>
          ) : null}
        </div>
      </div>
    </header>
  );
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="shrink-0 text-[var(--dads-ink-sub)]">{label}</dt>
      <dd className="mono min-w-0 truncate text-right font-bold text-[var(--dads-ink)]">
        {value}
      </dd>
    </div>
  );
}

function Mark() {
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
      className="shrink-0 text-[var(--dads-ink)]"
    >
      <circle cx="10" cy="10" r="7.25" stroke="currentColor" strokeWidth="1.5" />
      <path
        d="M2.8 12.4c2.3-1.6 4.6-1.9 6.9-1 2.2.9 4.5.6 6.8-1"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <circle cx="14.5" cy="6.1" r="1.6" fill="var(--dads-blue)" />
    </svg>
  );
}
