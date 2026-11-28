import { useState } from "react";

import { Header, type ScreenTabId } from "./components/Header";
import { useDeviceProbe } from "./hooks/useDeviceProbe";
import { useLogDocument } from "./hooks/useLogDocument";
import { FirmwareScreen } from "./screens/FirmwareScreen";
import { GeotagScreen } from "./screens/GeotagScreen";
import { LogScreen } from "./screens/LogScreen";

export default function App() {
  const device = useDeviceProbe();
  const log = useLogDocument();
  const [tab, setTab] = useState<ScreenTabId>("log");

  return (
    <div className="flex h-dvh flex-col bg-white text-[var(--dads-ink)]">
      <Header controller={device} currentTab={tab} onSelectTab={setTab} />

      <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {tab === "log" ? (
          <LogScreen
            log={log}
            device={device}
            onNavigateGeotag={() => setTab("geotag")}
            onNavigateFirmware={() => setTab("firmware")}
          />
        ) : null}
        {tab === "geotag" ? (
          <GeotagScreen log={log} onNavigateLog={() => setTab("log")} />
        ) : null}
        {tab === "firmware" ? <FirmwareScreen device={device} /> : null}
      </main>
    </div>
  );
}
