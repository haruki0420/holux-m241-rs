import { FolderOpen, RefreshCw, ShieldAlert, Usb } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { DadsBanner } from "../components/DadsBanner";
import type { DeviceProbeController } from "../hooks/useDeviceProbe";
import { pickOpenFile, pickSaveFile } from "../lib/dialog";
import { formatCount, formatHexOffset } from "../lib/format";
import {
  fwDump,
  fwFlash,
  fwInspect,
  fwPatch,
  fwProbeDa,
  splitError,
  type FlashMode,
  type FwDumpOutcome,
  type FwFlashOutcome,
  type FwInspectDto,
  type FwPatchOutcome,
  type FwProbeDaOutcome,
} from "../lib/ipc";

const FLASH_MODES: ReadonlyArray<{
  id: FlashMode;
  label: string;
  desc: string;
}> = [
  {
    id: "strict",
    label: "strict（厳格・推奨）",
    desc: "バックアップと一致、または WNRO パッチ対象オフセットのみ差分を許可します。",
  },
  {
    id: "recover",
    label: "recover（バックアップ復旧）",
    desc: "選択したバックアップとバイト完全一致するイメージでの復旧書き込みを行います。",
  },
  {
    id: "vendor",
    label: "vendor（公式リリース）",
    desc: "公式 V1.13 / V1.12 イメージを書き込みます（個体固有 14 バイトは上書きされます）。",
  },
  {
    id: "unverified",
    label: "unverified（自由イメージ）",
    desc: "物理制約（空でない・偶数長・512 KiB 以下）を満たす任意イメージを書き込みます。",
  },
];

/**
 * Tab 3: ファームウェア (Backup -> Patch / Select Image & DA -> Inspect & Mode -> Flash).
 *
 * All destructive serial protocol work lives strictly in the Rust backend (`m241-flash`).
 * This UI requires manual typed confirmation (`PROBE` for DA probe, `WRITE` for flashing).
 */
