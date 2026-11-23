import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { DeviceCard } from "./DeviceCard";
import { PortCard } from "./PortCard";

export function ConnectScreen({ controller }: { controller: DeviceProbeController }) {
  return (
    <div className="mx-auto flex w-full max-w-[1184px] flex-col gap-6 px-6 py-6">
      <header>
        <h1 className="text-title font-semibold text-ink">端末に接続</h1>
        <p className="mt-2 max-w-[720px] text-ui text-ink-2">
          M-241 を USB ケーブルでつなぎ、ポートを選んで端末情報を読み取ります。この画面は読み取りのみで、ファームの書き込み・ログの消去・記録の停止は行いません。
        </p>
      </header>

      <div className="grid grid-cols-[minmax(0,1.1fr)_minmax(0,1fr)] gap-6">
        <PortCard controller={controller} />
        <DeviceCard
          state={controller.state}
          sample={controller.sample}
          onToggleSample={controller.toggleSample}
        />
      </div>

      <p className="text-caption text-ink-3">
        書き込みを伴う画面は、後のフェーズで明示的な確認とともに追加します。旧 UI はパリティに達するまで残しています。
      </p>
    </div>
  );
}
