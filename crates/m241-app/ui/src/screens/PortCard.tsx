import { useState } from "react";

import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import type { PortInfo } from "../lib/ipc";

const KIND_LABEL: Record<string, string> = {
  usb: "USB",
  pci: "PCI",
  bluetooth: "Bluetooth",
  unknown: "—",
};

export function PortCard({ controller }: { controller: DeviceProbeController }) {
  const {
    ports,
    portsLoading,
    refresh,
    selectedPort,
    selectPort,
    manualPort,
    setManualPort,
    state,
    connect,
  } = controller;
  const probing = state.kind === "probing";
  // A Linux desktop exposes a dozen onboard `ttyS*` ports that no logger can be
  // on. Listing them by default pushes the connect button below the fold, so the
  // picker starts with USB ports and known cables and offers the rest on demand.
  const [showAll, setShowAll] = useState(false);
  const candidates = ports.filter((port) => port.likely || port.kind === "usb");
  const visible = showAll || candidates.length === 0 ? ports : candidates;
  const hidden = ports.length - visible.length;
  const selectedHidden =
    selectedPort !== "" && !visible.some((port) => port.name === selectedPort);

  return (
    <section aria-labelledby="port-card-title" className="card flex flex-col p-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 id="port-card-title" className="card-title">
            ポートを選ぶ
          </h2>
          <p className="card-note mt-1">端末がつながっているシリアルポートを選びます。</p>
        </div>
        <button
          type="button"
          className="btn-quiet shrink-0"
          onClick={() => void refresh()}
          disabled={portsLoading || probing}
        >
          再読込
        </button>
      </div>

      <div
        role="radiogroup"
        aria-label="シリアルポート"
        className="mt-4 flex flex-col gap-2"
      >
        {portsLoading ? (
          <PortListSkeleton />
        ) : ports.length === 0 ? (
          <div className="well">
            <p className="text-ui font-semibold text-ink">シリアルポートが見つかりません。</p>
            <p className="mt-2 text-caption text-ink-2">
              USB ケーブルを挿し、端末の電源を入れてから「再読込」を押してください。ポート名が分かる場合は、下の欄に直接入力できます。
            </p>
          </div>
        ) : (
          visible.map((port) => (
            <PortRow
              key={port.name}
              port={port}
              selected={manualPort === "" && selectedPort === port.name}
              disabled={probing}
              onSelect={() => selectPort(port.name)}
            />
          ))
        )}
      </div>

      {portsLoading || ports.length === 0 ? null : (
        <>
          {selectedHidden ? (
            <p className="mt-2 text-caption text-ink-3">
              選択中の {selectedPort} は絞り込みの外です。「すべて表示」で確認できます。
            </p>
          ) : null}
          {hidden > 0 ? (
            <button
              type="button"
              className="btn-quiet mt-2 self-start"
              onClick={() => setShowAll((current) => !current)}
            >
              {showAll ? "候補だけ表示" : `すべてのポートを表示（残り ${hidden} 件）`}
            </button>
          ) : null}
        </>
      )}

      <label className="mt-4 block">
        <span className="text-caption font-semibold text-ink-2">ポート名を直接入力</span>
        <input
          type="text"
          className="field mt-2"
          value={manualPort}
          onChange={(event) => setManualPort(event.target.value)}
          placeholder="/dev/ttyUSB0"
          spellCheck={false}
          autoComplete="off"
          disabled={probing}
        />
      </label>

      <div className="mt-auto pt-6">
        <button
          type="button"
          className="btn btn-primary w-full"
          onClick={() => void connect()}
          disabled={probing}
        >
          {probing ? "確認中…" : "接続して確認"}
        </button>
        <p className="card-note mt-2">
          読み取りのみ。書き込み・消去・記録の停止は行いません。
        </p>
        {state.kind === "error" ? (
          <p role="alert" className="mt-2 text-caption font-semibold text-error">
            接続できませんでした。右の「端末の情報」に対処を表示しています。
          </p>
        ) : null}
      </div>
    </section>
  );
}

function PortRow({
  port,
  selected,
  disabled,
  onSelect,
}: {
  port: PortInfo;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
}) {
  const kind = KIND_LABEL[port.kind] ?? port.kind;
  return (
    <label
      className={[
        "flex cursor-pointer items-center gap-2 rounded-card border px-2 py-2 transition-colors",
        "has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent/40",
        selected
          ? "border-accent-line bg-accent-soft"
          : "border-transparent hover:bg-surface-2",
        disabled ? "cursor-not-allowed opacity-60" : "",
      ].join(" ")}
    >
      <input
        type="radio"
        name="serial-port"
        value={port.name}
        checked={selected}
        onChange={onSelect}
        disabled={disabled}
        className="sr-only"
      />
      <span
        aria-hidden="true"
        className={`grid h-4 w-4 shrink-0 place-items-center rounded-full border ${
          selected ? "border-accent" : "border-line-strong"
        }`}
      >
        {selected ? <span className="h-2 w-2 rounded-full bg-accent" /> : null}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-ui font-semibold text-ink">{port.name}</span>
        {port.description ? (
          <span className="block truncate text-caption text-ink-3">{port.description}</span>
        ) : null}
      </span>
      <span className="shrink-0 text-caption text-ink-3">{kind}</span>
      {port.likely ? (
        <span className="chip shrink-0 border-accent-line bg-surface text-accent">候補</span>
      ) : null}
    </label>
  );
}

function PortListSkeleton() {
  return (
    <div aria-hidden="true" className="flex flex-col gap-2">
      {[0, 1].map((row) => (
        <div key={row} className="flex items-center gap-2 rounded-card px-2 py-2">
          <span className="h-4 w-4 rounded-full bg-surface-3" />
          <span className="h-3 w-32 animate-pulse rounded-full bg-surface-3" />
          <span className="ml-auto h-3 w-10 animate-pulse rounded-full bg-surface-3" />
        </div>
      ))}
    </div>
  );
}
