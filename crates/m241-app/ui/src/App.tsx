import { Header } from "./components/Header";
import { NavRail } from "./components/NavRail";
import { StatusBar } from "./components/StatusBar";
import { useDeviceProbe } from "./hooks/useDeviceProbe";
import { ConnectScreen } from "./screens/ConnectScreen";

export default function App() {
  const controller = useDeviceProbe();
  const { state, sample } = controller;

  const status = (() => {
    if (sample) {
      return "見本の値を表示しています（実機から読んだ値ではありません）。";
    }
    switch (state.kind) {
      case "idle":
        return "端末は未接続です。ポートを選んで端末情報を読み取ってください。";
      case "probing":
        return `${state.port} から端末情報を読み取り中…`;
      case "connected":
        return `接続済み · ${state.info.port} · 読み取り専用`;
      case "error":
        return "端末情報を読み取れませんでした。右の「対処」を確認してください。";
    }
  })();

  return (
    <div className="flex h-full flex-col bg-surface-2 text-ink">
      <Header tone={controller.tone} />
      <div className="flex min-h-0 flex-1">
        <NavRail />
        <main className="min-w-0 flex-1 overflow-y-auto">
          <ConnectScreen controller={controller} />
        </main>
      </div>
      <StatusBar text={status} right={sample ? "見本 · 読み取り専用" : "読み取り専用 · P0"} />
    </div>
  );
}
