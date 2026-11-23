import { TriangleAlert } from "lucide-react";

import type { ProbeState } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";
import { formatBytes, formatCount, loggingLabel } from "../lib/format";
import type { DeviceInfo } from "../lib/ipc";
import { SAMPLE } from "../lib/sample";

const STEPS = [
  "USB ケーブルで端末をつなぐ",
  "端末の電源を入れる",
  "ポートを選んで端末情報を読む",
] as const;

const REQUESTS = [
  ["$PMTK605", "ファーム・モデル"],
  ["$PMTK182,2,8", "ログ使用量"],
  ["$PMTK182,2,1", "記録の有無"],
] as const;

export function DeviceCard({
  state,
  sample,
  onToggleSample,
}: {
  state: ProbeState;
  sample: boolean;
  onToggleSample: () => void;
}) {
  const connected = state.kind === "connected";

  return (
    <section aria-labelledby="device-title" className="card flex flex-col p-5">
      <div>
        <h2 id="device-title" className="text-md font-medium text-ink">
          端末の情報
        </h2>
        <p className="mt-1 text-xs text-ink-3">接続後に読み取った値</p>
      </div>

      <div className="mt-4">
        {sample ? (
          <SampleBody onClose={onToggleSample} />
        ) : connected ? (
          <ConnectedBody info={state.info} />
        ) : state.kind === "probing" ? (
          <ProbingBody port={state.port} />
        ) : state.kind === "error" ? (
          <FailedBody message={state.message} hint={state.hint} />
        ) : (
          <IdleBody onSample={onToggleSample} />
        )}
      </div>
    </section>
  );
}

function IdleBody({ onSample }: { onSample: () => void }) {
  return (
    <div>
      <ol className="grid grid-cols-[1rem_1fr] items-baseline gap-x-3 gap-y-2">
        {STEPS.map((step, index) => (
          <li key={step} className="contents">
            <span className="mono text-xs text-ink-3">{index + 1}</span>
            <span className="text-sm text-ink-2">{step}</span>
          </li>
        ))}
      </ol>

      <div className="mt-4 rounded-control bg-inset p-3">
        <p className="text-xs text-ink-3">送る要求</p>
        <ul className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
          {REQUESTS.map(([sentence, meaning]) => (
            <li key={sentence} className="contents">
              <span className="mono text-xs text-ink-2">{sentence}</span>
              <span className="text-xs text-ink-3">{meaning}</span>
            </li>
          ))}
        </ul>
      </div>

      <button type="button" className="btn-quiet -ml-2 mt-2" onClick={onSample}>
        見本で試す
      </button>
    </div>
  );
}

function ProbingBody({ port }: { port: string }) {
  return (
    <div aria-busy="true">
      <div role="status" className="flex items-center gap-2 text-sm text-ink">
        <span aria-hidden="true" className="size-1.5 rounded-full bg-accent" />
        読み取り中
      </div>
      <p className="mono mt-1 text-xs text-ink-3">{port}</p>

      <div aria-hidden="true" className="mt-4 flex flex-col gap-2">
        {[0, 1, 2, 3, 4].map((row) => (
          <div key={row} className="flex items-center justify-between gap-4 py-1">
            <span className="h-3 w-24 rounded-tag bg-raise" />
            <span className="h-3 w-28 rounded-tag bg-raise" />
          </div>
        ))}
      </div>

      <p className="mt-4 text-xs text-ink-3">応答がない場合は 10 秒ほど待ちます</p>
    </div>
  );
}

function FailedBody({ message, hint }: { message: string; hint: string | null }) {
  return (
    <div role="alert">
      <p className="flex items-center gap-2 text-sm font-medium text-ink">
        <TriangleAlert size={16} strokeWidth={1.5} aria-hidden="true" className="text-error" />
        読み取れません
      </p>
      <div className="mt-3 rounded-control bg-error-wash p-3">
        <p className="text-xs text-ink-3">端末 / OS の応答</p>
        <p className="mt-1 text-sm break-words text-ink">{message}</p>
      </div>
      {hint === null ? null : (
        <div className="mt-3">
          <p className="text-xs text-ink-3">対処</p>
          <p className="mt-1 text-sm text-ink-2">{hint}</p>
        </div>
      )}
    </div>
  );
}

function ConnectedBody({ info }: { info: DeviceInfo }) {
  return (
    <div>
      <div role="status" className="flex items-center gap-2 text-sm text-ink">
        <span aria-hidden="true" className="size-1.5 rounded-full bg-accent" />
        接続済み
      </div>
      <p className="mono mt-1 text-xs text-ink-3">{info.port}</p>

      {info.knownRelease ? null : (
        <div className="mt-3 rounded-control bg-inset p-3">
          <p className="text-xs font-medium text-ink-2">未知のリリース</p>
          <p className="mt-1 text-xs text-ink-3">
            ファーム画面（P3）で吸い出して確認します
          </p>
        </div>
      )}

      <Figures info={info} />
    </div>
  );
}

function SampleBody({ onClose }: { onClose: () => void }) {
  return (
    <div>
      <div className="flex items-center justify-between gap-3">
        <p className="text-xs text-ink-3">実機から読んだ値ではありません</p>
        <span className="tag shrink-0">見本</span>
      </div>
      <Figures info={SAMPLE} />
      <button type="button" className="btn-quiet -ml-2 mt-2" onClick={onClose}>
        見本を閉じる
      </button>
    </div>
  );
}

function Figures({ info }: { info: DeviceInfo }) {
  // Only what this read actually returned. The flash part is not read here, so
  // the card does not claim a capacity the probe did not measure.
  return (
    <dl className="mt-2 flex flex-col">
      <Figure label="ファーム" value={info.release} />
      <Figure label="ビルド" value={info.build === "" ? "—" : info.build} />
      <Figure label="モデル" value={info.model === "" ? "—" : info.model} />
      <Figure label="SDK" value={info.sdk ?? "—"} />
      <Figure
        label="ログ使用量"
        value={formatBytes(info.logUsedBytes)}
        note={`${formatCount(info.logUsedBytes)} B`}
      />
      <Figure label="記録" value={loggingLabel(info.logEnabled)} mono={false} />
    </dl>
  );
}

function Figure({
  label,
  value,
  note,
  mono = true,
}: {
  label: string;
  value: string;
  note?: string;
  mono?: boolean;
}) {
  return (
    <div className="figure-row">
      <dt className="shrink-0 text-sm text-ink-2">{label}</dt>
      <dd className="min-w-0 text-right">
        <span className={cn("block text-sm text-ink", mono && "mono")}>{value}</span>
        {note === undefined ? null : (
          <span className="mono mt-0.5 block text-xs text-ink-3">{note}</span>
        )}
      </dd>
    </div>
  );
}
