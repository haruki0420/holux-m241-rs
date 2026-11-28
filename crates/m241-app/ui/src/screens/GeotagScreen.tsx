import { Camera, CheckCircle2, FolderOpen, RefreshCw } from "lucide-react";
import { useCallback, useState } from "react";

import { DadsBanner } from "../components/DadsBanner";
import type { LogController } from "../hooks/useLogDocument";
import { pickDirectory, pickOpenFile } from "../lib/dialog";
import {
  baseName,
  formatCount,
  formatGapSeconds,
  formatLatLon,
  formatUtcDateTime,
} from "../lib/format";
import {
  geotagApply,
  geotagPreview,
  splitError,
  type GeotagApplyOutcome,
  type GeotagOptionsDto,
  type GeotagPreviewDto,
} from "../lib/ipc";

const GAP_PRESETS = [
  { label: "1分 (60秒)", seconds: 60 },
  { label: "5分 (300秒)", seconds: 300 },
  { label: "15分 (900秒)", seconds: 900 },
  { label: "60分 (3600秒)", seconds: 3600 },
] as const;

/**
 * Tab 2: ジオタグ (Photo folder geotagging preview & batch EXIF apply).
 */
export function GeotagScreen({
  log,
  onNavigateLog,
}: {
  log: LogController;
  onNavigateLog: () => void;
}) {
  const [directory, setDirectory] = useState("");
  const [cameraOffsetMinutes, setCameraOffsetMinutes] = useState(540);
  const [logOffsetDays, setLogOffsetDays] = useState(0);
  const [maxGapSeconds, setMaxGapSeconds] = useState(300);
  const [groupDistanceMeters, setGroupDistanceMeters] = useState(0);
  const [rewriteDatetimeOriginal, setRewriteDatetimeOriginal] = useState(false);
  const [keepBackup, setKeepBackup] = useState(true);

  const [preview, setPreview] = useState<GeotagPreviewDto | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [applying, setApplying] = useState(false);
  const [applyOutcome, setApplyOutcome] = useState<GeotagApplyOutcome | null>(null);
  const [applyError, setApplyError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  const buildOptions = useCallback(
    (): GeotagOptionsDto => ({
      cameraOffsetMinutes: Number.isFinite(cameraOffsetMinutes) ? Math.round(cameraOffsetMinutes) : 0,
      logOffsetDays: Number.isFinite(logOffsetDays) ? Math.round(logOffsetDays) : 0,
      groupDistanceMeters: Math.max(
        0,
        Number.isFinite(groupDistanceMeters) ? Math.round(groupDistanceMeters) : 0,
      ),
      maxGapSeconds: Math.max(1, Number.isFinite(maxGapSeconds) ? Math.round(maxGapSeconds) : 300),
      keepBackup,
      rewriteDatetimeOriginal,
    }),
    [
      cameraOffsetMinutes,
      logOffsetDays,
      groupDistanceMeters,
      maxGapSeconds,
      keepBackup,
      rewriteDatetimeOriginal,
    ],
  );

  const runPreview = useCallback(
    async (targetDir: string) => {
      const trimmed = targetDir.trim();
      if (trimmed === "" || log.summary === null) {
        return;
      }
      setPreviewLoading(true);
      setPreviewError(null);
      setConfirmOpen(false);
      try {
        const result = await geotagPreview(trimmed, buildOptions());
        setPreview(result);
      } catch (error) {
        setPreviewError(splitError(error));
        setPreview(null);
      } finally {
        setPreviewLoading(false);
      }
    },
    [buildOptions, log.summary],
  );

  const handlePickDirectory = async () => {
    const picked = await pickDirectory({ title: "写真フォルダを選択" });
    if (!picked) {
      return;
    }
    setDirectory(picked);
    setApplyOutcome(null);
    setApplyError(null);
    if (log.summary !== null) {
      await runPreview(picked);
    }
  };

  const handleOpenLogQuick = async () => {
    const selected = await pickOpenFile({
      title: "参照するトラックログを開く",
      filters: [
        { name: "ログファイル", extensions: ["bin", "gpx", "kml", "kmz"] },
        { name: "すべて", extensions: ["*"] },
      ],
    });
    if (selected) {
      log.openFile(selected);
    }
  };

  const handleApply = async () => {
    const trimmed = directory.trim();
    if (trimmed === "" || log.summary === null) {
      return;
    }
    setApplying(true);
    setApplyError(null);
    setApplyOutcome(null);
    try {
      const result = await geotagApply(trimmed, buildOptions());
      setApplyOutcome(result);
      setConfirmOpen(false);
      // Refresh preview so updated state is visible.
      await runPreview(trimmed);
    } catch (error) {
      setApplyError(splitError(error));
    } finally {
      setApplying(false);
    }
  };

  const summary = log.summary;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-white">
      <div className="mx-auto max-w-5xl space-y-5 px-6 py-6 text-[14px]">
        {/* Header */}
        <div className="border-b border-[var(--dads-border-divider)] pb-3">
          <h1 className="text-[20px] font-bold text-[var(--dads-ink)]">
            写真へのジオタグ一括埋め込み
          </h1>
          <p className="mt-1 text-[13px] text-[var(--dads-ink-sub)]">
            読み込み済みのトラックログと撮影時刻を照合し、フォルダ内の写真に GPS 座標を書き込みます。
          </p>
        </div>

        {/* 1. Reference Log & 2. Photo Folder */}
        <div className="grid grid-cols-1 gap-4 border-b border-[var(--dads-border-divider)] pb-5 md:grid-cols-2">
          {/* Reference Log */}
          <div className="space-y-1.5">
            <div className="text-[13px] font-bold text-[var(--dads-ink)]">
              1. 参照するトラックログ
            </div>
            {summary ? (
              <div className="flex items-center justify-between gap-2 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-3.5 py-2.5">
                <div className="min-w-0">
                  <div className="truncate font-bold text-[var(--dads-ink)]" title={summary.source}>
                    {summary.name}
                  </div>
                  <div className="mono text-[12px] text-[var(--dads-ink-sub)]">
                    {formatCount(summary.points)} 点 ·{" "}
                    {summary.startedAt !== null
                      ? `${formatUtcDateTime(summary.startedAt)} UTC`
                      : "時刻不明"}
                  </div>
                </div>
                <button
                  type="button"
                  className="dads-btn shrink-0"
                  data-variant="outline"
                  data-size="xs"
                  onClick={onNavigateLog}
                >
                  ログタブへ
                </button>
              </div>
            ) : (
              <DadsBanner type="warning" title="トラックログが開かれていません">
                <p className="text-[12px] text-[var(--dads-ink)]">
                  先にログを開くか、下のボタンから読み込んでください。
                </p>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    className="dads-btn"
                    data-variant="outline"
                    data-size="xs"
                    onClick={() => void handleOpenLogQuick()}
                  >
                    ファイルを開く
                  </button>
                  <button
                    type="button"
                    className="dads-btn"
                    data-variant="neutral"
                    data-size="xs"
                    onClick={log.openSample}
                  >
                    サンプルを開く
                  </button>
                </div>
              </DadsBanner>
            )}
          </div>

          {/* Photo Directory */}
          <div className="space-y-1.5">
            <div className="text-[13px] font-bold text-[var(--dads-ink)]">
              2. 対象の写真フォルダ
            </div>
            <div className="flex items-center gap-2 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-2">
              <input
                type="text"
                aria-label="対象の写真フォルダパス"
                placeholder="/home/user/Pictures/Trip"
                value={directory}
                onChange={(event) => setDirectory(event.target.value)}
                className="dads-field mono min-w-0 flex-1 text-[12px]"
              />
              <button
                type="button"
                className="dads-btn shrink-0"
                data-variant="outline"
                data-size="sm"
                onClick={() => void handlePickDirectory()}
              >
                <FolderOpen size={14} strokeWidth={2} aria-hidden="true" />
                <span>フォルダ選択</span>
              </button>
            </div>
          </div>
        </div>

        {/* Time & Matching Options */}
        <div className="space-y-4 border-b border-[var(--dads-border-divider)] pb-5 text-[13px]">
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
            {/* cameraOffsetMinutes */}
            <div className="space-y-1.5">
              <label htmlFor="gt-camera-offset" className="block font-bold">
                カメラ時刻オフセット (分)
              </label>
              <input
                id="gt-camera-offset"
                type="number"
                step={1}
                value={cameraOffsetMinutes}
                onChange={(event) => {
                  const val = Number(event.target.value);
                  setCameraOffsetMinutes(Number.isFinite(val) ? val : 0);
                }}
                className="dads-field mono w-full"
              />
              <div className="flex flex-wrap gap-1">
                <button
                  type="button"
                  className="dads-btn"
                  data-variant={cameraOffsetMinutes === 540 ? "outline" : "neutral"}
                  data-size="xs"
                  onClick={() => setCameraOffsetMinutes(540)}
                >
                  JST (+540)
                </button>
                <button
                  type="button"
                  className="dads-btn"
                  data-variant={cameraOffsetMinutes === 0 ? "outline" : "neutral"}
                  data-size="xs"
                  onClick={() => setCameraOffsetMinutes(0)}
                >
                  UTC (0)
                </button>
              </div>
              <p className="text-[11px] text-[var(--dads-ink-meta)]">
                日本標準時 (JST) は 540 分。時計が 2 分進んでいる場合は 542。
              </p>
            </div>

            {/* logOffsetDays */}
            <div className="space-y-1.5">
              <label htmlFor="gt-log-offset-days" className="block font-bold">
                ログ日付補正 (日)
              </label>
              <input
                id="gt-log-offset-days"
                type="number"
                step={1}
                value={logOffsetDays}
                onChange={(event) => {
                  const val = Number(event.target.value);
                  setLogOffsetDays(Number.isFinite(val) ? val : 0);
                }}
                className="dads-field mono w-full"
              />
              <div className="flex flex-wrap gap-1">
                <button
                  type="button"
                  className="dads-btn"
                  data-variant={logOffsetDays === 0 ? "outline" : "neutral"}
                  data-size="xs"
                  onClick={() => setLogOffsetDays(0)}
                >
                  0日
                </button>
                <button
                  type="button"
                  className="dads-btn"
                  data-variant={logOffsetDays === 7168 ? "outline" : "neutral"}
                  data-size="xs"
                  onClick={() => setLogOffsetDays(7168)}
                >
                  +7168日 (WNRO)
                </button>
              </div>
              <p className="text-[11px] text-[var(--dads-ink-meta)]">
                未修正ファームウェアのログには 7168 日を設定します。
              </p>
            </div>

            {/* maxGapSeconds */}
            <div className="space-y-1.5">
              <label htmlFor="gt-max-gap-sec" className="block font-bold">
                許容時間差 (秒)
              </label>
              <input
                id="gt-max-gap-sec"
                type="number"
                min={1}
                step={30}
                value={maxGapSeconds}
                onChange={(event) => {
                  const val = Number(event.target.value);
                  if (Number.isFinite(val) && val >= 1) {
                    setMaxGapSeconds(Math.round(val));
                  }
                }}
                className="dads-field mono w-full"
              />
              <div className="flex flex-wrap gap-1">
                {GAP_PRESETS.map((preset) => (
                  <button
                    key={preset.seconds}
                    type="button"
                    className="dads-btn"
                    data-variant={maxGapSeconds === preset.seconds ? "outline" : "neutral"}
                    data-size="xs"
                    onClick={() => setMaxGapSeconds(preset.seconds)}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </div>

            {/* groupDistanceMeters */}
            <div className="space-y-1.5">
              <label htmlFor="gt-group-dist" className="block font-bold">
                同地点グループ距離 (m)
              </label>
              <input
                id="gt-group-dist"
                type="number"
                min={0}
                step={5}
                value={groupDistanceMeters}
                onChange={(event) => {
                  const val = Number(event.target.value);
                  setGroupDistanceMeters(Number.isFinite(val) && val >= 0 ? Math.round(val) : 0);
                }}
                className="dads-field mono w-full"
              />
              <p className="text-[11px] text-[var(--dads-ink-meta)]">
                0 を指定するとグループ化を行いません。
              </p>
            </div>
          </div>

          {/* Checkboxes + Preview Refresh Button */}
          <div className="flex flex-wrap items-center justify-between gap-4 pt-1">
            <div className="flex flex-wrap items-center gap-6">
              <label className="flex cursor-pointer items-center gap-2.5">
                <input
                  type="checkbox"
                  className="dads-checkbox"
                  checked={rewriteDatetimeOriginal}
                  onChange={(event) => setRewriteDatetimeOriginal(event.target.checked)}
                />
                <span className="font-bold">DateTimeOriginal を書き換える</span>
              </label>

              <label className="flex cursor-pointer items-center gap-2.5">
                <input
                  type="checkbox"
                  className="dads-checkbox"
                  checked={keepBackup}
                  onChange={(event) => setKeepBackup(event.target.checked)}
                />
                <span className="font-bold">元の写真をバックアップ (.bak)</span>
              </label>
            </div>

            <button
              type="button"
              className="dads-btn"
              data-variant="outline"
              data-size="sm"
              disabled={previewLoading || directory.trim() === "" || summary === null}
              onClick={() => void runPreview(directory)}
            >
              <RefreshCw
                size={14}
                strokeWidth={2}
                aria-hidden="true"
                className={previewLoading ? "animate-spin" : ""}
              />
              <span>{previewLoading ? "照合中…" : "プレビューを更新"}</span>
            </button>
          </div>
        </div>

        {/* Preview Error */}
        {previewError ? (
          <DadsBanner type="error" title="プレビューの取得に失敗しました">
            <p className="font-bold">{previewError.message}</p>
            {previewError.hint ? <p className="mt-1">{previewError.hint}</p> : null}
          </DadsBanner>
        ) : null}

        {/* Preview Table */}
        {preview ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="text-[13px] font-bold">
                照合結果:{" "}
                <span className="mono text-[var(--dads-ink-sub)]">
                  {baseName(preview.directory)}
                </span>
              </div>
              <div className="flex items-center gap-3 text-[13px] font-bold tabular-nums">
                <span className="rounded border border-[var(--dads-success)] bg-[var(--dads-success-bg)] px-2.5 py-0.5 text-[var(--dads-success)]">
                  一致: {formatCount(preview.matched)} 件
                </span>
                <span className="rounded border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] px-2.5 py-0.5 text-[var(--dads-ink-sub)]">
                  スキップ: {formatCount(preview.skipped)} 件
                </span>
              </div>
            </div>

            <div className="overflow-x-auto rounded-lg border border-[var(--dads-border-control)] bg-white">
              <table className="dads-table">
                <thead>
                  <tr>
                    <th className="w-16">写真</th>
                    <th>ファイル名</th>
                    <th>撮影時刻 (UTC)</th>
                    <th>時刻差</th>
                    <th>GPS 座標 / 点</th>
                    <th>状態 / スキップ理由</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.items.length === 0 ? (
                    <tr>
                      <td colSpan={6} className="py-6 text-center text-[var(--dads-ink-meta)]">
                        対象フォルダに画像ファイルが見つかりませんでした。
                      </td>
                    </tr>
                  ) : (
                    preview.items.map((item) => {
                      const matched = item.skippedReason === null && item.lat !== null && item.lon !== null;
                      return (
                        <tr
                          key={item.path}
                          className={matched ? "" : "bg-[var(--dads-surface-50)]/60 opacity-85"}
                        >
                          <td>
                            {item.thumbnail ? (
                              <img
                                src={item.thumbnail}
                                alt={item.name}
                                className="size-12 rounded border border-[var(--dads-border-divider)] object-cover"
                              />
                            ) : (
                              <div className="flex size-12 items-center justify-center rounded border border-[var(--dads-border-divider)] bg-[var(--dads-surface-50)] text-[var(--dads-ink-meta)]">
                                <Camera size={16} strokeWidth={1.5} aria-hidden="true" />
                              </div>
                            )}
                          </td>
                          <td>
                            <div className="font-bold text-[var(--dads-ink)]" title={item.path}>
                              {item.name}
                            </div>
                            <div className="text-[11px] text-[var(--dads-ink-meta)]">
                              時刻元: {item.timeSource ?? "不明"}
                              {item.groupId !== null ? ` · グループ #${item.groupId}` : ""}
                            </div>
                          </td>
                          <td className="mono whitespace-nowrap">
                            {item.capturedAt === null ? "—" : formatUtcDateTime(item.capturedAt)}
                          </td>
                          <td className="mono whitespace-nowrap">
                            {formatGapSeconds(item.gapSeconds)}
                          </td>
                          <td className="mono whitespace-nowrap">
                            <div>{formatLatLon(item.lat, item.lon)}</div>
                            {item.pointIndex !== null ? (
                              <div className="text-[11px] text-[var(--dads-ink-meta)]">
                                点 #{item.pointIndex + 1}
                              </div>
                            ) : null}
                          </td>
                          <td>
                            {matched ? (
                              <span className="inline-flex items-center gap-1 rounded border border-[var(--dads-success)] bg-[var(--dads-success-bg)] px-2 py-0.5 text-[12px] font-bold text-[var(--dads-success)]">
                                <CheckCircle2 size={12} strokeWidth={2.5} aria-hidden="true" />
                                <span>書き込み対象</span>
                              </span>
                            ) : (
                              <span className="inline-block rounded border border-[var(--dads-warning-border)] bg-[var(--dads-warning-bg)] px-2 py-0.5 text-[12px] font-bold text-[var(--dads-warning)]">
                                {item.skippedReason ?? "対象外"}
                              </span>
                            )}
                          </td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
          </div>
        ) : (
          <div className="rounded-lg border border-dashed border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-6 text-center text-[13px] text-[var(--dads-ink-sub)]">
            写真フォルダを選択すると、プレビュー一覧がここに表示されます。
          </div>
        )}

        {/* Batch Apply Action Bar */}
        <div className="space-y-3 rounded-lg border border-[var(--dads-border-control)] bg-[var(--dads-surface-50)] p-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="text-[13px] text-[var(--dads-ink-sub)]">
              {preview
                ? `一致した ${formatCount(preview.matched)} 枚に GPS 座標を書き込みます（スキップ ${formatCount(preview.skipped)} 件）。`
                : "プレビュー後に一括書き込みを実行できます。"}
            </div>

            <button
              type="button"
              className="dads-btn"
              data-variant="solid-fill"
              data-size="md"
              disabled={
                applying ||
                preview === null ||
                preview.matched === 0 ||
                directory.trim() === "" ||
                summary === null
              }
              onClick={() => setConfirmOpen(true)}
            >
              {applying
                ? "書き込み実行中…"
                : `一括書き込みを実行 (${preview ? formatCount(preview.matched) : 0} 枚)`}
            </button>
          </div>

          {confirmOpen && preview ? (
            <DadsBanner type="info" title="書き込み内容の確認">
              <p>
                対象フォルダ <span className="mono font-bold">{preview.directory}</span> 内の一致写真{" "}
                <span className="mono font-bold">{formatCount(preview.matched)} 枚</span> に GPS EXIF を書き込みます。
              </p>
              <ul className="mt-1.5 list-disc pl-5 text-[12px] text-[var(--dads-ink)]">
                <li>
                  元ファイルバックアップ (.bak):{" "}
                  <strong>{keepBackup ? "作成する" : "作成しない"}</strong>
                </li>
                <li>
                  DateTimeOriginal 書き換え:{" "}
                  <strong>
                    {rewriteDatetimeOriginal
                      ? `書き換える (オフセット ${cameraOffsetMinutes} 分)`
                      : "書き換えない"}
                  </strong>
                </li>
              </ul>
              <div className="mt-3 flex justify-end gap-2">
                <button
                  type="button"
                  className="dads-btn"
                  data-variant="outline"
                  data-size="sm"
                  disabled={applying}
                  onClick={() => setConfirmOpen(false)}
                >
                  キャンセル
                </button>
                <button
                  type="button"
                  className="dads-btn"
                  data-variant="solid-fill"
                  data-size="sm"
                  disabled={applying}
                  onClick={() => void handleApply()}
                >
                  {applying ? "実行中…" : "実行する"}
                </button>
              </div>
            </DadsBanner>
          ) : null}

          {applyOutcome ? (
            <DadsBanner
              type={applyOutcome.errors.length > 0 ? "warning" : "success"}
              title={
                applyOutcome.errors.length > 0
                  ? "一括書き込みが完了しました（一部エラーあり）"
                  : "一括書き込みが完了しました"
              }
            >
              <dl className="grid grid-cols-3 gap-2 tabular-nums text-[13px]">
                <div>
                  <dt className="text-[11px] text-[var(--dads-ink-sub)]">書き込み成功</dt>
                  <dd className="mono font-bold">{formatCount(applyOutcome.written)} 件</dd>
                </div>
                <div>
                  <dt className="text-[11px] text-[var(--dads-ink-sub)]">スキップ</dt>
                  <dd className="mono font-bold">{formatCount(applyOutcome.skipped)} 件</dd>
                </div>
                <div>
                  <dt className="text-[11px] text-[var(--dads-ink-sub)]">バックアップ作成</dt>
                  <dd className="mono font-bold">
                    {formatCount(applyOutcome.backupsCreated)} 件
                  </dd>
                </div>
              </dl>
              {applyOutcome.errors.length > 0 ? (
                <ul className="mt-2 list-disc space-y-0.5 pl-5 text-[12px] text-[var(--dads-error)]">
                  {applyOutcome.errors.map((err, idx) => (
                    <li key={idx}>{err}</li>
                  ))}
                </ul>
              ) : null}
            </DadsBanner>
          ) : null}

          {applyError ? (
            <DadsBanner type="error" title="ジオタグ書き込みに失敗しました">
              <p className="font-bold">{applyError.message}</p>
              {applyError.hint ? <p className="mt-1">{applyError.hint}</p> : null}
            </DadsBanner>
          ) : null}
        </div>
      </div>
    </div>
  );
}
