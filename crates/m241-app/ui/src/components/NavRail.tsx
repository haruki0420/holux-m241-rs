import type { ComponentType, SVGProps } from "react";

import { ConnectIcon, FirmwareIcon, LogIcon, SettingsIcon } from "./Icons";

type NavItem = {
  id: string;
  label: string;
  Icon: ComponentType<SVGProps<SVGSVGElement>>;
  enabled: boolean;
};

// The four screens the toolkit has. Only Connect exists in P0; the other three
// are shown disabled rather than hidden so the rail is the real map of the app
// and the phase they arrive in is visible on hover.
const ITEMS: readonly NavItem[] = [
  { id: "connect", label: "接続", Icon: ConnectIcon, enabled: true },
  { id: "log", label: "ログ", Icon: LogIcon, enabled: false },
  { id: "firmware", label: "ファーム", Icon: FirmwareIcon, enabled: false },
  { id: "settings", label: "設定", Icon: SettingsIcon, enabled: false },
];

const PHASE: Record<string, string> = {
  log: "ログ画面は P2 で追加します。",
  firmware: "ファーム画面は P3 で追加します。",
  settings: "設定画面は P4 で追加します。",
};

export function NavRail() {
  return (
    <nav
      aria-label="画面"
      className="flex w-16 shrink-0 flex-col gap-2 border-r border-line bg-surface px-1 py-2"
    >
      {ITEMS.map(({ id, label, Icon, enabled }) => {
        const active = id === "connect";
        return (
          <button
            key={id}
            type="button"
            disabled={!enabled}
            aria-current={active ? "page" : undefined}
            title={enabled ? label : PHASE[id]}
            className={[
              "flex h-14 w-14 flex-col items-center justify-center gap-1 rounded-card transition-colors",
              active
                ? "bg-accent-soft text-accent"
                : enabled
                  ? "text-ink-2 hover:bg-surface-2 hover:text-ink"
                  : "cursor-not-allowed text-ink-3",
            ].join(" ")}
          >
            <Icon />
            <span className="text-caption font-semibold">{label}</span>
          </button>
        );
      })}
    </nav>
  );
}
