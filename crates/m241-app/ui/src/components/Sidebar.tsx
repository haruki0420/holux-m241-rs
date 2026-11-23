import { Cpu, Route, SlidersHorizontal, Usb } from "lucide-react";
import type { ComponentType } from "react";

import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";
import { DeviceSummary } from "./DeviceSummary";

/** The screens the shell knows about. */
export type ScreenId = "connect" | "log" | "firmware" | "settings";

type Item = {
  id: ScreenId;
  label: string;
  Icon: ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
};

/*
 * The rail is the map of the tool, so all four screens stay listed. A screen
 * that is not built yet is a disabled row with one quiet word — not a phase
 * number, which was the previous draft's way of explaining the future to the
 * reader. `available` is what the shell can actually show today.
 */
const ITEMS: readonly Item[] = [
  { id: "connect", label: "接続", Icon: Usb },
  { id: "log", label: "ログ", Icon: Route },
  { id: "firmware", label: "ファーム", Icon: Cpu },
  { id: "settings", label: "設定", Icon: SlidersHorizontal },
];

export function Sidebar({
  controller,
  current,
  available,
  onSelect,
}: {
  controller: DeviceProbeController;
  current: ScreenId;
  available: readonly ScreenId[];
  onSelect: (screen: ScreenId) => void;
}) {
  return (
    <aside className="flex w-56 shrink-0 flex-col border-r border-hair px-4 py-4">
      <p className="px-2 text-xs text-ink-3">画面</p>
      <nav aria-label="画面" className="mt-2">
        <ul className="flex flex-col gap-1">
          {ITEMS.map(({ id, label, Icon }) => {
            const enabled = available.includes(id);
            const active = current === id;
            return (
              <li key={id}>
                <button
                  type="button"
                  aria-current={active ? "page" : undefined}
                  disabled={!enabled}
                  onClick={() => onSelect(id)}
                  className={cn(
                    "relative flex h-9 w-full items-center gap-2 rounded-control px-2 text-left text-sm",
                    active && "bg-card font-medium text-ink",
                    !active && enabled && "text-ink-2 hover:bg-inset hover:text-ink",
                    !enabled && "cursor-default text-ink-3",
                  )}
                >
                  {active ? <span aria-hidden="true" className="trail-mark" /> : null}
                  <Icon
                    size={16}
                    strokeWidth={1.5}
                    className={cn(
                      "shrink-0",
                      active ? "text-accent" : enabled ? "text-ink-2" : "text-ink-3",
                    )}
                  />
                  <span className="truncate">{label}</span>
                  {enabled ? null : (
                    <span className="ml-auto shrink-0 text-xs text-ink-3">準備中</span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      </nav>

      <DeviceSummary controller={controller} />
    </aside>
  );
}
