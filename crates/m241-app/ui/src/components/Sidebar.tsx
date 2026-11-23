import { Cpu, Route, SlidersHorizontal, Usb } from "lucide-react";
import type { ComponentType } from "react";

import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { DeviceSummary } from "./DeviceSummary";

type Item = {
  id: string;
  label: string;
  Icon: ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
  phase: string;
  active: boolean;
};

/*
 * The four screens the toolkit will have. The three that are not built yet stay
 * visible with the phase they arrive in, because the rail is the map of the
 * tool, not a menu of what happens to exist today. `phase` is why the row is
 * greyed out, so it is stated instead of hidden in a tooltip.
 */
const ITEMS: readonly Item[] = [
  { id: "connect", label: "接続", Icon: Usb, phase: "", active: true },
  { id: "log", label: "ログ", Icon: Route, phase: "P2", active: false },
  { id: "firmware", label: "ファーム", Icon: Cpu, phase: "P3", active: false },
  { id: "settings", label: "設定", Icon: SlidersHorizontal, phase: "P4", active: false },
];

export function Sidebar({ controller }: { controller: DeviceProbeController }) {
  return (
    <aside className="flex w-62 shrink-0 flex-col border-r border-hair px-4 py-4">
      <p className="px-2 text-xs text-ink-3">画面</p>
      <nav aria-label="画面" className="mt-2">
        <ul className="flex flex-col gap-1">
          {ITEMS.map(({ id, label, Icon, phase, active }) => (
            <li key={id}>
              <div
                aria-current={active ? "page" : undefined}
                aria-disabled={active ? undefined : "true"}
                className={
                  active
                    ? "relative flex h-9 items-center gap-2 rounded-control bg-card px-2 text-sm font-medium text-ink"
                    : "flex h-9 items-center gap-2 rounded-control px-2 text-sm text-ink-3"
                }
              >
                {active ? <span aria-hidden="true" className="trail-mark" /> : null}
                <Icon
                  size={16}
                  strokeWidth={1.5}
                  className={active ? "shrink-0 text-accent" : "shrink-0 text-ink-3"}
                />
                <span className="truncate">{label}</span>
                {phase === "" ? null : (
                  <span className="mono ml-auto shrink-0 text-xs text-ink-3">{phase}</span>
                )}
              </div>
            </li>
          ))}
        </ul>
      </nav>

      <DeviceSummary controller={controller} />
    </aside>
  );
}
