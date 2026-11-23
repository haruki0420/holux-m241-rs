import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { DeviceCard } from "./DeviceCard";
import { MapPanel } from "./MapPanel";
import { PortCard } from "./PortCard";

export function ConnectScreen({ controller }: { controller: DeviceProbeController }) {
  const { state } = controller;
  const stage = state.kind === "connected" ? 2 : state.kind === "probing" ? 1 : 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-6 px-8 py-6">
      <header className="shrink-0">
        <h1 className="text-xl font-medium text-ink">端末に接続</h1>
        <p className="mt-1 text-sm text-ink-2">ポートを選んで端末の情報を読み取ります</p>
      </header>

      <div className="grid shrink-0 grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <PortCard controller={controller} />
        <DeviceCard
          state={controller.state}
          sample={controller.sample}
          onToggleSample={controller.toggleSample}
        />
      </div>

      <MapPanel stage={stage} />
    </div>
  );
}
