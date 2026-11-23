import { ChartLine, Film, Image, LayoutGrid, Route } from "lucide-react";
import { useRef, useState, type ComponentType } from "react";

import type { LogController } from "../../hooks/useLogDocument";
import { ChartPanel } from "./ChartPanel";
import { MediaPanel } from "./MediaPanel";
import { PhotoListPanel, PhotoViewPanel } from "./PhotoPanels";
import { TrackListPanel } from "./TrackListPanel";

type PanelId = "tracks" | "chart" | "photo" | "photo-list" | "media";

type Tab = {
  id: PanelId;
  label: string;
  Icon: ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
};

/*
 * The five panels the reference tool puts in its dock, kept as five tabs in the
 * same order. The strip is icon-only because five labels do not fit a 272px
 * dock; the panel heading under the strip carries the name in words, and each
 * tab has an accessible name, so nothing is only an icon.
 */
const TABS: readonly Tab[] = [
  { id: "tracks", label: "トラック一覧", Icon: Route },
  { id: "chart", label: "速度・高度", Icon: ChartLine },
  { id: "photo", label: "写真", Icon: Image },
  { id: "photo-list", label: "写真一覧", Icon: LayoutGrid },
  { id: "media", label: "メディア", Icon: Film },
];

/** The right dock: five panels over the remaining third of the screen. */
export function Dock({ log }: { log: LogController }) {
  const [panel, setPanel] = useState<PanelId>("tracks");
  const buttons = useRef<Array<HTMLButtonElement | null>>([]);
  const activeTab = TABS.find((tab) => tab.id === panel) ?? TABS[0];

  const moveFocus = (from: number, delta: number) => {
    const next = (from + delta + TABS.length) % TABS.length;
    const tab = TABS[next];
    if (tab === undefined) {
      return;
    }
    setPanel(tab.id);
    buttons.current[next]?.focus();
  };

  return (
    <aside className="flex w-[272px] shrink-0 flex-col border-l border-hair bg-card">
      <div
        role="tablist"
        aria-label="ログのパネル"
        aria-orientation="horizontal"
        className="flex gap-1 border-b border-hair px-2 py-2"
        onKeyDown={(event) => {
          const index = TABS.findIndex((tab) => tab.id === panel);
          if (event.key === "ArrowRight") {
            moveFocus(index, 1);
            event.preventDefault();
          } else if (event.key === "ArrowLeft") {
            moveFocus(index, -1);
            event.preventDefault();
          } else if (event.key === "Home") {
            moveFocus(0, 0);
            event.preventDefault();
          } else if (event.key === "End") {
            moveFocus(TABS.length - 1, 0);
            event.preventDefault();
          }
        }}
      >
        {TABS.map(({ id, label, Icon }, index) => {
          const active = id === panel;
          return (
            <button
              key={id}
              ref={(node) => {
                buttons.current[index] = node;
              }}
              type="button"
              role="tab"
              id={`dock-tab-${id}`}
              aria-selected={active}
              aria-controls={`dock-panel-${id}`}
              aria-label={label}
              title={label}
              tabIndex={active ? 0 : -1}
              onClick={() => setPanel(id)}
              className={
                active
                  ? "relative flex h-9 w-11 items-center justify-center rounded-control bg-inset text-accent"
                  : "flex h-9 w-11 items-center justify-center rounded-control text-ink-3 hover:bg-inset hover:text-ink"
              }
            >
              {active ? <span aria-hidden="true" className="trail-mark" /> : null}
              <Icon size={18} strokeWidth={1.5} aria-hidden="true" />
            </button>
          );
        })}
      </div>

      <div
        role="tabpanel"
        id={`dock-panel-${panel}`}
        aria-labelledby={`dock-tab-${panel}`}
        tabIndex={0}
        className="flex min-h-0 flex-1 flex-col focus-visible:outline-offset-[-2px]"
      >
        <header className="flex items-baseline justify-between gap-2 px-4 pb-2 pt-3">
          <h2 className="text-sm font-medium text-ink">{activeTab?.label}</h2>
          <p className="mono text-[11px] text-ink-3">{countFor(panel, log)}</p>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
          {panel === "tracks" ? <TrackListPanel log={log} /> : null}
          {panel === "chart" ? <ChartPanel log={log} /> : null}
          {panel === "photo" ? <PhotoViewPanel log={log} /> : null}
          {panel === "photo-list" ? <PhotoListPanel log={log} /> : null}
          {panel === "media" ? <MediaPanel log={log} /> : null}
        </div>
      </div>
    </aside>
  );
}

function countFor(panel: PanelId, log: LogController): string {
  switch (panel) {
    case "tracks":
      return log.summary === null ? "" : `${log.summary.days.length} 日`;
    case "chart":
      return `${log.points.length} 点`;
    case "photo":
    case "photo-list":
      return log.photos.length === 0 ? "" : `${log.photos.length} 枚`;
    case "media":
      return log.media.length === 0 ? "" : `${log.media.length} 件`;
  }
}