export function FirmwareScreen({ device }: { device: DeviceProbeController }) {
  const port = device.activePort;

  // Step 1: Backup
  const [assumeBrom, setAssumeBrom] = useState(false);
  const [backupPath, setBackupPath] = useState("");
  const [dumping, setDumping] = useState(false);
  const [dumpOutcome, setDumpOutcome] = useState<FwDumpOutcome | null>(null);
  const [dumpError, setDumpError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  // Step 2: Patch / Image / DA
  const [patchInputPath, setPatchInputPath] = useState("");
  const [patching, setPatching] = useState(false);
  const [patchOutcome, setPatchOutcome] = useState<FwPatchOutcome | null>(null);
  const [patchError, setPatchError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  const [imagePath, setImagePath] = useState("");
  const [daPath, setDaPath] = useState("");

  // Optional DA probe (PROBE typed guard)
  const [probeConfirmWord, setProbeConfirmWord] = useState("");
  const [probingDa, setProbingDa] = useState(false);
  const [probeOutcome, setProbeOutcome] = useState<FwProbeDaOutcome | null>(null);
  const [probeError, setProbeError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  // Step 3: Inspect & Mode
  const [mode, setMode] = useState<FlashMode>("strict");
  const [inspecting, setInspecting] = useState(false);
  const [inspect, setInspect] = useState<FwInspectDto | null>(null);
  const [inspectError, setInspectError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  // Step 4: Flash (WRITE typed guard)
  const [riskAgreed, setRiskAgreed] = useState(false);
  const [writeConfirmWord, setWriteConfirmWord] = useState("");
  const [flashing, setFlashing] = useState(false);
  const [flashOutcome, setFlashOutcome] = useState<FwFlashOutcome | null>(null);
  const [flashError, setFlashError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  const inspectTicket = useRef(0);

  const runInspect = useCallback(async (img: string, bak: string) => {
    const ticket = inspectTicket.current + 1;
    inspectTicket.current = ticket;
    const trimmedImg = img.trim();
    if (trimmedImg === "") {
      setInspecting(false);
      setInspect(null);
      setInspectError(null);
      return;
    }
    setInspecting(true);
    setInspectError(null);
    try {
      const result = await fwInspect(trimmedImg, bak.trim() === "" ? null : bak.trim());
      if (inspectTicket.current !== ticket) {
        return;
      }
      setInspect(result);
      setMode(result.recommendedMode);
      setRiskAgreed(false);
    } catch (error) {
      if (inspectTicket.current !== ticket) {
        return;
      }
      setInspect(null);
      setInspectError(splitError(error));
    } finally {
      if (inspectTicket.current === ticket) {
        setInspecting(false);
      }
    }
  }, []);

  useEffect(() => {
    void runInspect(imagePath, backupPath);
  }, [imagePath, backupPath, runInspect]);

  const handleDump = async () => {
    if (port === "") {
      setDumpError({
        message: "シリアルポートが選択されていません",
        hint: "上部ヘッダーでポートを選択または手動入力してください。",
      });
      return;
    }
    const stamp = new Date().toISOString().slice(0, 10);
    const destination = await pickSaveFile({
      title: "バックアップの保存先を選択",
      defaultPath: `m241_backup_${stamp}.bin`,
      filters: [{ name: "ファームウェア (.bin)", extensions: ["bin"] }],
    });
    if (!destination) {
      return;
    }
    setDumping(true);
    setDumpError(null);
    setDumpOutcome(null);
    try {
      const result = await fwDump(port, destination, assumeBrom);
      setDumpOutcome(result);
      setBackupPath(result.destination);
      if (patchInputPath === "") {
        setPatchInputPath(result.destination);
      }
    } catch (error) {
      setDumpError(splitError(error));
    } finally {
      setDumping(false);
    }
  };

  const handlePickBackup = async () => {
    const picked = await pickOpenFile({
      title: "既存のバックアップを選択",
      filters: [{ name: "ファームウェア (.bin)", extensions: ["bin"] }],
    });
    if (picked) {
      setBackupPath(picked);
      setDumpOutcome(null);
      setDumpError(null);
      if (patchInputPath === "") {
        setPatchInputPath(picked);
      }
    }
  };

  const handlePickPatchInput = async () => {
    const picked = await pickOpenFile({
      title: "パッチ元のファームウェアを選択",
      filters: [{ name: "ファームウェア (.bin)", extensions: ["bin"] }],
    });
    if (picked) {
      setPatchInputPath(picked);
    }
  };

  const handleRunPatch = async () => {
    const input = (patchInputPath || backupPath).trim();
    if (input === "") {
      setPatchError({
        message: "パッチ元のファイルが選択されていません",
        hint: "先にステップ 1 でバックアップを取得または選択してください。",
      });
      return;
    }
    const defaultOut = input.replace(/\.bin$/i, "") + "_patched_v114.bin";
    const outputPath = await pickSaveFile({
      title: "WNRO パッチ済みイメージの保存先を選択",
      defaultPath: defaultOut,
      filters: [{ name: "ファームウェア (.bin)", extensions: ["bin"] }],
    });
    if (!outputPath) {
      return;
    }
    setPatching(true);
    setPatchError(null);
    setPatchOutcome(null);
    try {
      const result = await fwPatch(input, outputPath);
      setPatchOutcome(result);
      setImagePath(result.outputPath);
    } catch (error) {
      setPatchError(splitError(error));
    } finally {
      setPatching(false);
    }
  };

  const handlePickImage = async () => {
    const picked = await pickOpenFile({
      title: "書き込むファームウェアイメージを選択",
      filters: [{ name: "ファームウェア (.bin)", extensions: ["bin"] }],
    });
    if (picked) {
      setImagePath(picked);
    }
  };

  const handlePickDa = async () => {
    const picked = await pickOpenFile({
      title: "DA バイナリ (DA_v3.8.4.bin) を選択",
      filters: [{ name: "DA バイナリ (.bin)", extensions: ["bin"] }],
    });
    if (picked) {
      setDaPath(picked);
    }
  };

  const handleProbeDa = async () => {
    if (port === "" || daPath.trim() === "" || probeConfirmWord.trim() !== "PROBE") {
      return;
    }
    setProbingDa(true);
    setProbeError(null);
    setProbeOutcome(null);
    try {
      const result = await fwProbeDa(port, daPath.trim(), assumeBrom, probeConfirmWord.trim());
      setProbeOutcome(result);
      setProbeConfirmWord("");
    } catch (error) {
      setProbeError(splitError(error));
    } finally {
      setProbingDa(false);
    }
  };

  const needsRiskAck =
    mode === "vendor" ||
    mode === "unverified" ||
    (inspect !== null && inspect.warnings.length > 0);

  const canFlash =
    !flashing &&
    port !== "" &&
    imagePath.trim() !== "" &&
    backupPath.trim() !== "" &&
    daPath.trim() !== "" &&
    (!needsRiskAck || riskAgreed) &&
    writeConfirmWord.trim() === "WRITE";

  const handleFlash = async () => {
    if (!canFlash) {
      return;
    }
    setFlashing(true);
    setFlashError(null);
    setFlashOutcome(null);
    try {
      const result = await fwFlash({
        port,
        imagePath: imagePath.trim(),
        backupPath: backupPath.trim(),
        daPath: daPath.trim(),
        mode,
        assumeBrom,
        confirmWord: writeConfirmWord.trim(),
      });
      setFlashOutcome(result);
      setWriteConfirmWord("");
    } catch (error) {
      setFlashError(splitError(error));
    } finally {
      setFlashing(false);
    }
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-white">
      <div className="mx-auto max-w-3xl space-y-6 px-6 py-6 text-[14px]">
        {/* Page Heading */}
        <div className="border-b border-[var(--dads-border-divider)] pb-3">
          <h1 className="text-[20px] font-bold text-[var(--dads-ink)]">
            ファームウェア管理と書き込み
          </h1>
          <p className="mt-1 text-[13px] leading-relaxed text-[var(--dads-ink-sub)]">
            バックアップ取得・WNRO パッチ生成・イメージ検査・書き込みおよび全領域読み戻し照合を順番に実施します。
          </p>
        </div>

        {/* Target port status */}
        {port === "" ? (
          <DadsBanner type="warning" title="シリアルポートが未選択です">
            本体からバックアップ取得や書き込みを行うには、上部ヘッダーからポートを選択または手動入力してください。
          </DadsBanner>
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-3.5 py-2.5 text-[13px]">
            <div>
              <span className="font-bold">対象ポート: </span>
              <span className="mono font-bold text-[var(--dads-ink)]">{port}</span>
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-[12px] font-bold">
              <input
                type="checkbox"
                className="dads-checkbox"
                checked={assumeBrom}
                onChange={(event) => setAssumeBrom(event.target.checked)}
              />
              <span>すでに BootROM で待機中として接続する (--assume-brom)</span>
            </label>
          </div>
        )}

        {/* Step 1: Backup */}
        <section
          aria-label="ステップ 1: バックアップ"
          className="space-y-3 border-b border-[var(--dads-border-divider)] pb-6"
        >
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-[16px] font-bold">ステップ 1. バックアップ取得・選択</h2>
            <span
              className={
                backupPath.trim() !== ""
                  ? "rounded border border-[var(--dads-success)] bg-[var(--dads-success-bg)] px-2 py-0.5 text-[12px] font-bold text-[var(--dads-success)]"
                  : "rounded border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-2 py-0.5 text-[12px] font-bold text-[var(--dads-ink-sub)]"
              }
            >
              {backupPath.trim() !== "" ? "選択済み" : "未選択"}
            </span>
          </div>

          <p className="text-[13px] text-[var(--dads-ink-sub)]">
            書き込み前の安全確認と復旧のため、最初に本体のファームウェアバックアップを取得または指定してください。
          </p>

          <div className="flex flex-wrap gap-2.5">
            <button
              type="button"
              className="dads-btn"
              data-variant="solid-fill"
              data-size="sm"
              disabled={dumping || port === ""}
              onClick={() => void handleDump()}
            >
              <Usb size={14} strokeWidth={2} aria-hidden="true" />
              <span>{dumping ? "吸い出し中…" : "本体からバックアップ取得"}</span>
            </button>
            <button
              type="button"
              className="dads-btn"
              data-variant="outline"
              data-size="sm"
              disabled={dumping}
              onClick={() => void handlePickBackup()}
            >
              <FolderOpen size={14} strokeWidth={2} aria-hidden="true" />
              <span>既存バックアップを選択</span>
            </button>
          </div>

          <label className="block space-y-1 text-[12px]">
            <span className="font-bold text-[var(--dads-ink-sub)]">バックアップファイルパス</span>
            <input
              type="text"
              placeholder="/path/to/m241_backup.bin"
              value={backupPath}
              onChange={(event) => setBackupPath(event.target.value)}
              className="dads-field mono w-full"
            />
          </label>

          {dumpOutcome ? (
            <DadsBanner type="success" title="バックアップを取得しました">
              <dl className="space-y-1 text-[12px] tabular-nums">
                <div className="truncate">
                  保存先: <span className="mono font-bold">{dumpOutcome.destination}</span>
                </div>
                <div>
                  サイズ: <span className="mono font-bold">{formatCount(dumpOutcome.bytes)} B</span> ·{" "}
                  プロファイル: <span className="mono font-bold">{dumpOutcome.profile ?? "不明"}</span> ·{" "}
                  WNRO パッチ:{" "}
                  <span className="font-bold">
                    {dumpOutcome.wnroPatched === null
                      ? "不明"
                      : dumpOutcome.wnroPatched
                        ? "適用済み"
                        : "未適用"}
                  </span>
                </div>
                <div className="truncate">
                  SHA-256: <span className="mono">{dumpOutcome.sha256}</span>
                </div>
              </dl>
            </DadsBanner>
          ) : null}

          {dumpError ? (
            <DadsBanner type="error" title="バックアップ取得に失敗しました">
              <p className="font-bold">{dumpError.message}</p>
              {dumpError.hint ? <p className="mt-1">{dumpError.hint}</p> : null}
            </DadsBanner>
          ) : null}
        </section>

        {/* Step 2: Patch / Image / DA */}
        <section
          aria-label="ステップ 2: パッチ作成・イメージ・DA 選択"
          className="space-y-4 border-b border-[var(--dads-border-divider)] pb-6"
        >
          <h2 className="text-[16px] font-bold">
            ステップ 2. イメージ準備と DA ファイル選択
          </h2>

          {/* WNRO Patch box */}
          <div className="space-y-2.5 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-3.5 text-[13px]">
            <div className="font-bold">WNRO パッチ済みイメージ作成 (任意)</div>
            <div className="flex flex-wrap items-center gap-2">
              <input
                type="text"
                aria-label="パッチ元イメージ"
                placeholder="パッチ元バックアップ (.bin)"
                value={patchInputPath || backupPath}
                onChange={(event) => setPatchInputPath(event.target.value)}
                className="dads-field mono min-w-0 flex-1 text-[12px]"
              />
              <button
                type="button"
                className="dads-btn"
                data-variant="neutral"
                data-size="sm"
                onClick={() => void handlePickPatchInput()}
              >
                入力選択
              </button>
              <button
                type="button"
                className="dads-btn"
                data-variant="outline"
                data-size="sm"
                disabled={patching || (patchInputPath || backupPath).trim() === ""}
                onClick={() => void handleRunPatch()}
              >
                {patching ? "作成中…" : "WNRO パッチを作成して保存"}
              </button>
            </div>

            {patchOutcome ? (
              <DadsBanner type="success" title="パッチ済みイメージを作成しました">
                <div className="space-y-1 text-[12px] tabular-nums">
                  <div className="truncate">
                    出力: <span className="mono font-bold">{patchOutcome.outputPath}</span>
                  </div>
                  <div>
                    サイズ: <span className="mono font-bold">{formatCount(patchOutcome.bytes)} B</span> ·{" "}
                    変更箇所:{" "}
                    <span className="mono font-bold">
                      {patchOutcome.changedOffsets.map(formatHexOffset).join(", ")}
                    </span>
                  </div>
                  <div className="truncate">
                    SHA-256: <span className="mono">{patchOutcome.sha256}</span>
                  </div>
                </div>
              </DadsBanner>
            ) : null}

            {patchError ? (
              <DadsBanner type="error" title="パッチ作成に失敗しました">
                <p className="font-bold">{patchError.message}</p>
                {patchError.hint ? <p className="mt-1">{patchError.hint}</p> : null}
              </DadsBanner>
            ) : null}
          </div>

          {/* Image File Picker */}
          <div className="space-y-1.5 text-[13px]">
            <label htmlFor="fw-image-path" className="block font-bold">
              書き込むファームウェアイメージ (.bin)
            </label>
            <div className="flex gap-2">
              <input
                id="fw-image-path"
                type="text"
                placeholder="/path/to/m241_patched_v114.bin"
                value={imagePath}
                onChange={(event) => setImagePath(event.target.value)}
                className="dads-field mono min-w-0 flex-1"
              />
              <button
                type="button"
                className="dads-btn shrink-0"
                data-variant="outline"
                data-size="sm"
                onClick={() => void handlePickImage()}
              >
                <FolderOpen size={14} strokeWidth={2} aria-hidden="true" />
                <span>イメージ選択</span>
              </button>
            </div>
          </div>

          {/* DA File Picker & Optional PROBE Guard */}
          <div className="space-y-2.5 text-[13px]">
            <label htmlFor="fw-da-path" className="block font-bold">
              Download Agent ファイル (DA_v3.8.4.bin)
            </label>
            <div className="flex gap-2">
              <input
                id="fw-da-path"
                type="text"
                placeholder="/path/to/DA_v3.8.4.bin"
                value={daPath}
                onChange={(event) => setDaPath(event.target.value)}
                className="dads-field mono min-w-0 flex-1"
              />
              <button
                type="button"
                className="dads-btn shrink-0"
                data-variant="outline"
                data-size="sm"
                onClick={() => void handlePickDa()}
              >
                <FolderOpen size={14} strokeWidth={2} aria-hidden="true" />
                <span>DA 選択</span>
              </button>
            </div>

            {/* Optional DA probe */}
            <details className="dads-disclosure pt-1">
              <summary>
                <span className="dads-disc-icon" aria-hidden="true">
                  ▸
                </span>
                <span>任意の DA 接続確認（BootROM → DA ロード試験）</span>
              </summary>
              <div className="dads-disclosure-body space-y-2.5 text-[12px]">
                <p className="text-[var(--dads-ink-sub)] leading-relaxed">
                  DA バイナリを RAM へ転送してフラッシュチップ識別情報を取得します。RAM 上で DA を起動するため、確認語{" "}
                  <code className="rounded border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-1 py-0.5 font-bold">
                    PROBE
                  </code>{" "}
                  の手入力が必要です。
                </p>
                <div className="flex flex-wrap items-center gap-2">
                  <input
                    type="text"
                    aria-label="DA 接続確認の確認語 PROBE"
                    placeholder="PROBE"
                    value={probeConfirmWord}
                    onChange={(event) => setProbeConfirmWord(event.target.value)}
                    className="dads-field mono w-28"
                  />
                  <button
                    type="button"
                    className="dads-btn"
                    data-variant="outline"
                    data-size="sm"
                    disabled={
                      probingDa ||
                      port === "" ||
                      daPath.trim() === "" ||
                      probeConfirmWord.trim() !== "PROBE"
                    }
                    onClick={() => void handleProbeDa()}
                  >
                    {probingDa ? "確認中…" : "DA 接続確認を実行"}
                  </button>
                </div>

                {probeOutcome ? (
                  <DadsBanner type="success" title="DA 接続確認に成功しました">
                    <dl className="grid grid-cols-2 gap-x-4 gap-y-1 tabular-nums">
                      <div>
                        ポート: <span className="mono font-bold">{probeOutcome.port}</span>
                      </div>
                      <div>
                        DA バージョン: <span className="mono font-bold">{probeOutcome.daVersion}</span>
                      </div>
                      <div>
                        フラッシュ ID: <span className="mono font-bold">{probeOutcome.flashId}</span>
                      </div>
                      <div>
                        容量:{" "}
                        <span className="mono font-bold">
                          {formatCount(probeOutcome.flashSizeBytes)} B
                        </span>
                      </div>
                      <div>
                        Mfr / Dev:{" "}
                        <span className="mono font-bold">
                          {formatHexOffset(probeOutcome.flashManufacturer)} /{" "}
                          {formatHexOffset(probeOutcome.flashDevice)}
                        </span>
                      </div>
                    </dl>
                  </DadsBanner>
                ) : null}

                {probeError ? (
                  <DadsBanner type="error" title="DA 接続確認に失敗しました">
                    <p className="font-bold">{probeError.message}</p>
                    {probeError.hint ? <p className="mt-1">{probeError.hint}</p> : null}
                  </DadsBanner>
                ) : null}
              </div>
            </details>
          </div>
        </section>

        {/* Step 3: Inspect & Flash Mode */}
        <section
          aria-label="ステップ 3: 書き込みモードと自動検査結果"
          className="space-y-4 border-b border-[var(--dads-border-divider)] pb-6"
        >
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-[16px] font-bold">
              ステップ 3. 事前検査結果と書き込みモード選択
            </h2>
            {imagePath.trim() !== "" ? (
              <button
                type="button"
                className="dads-btn"
                data-variant="neutral"
                data-size="xs"
                disabled={inspecting}
                onClick={() => void runInspect(imagePath, backupPath)}
              >
                <RefreshCw
                  size={12}
                  strokeWidth={2}
                  aria-hidden="true"
                  className={inspecting ? "animate-spin" : ""}
                />
                <span>再検査</span>
              </button>
            ) : null}
          </div>

          {inspectError ? (
            <DadsBanner type="error" title="イメージ検査に失敗しました">
              <p className="font-bold">{inspectError.message}</p>
              {inspectError.hint ? <p className="mt-1">{inspectError.hint}</p> : null}
            </DadsBanner>
          ) : null}

          {inspect ? (
            <div className="space-y-3">
              <DadsBanner
                type={
                  !inspect.armVectorOk || inspect.warnings.length > 0 ? "warning" : "success"
                }
                title={`推奨モード: ${inspect.recommendedMode}`}
              >
                <div className="space-y-1.5 text-[12px] tabular-nums">
                  <div className="grid grid-cols-1 gap-1 sm:grid-cols-2">
                    <div>
                      イメージサイズ:{" "}
                      <span className="mono font-bold">{formatCount(inspect.imageBytes)} B</span>
                    </div>
                    <div>
                      プロファイル:{" "}
                      <span className="mono font-bold">{inspect.profile ?? "未判定"}</span>
                    </div>
                    <div>
                      ARM 先頭ベクタ:{" "}
                      <span
                        className={
                          inspect.armVectorOk
                            ? "font-bold text-[var(--dads-success)]"
                            : "font-bold text-[var(--dads-error)]"
                        }
                      >
                        {inspect.armVectorOk ? "検出（正常）" : "不一致"}
                      </span>
                    </div>
                    <div>
                      WNRO パッチ状態:{" "}
                      <span className="font-bold">
                        {inspect.wnroPatched === null
                          ? "判定不可"
                          : inspect.wnroPatched
                            ? "適用済み"
                            : "未適用（ stock ）"}
                      </span>
                    </div>
                    <div>
                      公式ベンダー一致:{" "}
                      <span className="font-bold">
                        {inspect.isVendorV113
                          ? "公式 V1.13 一致"
                          : inspect.isVendorV112
                            ? "公式 V1.12 一致"
                            : "非公式 / 変更済み"}
                      </span>
                    </div>
                    <div>
                      バックアップ差分:{" "}
                      <span className="mono font-bold">
                        {inspect.diffCount === null
                          ? "バックアップ未指定"
                          : `${formatCount(inspect.diffCount)} B`}
                        {inspect.outsidePatchOffsets === true
                          ? "（パッチ範囲外あり）"
                          : inspect.outsidePatchOffsets === false
                            ? "（パッチ範囲内のみ）"
                            : ""}
                      </span>
                    </div>
                  </div>
                  <div className="truncate border-t border-[var(--dads-border-divider)] pt-1">
                    イメージ SHA-256: <span className="mono">{inspect.imageSha256}</span>
                  </div>
                  {inspect.backupSha256 ? (
                    <div className="truncate">
                      バックアップ SHA-256: <span className="mono">{inspect.backupSha256}</span>
                    </div>
                  ) : null}
                </div>
              </DadsBanner>

              {inspect.warnings.length > 0 ? (
                <DadsBanner type="warning" title="検査時の注意事項">
                  <ul className="list-disc space-y-1 pl-5 text-[12px]">
                    {inspect.warnings.map((warn, idx) => (
                      <li key={idx}>{warn}</li>
                    ))}
                  </ul>
                </DadsBanner>
              ) : null}
            </div>
          ) : (
            <p className="text-[13px] text-[var(--dads-ink-meta)]">
              ステップ 2 で書き込みイメージを指定すると、サイズ・SHA-256・ARM ベクタ・差分検査が自動実行されます。
            </p>
          )}

          {/* Mode Selector */}
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {FLASH_MODES.map((entry) => {
              const selected = mode === entry.id;
              const isRecommended = inspect?.recommendedMode === entry.id;
              return (
                <label
                  key={entry.id}
                  className={
                    selected
                      ? "flex cursor-pointer items-start gap-2.5 rounded-lg border-2 border-[var(--dads-blue)] bg-[var(--dads-blue-tint)] p-3"
                      : "flex cursor-pointer items-start gap-2.5 rounded-lg border border-[var(--dads-border-control)] bg-white p-3 hover:bg-[var(--dads-surface-50)]"
                  }
                >
                  <input
                    type="radio"
                    name="fw-flash-mode"
                    value={entry.id}
                    checked={selected}
                    onChange={() => {
                      setMode(entry.id);
                      setRiskAgreed(false);
                    }}
                    className="mt-0.5"
                  />
                  <div className="min-w-0 flex-1 text-[12px]">
                    <div className="flex flex-wrap items-center gap-1.5 font-bold text-[var(--dads-ink)]">
                      <span>{entry.label}</span>
                      {isRecommended ? (
                        <span className="rounded bg-[var(--dads-blue)] px-1.5 py-0.5 text-[10px] font-bold text-white">
                          推奨
                        </span>
                      ) : null}
                    </div>
                    <p className="mt-1 leading-relaxed text-[var(--dads-ink-sub)]">{entry.desc}</p>
                  </div>
                </label>
              );
            })}
          </div>
        </section>

        {/* Step 4: Execute Flash with WRITE guard */}
        <section aria-label="ステップ 4: 書き込み実行" className="space-y-4 pb-6">
          <h2 className="text-[16px] font-bold">
            ステップ 4. 書き込み実行と読み戻し照合
          </h2>

          <div className="rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-3.5 text-[12px] space-y-1">
            <div>
              ポート: <span className="mono font-bold">{port || "未選択"}</span> · モード:{" "}
              <span className="mono font-bold">{mode}</span>
              {assumeBrom ? " (BootROM 前提)" : ""}
            </div>
            <div className="truncate">
              バックアップ: <span className="mono font-bold">{backupPath || "未選択"}</span>
            </div>
            <div className="truncate">
              書き込みイメージ: <span className="mono font-bold">{imagePath || "未選択"}</span>
            </div>
            <div className="truncate">
              DA ファイル: <span className="mono font-bold">{daPath || "未選択"}</span>
            </div>
          </div>

          {needsRiskAck ? (
            <label className="flex cursor-pointer items-start gap-3 rounded-lg border-2 border-[var(--dads-warning-border)] bg-[var(--dads-warning-bg)] p-3 text-[13px]">
              <input
                type="checkbox"
                className="dads-checkbox mt-0.5"
                checked={riskAgreed}
                onChange={(event) => setRiskAgreed(event.target.checked)}
              />
              <span className="font-bold leading-relaxed">
                上記の検査結果と注意事項（個体データの変更や未検証モードのリスクを含む）を確認・理解した上で実行します。
              </span>
            </label>
          ) : null}

          <div className="flex flex-wrap items-center gap-3 text-[13px]">
            <label htmlFor="fw-write-confirm" className="font-bold">
              確認のため{" "}
              <code className="rounded border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-1.5 py-0.5">
                WRITE
              </code>{" "}
              と入力:
            </label>
            <input
              id="fw-write-confirm"
              type="text"
              placeholder="WRITE"
              value={writeConfirmWord}
              onChange={(event) => setWriteConfirmWord(event.target.value)}
              className="dads-field mono w-36"
            />
            <button
              type="button"
              className="dads-btn"
              data-variant={mode === "unverified" || mode === "recover" ? "danger" : "solid-fill"}
              data-size="md"
              disabled={!canFlash}
              onClick={() => void handleFlash()}
            >
              <ShieldAlert size={16} strokeWidth={2} aria-hidden="true" />
              <span>{flashing ? "書き込み・読み戻し照合中…" : "書き込みを実行"}</span>
            </button>
          </div>

          {flashOutcome ? (
            <DadsBanner type="success" title="ファームウェア書き込みと読み戻し照合が完了しました">
              <dl className="grid grid-cols-1 gap-1 text-[12px] tabular-nums sm:grid-cols-2">
                <div>
                  ポート: <span className="mono font-bold">{flashOutcome.port}</span>
                </div>
                <div>
                  モード: <span className="mono font-bold">{flashOutcome.mode}</span>
                </div>
                <div>
                  書き込みバイト数:{" "}
                  <span className="mono font-bold">{formatCount(flashOutcome.bytesWritten)} B</span>
                </div>
                <div>
                  所要時間:{" "}
                  <span className="mono font-bold">
                    {(flashOutcome.elapsedMs / 1000).toFixed(1)} 秒
                  </span>
                </div>
                <div>
                  読み戻し照合:{" "}
                  <span className="font-bold text-[var(--dads-success)]">
                    {flashOutcome.verifiedReadBack ? "一致" : "不一致"}
                  </span>
                </div>
                <div>
                  個体データ復元バイト数 (restoredPerUnitBytes):{" "}
                  <span className="mono font-bold">
                    {formatCount(flashOutcome.restoredPerUnitBytes)} B
                  </span>
                </div>
                <div className="truncate sm:col-span-2">
                  イメージ SHA-256: <span className="mono">{flashOutcome.imageSha256}</span>
                </div>
              </dl>
            </DadsBanner>
          ) : null}

          {flashError ? (
            <DadsBanner type="error" title="ファームウェア書き込みに失敗しました">
              <p className="font-bold">{flashError.message}</p>
              {flashError.hint ? <p className="mt-1">{flashError.hint}</p> : null}
            </DadsBanner>
          ) : null}
        </section>
      </div>
    </div>
  );
}
