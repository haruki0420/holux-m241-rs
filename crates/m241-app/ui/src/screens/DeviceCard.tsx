import type { ProbeState } from "../hooks/useDeviceProbe";
import { formatBytes, formatCount, loggingLabel } from "../lib/format";
import type { DeviceInfo } from "../lib/ipc";

/**
 * The sample card, shown only when the operator asks for it.
 *
 * The values are the device facts recorded in the toolkit's documents, labelled
 * as a sample in the UI. Without it the success state could only be inspected
 * with a unit attached, and the P0 review needs to see the layout it will have.
 */
const SAMPLE: DeviceInfo = {
  port: "/dev/ttyUSB0",
  release: "B-core_1.20",
  build: "0043",
  model: "01017-00F",
  sdk: "1.0",
  knownRelease: true,
  logUsedBytes: 212_860,
  logEnabled: true,
};

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
    <section aria-labelledby="device-card-title" className="card flex flex-col p-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 id="device-card-title" className="card-title">
            端末の情報
          </h2>
          <p className="card-note mt-1">読み取った値はこの画面にだけ表示します。</p>
        </div>
        <span className="chip shrink-0 border-line bg-surface-2 text-ink-2">
          読み取りのみ
        </span>
      </div>

      <div className="mt-6 flex-1">
        {sample ? (
          <SampleInfo />
        ) : connected ? (
          <ConnectedInfo info={state.info} />
        ) : state.kind === "probing" ? (
          <ProbingInfo port={state.port} />
        ) : state.kind === "error" ? (
          <FailedInfo message={state.message} hint={state.hint} />
        ) : (
          <IdleInfo />
        )}
      </div>

      {connected ? null : (
        <div className="mt-6 border-t border-line pt-4">
          <button type="button" className="btn-quiet" onClick={onToggleSample}>
            {sample ? "見本を閉じる" : "見本の値を表示（実機の値ではありません）"}
          </button>
        </div>
      )}
    </section>
  );
}

function IdleInfo() {
  return (
    <div>
      <div className="well">
        <ol className="flex flex-col gap-2">
          {[
            "USB ケーブルで端末をつなぐ",
            "端末の電源を入れる",
            "ポートを選んで「接続して確認」を押す",
          ].map((step, index) => (
            <li key={step} className="flex items-center gap-2 text-ui text-ink-2">
              <span
                aria-hidden="true"
                className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-surface-3 text-caption font-semibold text-ink-2"
              >
                {index + 1}
              </span>
              {step}
            </li>
          ))}
        </ol>
      </div>
      <p className="card-note mt-4">
        ファーム・モデル・ログ使用量は、接続後に $PMTK605 と $PMTK182,2,8 から読み取ります。
      </p>
      <div className="mt-6">
        <p className="text-caption font-semibold text-ink-2">この画面が端末に送る要求</p>
        <ul className="mt-2 flex flex-col gap-2">
          {[
            ["$PMTK605", "ファーム・ビルド・モデル（読み取り）"],
            ["$PMTK182,2,8", "ログ使用量（読み取り）"],
            ["$PMTK182,2,1", "記録の有無（読み取り）"],
          ].map(([sentence, meaning]) => (
            <li key={sentence} className="flex items-baseline gap-2 text-caption text-ink-3">
              <span className="font-semibold text-ink-2">{sentence}</span>
              <span>{meaning}</span>
            </li>
          ))}
        </ul>
        <p className="mt-2 text-caption text-ink-3">
          書き込み・消去・記録の停止を送る要求はありません。
        </p>
      </div>
    </div>
  );
}

function ProbingInfo({ port }: { port: string }) {
  return (
    <div>
      <div aria-hidden="true" className="flex flex-col gap-2">
        {[0, 1, 2, 3, 4, 5].map((row) => (
          <div key={row} className="flex items-center justify-between py-2">
            <span className="h-3 w-24 animate-pulse rounded-full bg-surface-3" />
            <span className="h-3 w-32 animate-pulse rounded-full bg-surface-3" />
          </div>
        ))}
      </div>
      <p role="status" className="card-note mt-4">
        {port} に問い合わせています。応答がない場合は最大 10 秒ほどかかります。
      </p>
    </div>
  );
}

function FailedInfo({ message, hint }: { message: string; hint: string | null }) {
  return (
    <div role="alert">
      <p className="text-body font-semibold text-ink">接続できませんでした</p>
      <div className="well mt-4">
        <p className="text-caption font-semibold text-ink-3">端末 / OS の応答</p>
        <p className="mt-2 text-ui text-ink">{message}</p>
      </div>
      {hint ? (
        <div className="mt-4">
          <p className="text-caption font-semibold text-ink-2">対処</p>
          <p className="mt-2 text-ui text-ink-2">{hint}</p>
        </div>
      ) : null}
      <p className="card-note mt-4">
        ポート名とケーブルを確認して、もう一度「接続して確認」を押してください。
      </p>
    </div>
  );
}

function ConnectedInfo({ info }: { info: DeviceInfo }) {
  return (
    <div>
      <div role="status" className="flex items-center gap-2 text-ui font-semibold text-accent">
        <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-accent" />
        接続しました（読み取りのみ）
      </div>
      <p className="card-note mt-1">{info.port}</p>
      {info.knownRelease ? null : (
        <div className="well mt-4">
          <p className="text-caption font-semibold text-ink-2">未知のリリース</p>
          <p className="mt-2 text-caption text-ink-2">
            このビルドが知らないファームウェアです。ファーム画面（P3）で吸い出して確認してください。
          </p>
        </div>
      )}
      <DeviceFigures info={info} />
    </div>
  );
}

function SampleInfo() {
  return (
    <div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="chip border-line bg-surface-3 text-ink-2">見本</span>
        <p className="text-caption text-ink-3">実機から読んだ値ではありません。</p>
      </div>
      <DeviceFigures info={SAMPLE} />
    </div>
  );
}

function DeviceFigures({ info }: { info: DeviceInfo }) {
  return (
    <dl className="mt-4 divide-y divide-line">
      <Figure label="ファーム" value={info.release} />
      <Figure label="ビルド" value={info.build !== "" ? info.build : "—"} />
      <Figure label="モデル" value={info.model !== "" ? info.model : "—"} />
      <Figure label="SDK" value={info.sdk ?? "—"} />
      <Figure
        label="フラッシュ"
        value="512 KiB"
        sub="量産品の既知値（この画面ではチップ ID を読んでいません）"
      />
      <Figure
        label="ログ使用量"
        value={formatBytes(info.logUsedBytes)}
        sub={`${formatCount(info.logUsedBytes)} B`}
      />
      <Figure label="記録" value={loggingLabel(info.logEnabled)} />
    </dl>
  );
}

function Figure({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="figure-row">
      <dt className="figure-label">{label}</dt>
      <dd className="min-w-0 text-right">
        <span className="figure-value block break-all">{value}</span>
        {sub ? <span className="mt-1 block text-caption text-ink-3">{sub}</span> : null}
      </dd>
    </div>
  );
}
