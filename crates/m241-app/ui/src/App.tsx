import { useState } from "react";

import { Header } from "./components/Header";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { useDeviceProbe } from "./hooks/useDeviceProbe";
import { useLogDocument } from "./hooks/useLogDocument";
import { ConnectScreen } from "./screens/ConnectScreen";
import { LogScreen } from "./screens/LogScreen";

export default function App() {
  const controller = useDeviceProbe();
  // The loaded document lives above the screens so switching to 接続 and back
  // keeps the track, its photos and the camera position.
  const log = useLogDocument();
  const [screen, setScreen] = useState<ScreenId>("connect");

  return (
    <div className="flex h-dvh flex-col bg-paper text-ink">
      {/* The header floats over the map so the map can own the full window
          height; the screens that are not a map pad their own content below it. */}
      <Header controller={controller} />
      <div className="flex min-h-0 flex-1">
        <Sidebar
          controller={controller}
          current={screen}
          available={["connect", "log"]}
          onSelect={setScreen}
        />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          {screen === "connect" ? (
            <div className="flex min-h-0 flex-1 flex-col pt-14">
              <ConnectScreen controller={controller} onOpenLog={() => setScreen("log")} />
            </div>
          ) : (
            <LogScreen log={log} />
          )}
        </main>
      </div>
    </div>
  );
}
