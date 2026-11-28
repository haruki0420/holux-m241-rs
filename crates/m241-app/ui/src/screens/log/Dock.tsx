import { useState } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { cn } from "../../lib/cn";
import { ChartPanel } from "./ChartPanel";
import { PhotoPanels } from "./PhotoPanels";
import { SummaryPanel } from "./SummaryPanel";
import { TrackPointsPanel } from "./TrackPointsPanel";

type DockTabId = "summary" | "chart" | "points" | "photos";

const DOCK_TABS: ReadonlyArray<{ id: DockTabId; label: string }> = [
  { id: "summary", label: "概要・日別" },
  { id: "chart", label: "グラフ" },
  { id: "points", label: "トラック点" },
  { id: "photos", label: "写真" },
];

/**
 * Right-side detail panel for the Log screen.
 */
export function Dock({
  log,
  onNavigateGeotag,
  onNavigateFirmware,
}: {
  log: LogController;
  onNavigateGeotag: () => void;
  onNavigateFirmware: () => void;
}) {
  const [tab, setTab] = useState<DockTabId>("summary");

  return (
    <aside className="flex w-[360px] shrink-0 flex-col border-l border-[var(--dads-border-control)] bg-white">
      <div
        role="tablist"
        aria-label="ログ詳細パネル"
        className="flex border-b border-[var(--dads-border-divider)] bg-[var(--dads-surface-50)] px-2 pt-1"
      >
        {DOCK_TABS.map((item) => {
          const active = item.id === tab;
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setTab(item.id)}
              className={cn(
                "flex-1 border-b-[3px] px-2 py-2 text-[12px] whitespace-nowrap transition-colors cursor-pointer",
                active
                  ? "border-[var(--dads-blue)] bg-white font-bold text-[var(--dads-blue)]"
                  : "border-transparent font-medium text-[var(--dads-ink-sub)] hover:text-[var(--dads-ink)]",
              )}
            >
              {item.label}
            </button>
          );
        })}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {tab === "summary" ? (
          <SummaryPanel
            log={log}
            onNavigateGeotag={onNavigateGeotag}
            onNavigateFirmware={onNavigateFirmware}
          />
        ) : null}
        {tab === "chart" ? <ChartPanel log={log} /> : null}
        {tab === "points" ? <TrackPointsPanel log={log} /> : null}
        {tab === "photos" ? <PhotoPanels log={log} /> : null}
      </div>
    </aside>
  );
}
