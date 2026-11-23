import { useState } from "react";

import { Header } from "./components/Header";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { useDeviceProbe } from "./hooks/useDeviceProbe";
import { ConnectScreen } from "./screens/ConnectScreen";

export default function App() {
  const controller = useDeviceProbe();
  const [screen, setScreen] = useState<ScreenId>("connect");

  return (
    <div className="flex h-dvh flex-col bg-paper text-ink">
      <Header controller={controller} />
      <div className="flex min-h-0 flex-1">
        <Sidebar
          controller={controller}
          current={screen}
          available={["connect"]}
          onSelect={setScreen}
        />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
          <ConnectScreen controller={controller} />
        </main>
      </div>
    </div>
  );
}
