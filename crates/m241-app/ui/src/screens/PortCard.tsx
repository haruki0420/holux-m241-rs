import { RotateCw } from "lucide-react";
import { useState } from "react";

import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { cn } from "../lib/cn";
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
    <section aria-labelledby="port-title" className="card flex flex-col p-5">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 id="port-title" className="text-md font-medium text-ink">
            ポート
          </h2>
          <p className="mt-1 text-xs text-ink-3">端末のシリアルポートを選びます</p>
        </div>
        <button
          type="button"
          className="btn-quiet shrink-0"
          onClick={() => void refresh()}
          disabled={portsLoading || probing}
        >
          <RotateCw size={16} strokeWidth={1.5} aria-hidden="true" />
          再読込
        </button>
      </div>

      {/* The list takes the card's spare height so the picker and the manual
          fallback stay one block and the primary action keeps the bottom edge. */}
      <div className="flex flex-1 flex-col items-start">
        <div
          role="radiogroup"
          aria-label="シリアルポート"
          className="mt-4 flex w-full flex-col gap-1"
        >
          {portsLoading ? (
            <PortListSkeleton />
          ) : ports.length === 0 ? (
            <div className="rounded-control bg-inset p-4">
              <p className="text-sm font-medium text-ink">ポートが見つかりません</p>
              <p className="mt-1 text-xs text-ink-2">
                USB ケーブルを挿して、再読込を押します
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

        {hidden > 0 ? (
          <button
            type="button"
            className="btn-quiet mt-1"
            onClick={() => setShowAll((current) => !current)}
          >
            {showAll ? "候補だけ表示" : `ほか ${hidden} 件を表示`}
          </button>
        ) : null}

        {selectedHidden ? (
          <p className="mt-1 text-xs text-ink-3">
            選択中の {selectedPort} は絞り込みの外です
          </p>
        ) : null}
      </div>

      {/* A rule closes the picker, so the spare height the sibling card adds
          lands inside the list area instead of between two form fields. */}
      <div className="mt-3 border-t border-hair pt-4">
        <label className="block">
          <span className="text-xs font-medium text-ink-2">ポート名を入力</span>
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

        <div className="mt-4">
          <button
            type="button"
            className="btn btn-primary w-full"
            onClick={() => void connect()}
            disabled={probing}
          >
            {probing ? "読み取り中…" : "端末情報を読む"}
          </button>
          {state.kind === "error" ? (
            <p className="mt-2 text-xs text-error">読み取れませんでした</p>
          ) : null}
        </div>
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
    <label className="port-row" data-selected={selected} data-disabled={disabled}>
      <input
        type="radio"
        name="serial-port"
        value={port.name}
        checked={selected}
        onChange={onSelect}
        disabled={disabled}
        className="sr-only"
      />
      {selected ? <span aria-hidden="true" className="trail-mark" /> : null}
      <span
        aria-hidden="true"
        className={cn(
          "grid size-3.5 shrink-0 place-items-center rounded-full border",
          selected ? "border-ink" : "border-control",
        )}
      >
        {selected ? <span className="size-1.5 rounded-full bg-ink" /> : null}
      </span>
      <span className="min-w-0 flex-1">
        <span className="mono block truncate text-sm text-ink">{port.name}</span>
        {port.description ? (
          <span className="block truncate text-xs text-ink-3">{port.description}</span>
        ) : null}
      </span>
      <span className="shrink-0 text-xs text-ink-3">{kind}</span>
      {port.likely ? (
        <span className="flex shrink-0 items-center gap-1 text-xs text-ink-2">
          <span aria-hidden="true" className="size-1 rounded-full bg-accent" />
          候補
        </span>
      ) : null}
    </label>
  );
}

/** A structural placeholder: the shape of a row, nothing that moves. */
function PortListSkeleton() {
  return (
    <div aria-hidden="true" className="flex flex-col gap-1">
      {[0, 1].map((row) => (
        <div key={row} className="flex min-h-10 items-center gap-3 px-3">
          <span className="size-3.5 shrink-0 rounded-full bg-raise" />
          <span className="h-3 w-32 rounded-tag bg-raise" />
          <span className="ml-auto h-3 w-8 rounded-tag bg-raise" />
        </div>
      ))}
    </div>
  );
}
