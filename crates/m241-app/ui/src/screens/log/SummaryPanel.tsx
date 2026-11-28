import { DadsBanner } from "../../components/DadsBanner";
import type { LogController } from "../../hooks/useLogDocument";
import {
  formatCount,
  formatDistance,
  formatDuration,
  formatHeight,
  formatShortDate,
  formatSpeed,
  formatUtcDate,
  formatUtcDateTime,
} from "../../lib/format";

/** 2014-01-01T00:00:00Z — timestamps older than this indicate an unpatched WNRO log. */
const WNRO_CUTOFF_UNIX = 1_388_534_400;

/**
 * KPI Summary, Daily Table, and Parser Diagnostic Disclosure.
 */
export function SummaryPanel({
  log,
  onNavigateGeotag,
  onNavigateFirmware,
}: {
  log: LogController;
  onNavigateGeotag: () => void;
  onNavigateFirmware: () => void;
}) {
  const summary = log.summary;
  if (summary === null) {
    return null;
  }

  const activeDate =
    log.selectedPoint?.utc == null ? null : formatUtcDate(log.selectedPoint.utc);
  const maybeWnro =
    summary.startedAt !== null && summary.startedAt < WNRO_CUTOFF_UNIX;
  const { report } = summary;
  const hasAnomalies =
    report.damagedBlocks > 0 ||
    report.badChecksums > 0 ||
    report.truncatedRecords > 0 ||
    report.layoutMismatchedBlocks > 0 ||
    report.unparsedBytes > 0;

  return (
    <div className="space-y-4">
      {maybeWnro ? (
        <DadsBanner type="warning" title="週番号ロールオーバー (WNRO) の可能性">
          <p className="text-[12px] text-[var(--dads-ink)]">
            開始時刻が{" "}
            <span className="mono font-bold">
              {summary.startedAt !== null ? formatUtcDateTime(summary.startedAt) : "—"} UTC
            </span>{" "}
            となっています。未修正ファームウェアによる 7,168 日の日付ずれの可能性があります。
          </p>
          <button
            type="button"
            className="mt-1.5 text-[12px] font-bold text-[var(--dads-blue)] underline cursor-pointer"
            onClick={onNavigateFirmware}
          >
            ファームウェアタブで修復する →
          </button>
        </DadsBanner>
      ) : null}

      {/* KPI Summary */}
      <section aria-label="KPI サマリー" className="space-y-2">
        <div className="flex items-baseline justify-between">
          <h3 className="text-[13px] font-bold text-[var(--dads-ink)]">トラック集計</h3>
          <span className="mono text-[11px] text-[var(--dads-ink-meta)]">
            形式: {summary.format.toUpperCase()}
          </span>
        </div>

        <dl className="grid grid-cols-2 gap-2">
          <KpiCard label="総距離" value={formatDistance(summary.distanceM)} />
          <KpiCard label="所要時間" value={formatDuration(summary.durationSeconds)} />
          <KpiCard
            label="記録点 / 測位"
            value={`${formatCount(summary.points)} / ${formatCount(summary.fixes)}`}
          />
          <KpiCard label="ウェイポイント" value={`${formatCount(summary.waypoints)} 点`} />
          <KpiCard
            label="平均 / 最高速度"
            value={`${formatSpeed(summary.averageSpeedKmh)} / ${formatSpeed(summary.maxSpeedKmh)}`}
          />
          <KpiCard
            label="標高範囲"
            value={
              summary.minAltitudeM === null || summary.maxAltitudeM === null
                ? "—"
                : `${formatHeight(summary.minAltitudeM)} 〜 ${formatHeight(summary.maxAltitudeM)}`
            }
          />
        </dl>
      </section>

      {/* Day Table */}
      <section aria-label="日別テーブル" className="space-y-2">
        <div className="flex items-center justify-between">
          <h3 className="text-[13px] font-bold text-[var(--dads-ink)]">
            日別一覧 ({summary.days.length} 日)
          </h3>
          <button
            type="button"
            className="dads-btn"
            data-variant="outline"
            data-size="xs"
            onClick={log.fitWhole}
          >
            すべて表示
          </button>
        </div>

        {summary.days.length === 0 ? (
          <p className="text-[12px] text-[var(--dads-ink-meta)]">日別の記録がありません。</p>
        ) : (
          <div className="overflow-x-auto rounded-lg border border-[var(--dads-border-control)]">
            <table className="dads-table">
              <thead>
                <tr>
                  <th>日付 (UTC)</th>
                  <th className="text-right">点数</th>
                  <th className="text-right">距離</th>
                  <th className="text-right">時間</th>
                </tr>
              </thead>
              <tbody>
                {summary.days.map((day) => {
                  const active = day.date === activeDate;
                  return (
                    <tr
                      key={day.date}
                      onClick={() => log.fitDay(day)}
                      className={
                        active
                          ? "cursor-pointer bg-[var(--dads-blue-tint)] font-bold"
                          : "cursor-pointer hover:bg-[var(--dads-surface-50)]"
                      }
                    >
                      <td>
                        <div className="font-bold text-[var(--dads-ink)]">
                          {formatShortDate(day.date)}
                        </div>
                        <div className="mono text-[11px] text-[var(--dads-ink-meta)]">
                          {day.date}
                        </div>
                      </td>
                      <td className="mono text-right">{formatCount(day.points)}</td>
                      <td className="mono text-right">{formatDistance(day.distanceM)}</td>
                      <td className="mono text-right">
                        {formatDuration(day.durationSeconds)}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {/* Parser Diagnostics Disclosure */}
      <section aria-label="パーサー診断" className="border-t border-[var(--dads-border-divider)] pt-3">
        <details className="dads-disclosure" open={hasAnomalies}>
          <summary>
            <span className="dads-disc-icon" aria-hidden="true">
              ▸
            </span>
            <span>パーサー診断詳細</span>
            <span
              className={
                hasAnomalies
                  ? "rounded border border-[var(--dads-warning-border)] bg-[var(--dads-warning-bg)] px-1.5 py-0.5 text-[11px] font-bold text-[var(--dads-warning)]"
                  : "rounded border border-[var(--dads-success)] bg-[var(--dads-success-bg)] px-1.5 py-0.5 text-[11px] font-bold text-[var(--dads-success)]"
              }
            >
              {hasAnomalies ? "注意あり" : "正常"}
            </span>
          </summary>

          <div className="dads-disclosure-body space-y-1.5 text-[12px]">
            <dl className="space-y-1 tabular-nums">
              <DiagRow label="総ブロック数" value={formatCount(report.blocks)} />
              <DiagRow label="空ブロック数" value={formatCount(report.emptyBlocks)} />
              <DiagRow
                label="破損ブロック数"
                value={formatCount(report.damagedBlocks)}
                warn={report.damagedBlocks > 0}
              />
              <DiagRow label="レコード数" value={formatCount(report.records)} />
              <DiagRow
                label="チェックサム不一致"
                value={formatCount(report.badChecksums)}
                warn={report.badChecksums > 0}
              />
              <DiagRow
                label="切り詰めレコード"
                value={formatCount(report.truncatedRecords)}
                warn={report.truncatedRecords > 0}
              />
              <DiagRow label="ジャーナル項目" value={formatCount(report.journalItems)} />
              <DiagRow
                label="レイアウト不一致ブロック"
                value={formatCount(report.layoutMismatchedBlocks)}
                warn={report.layoutMismatchedBlocks > 0}
              />
              <DiagRow
                label="未解析バイト数"
                value={`${formatCount(report.unparsedBytes)} B`}
                warn={report.unparsedBytes > 0}
              />
            </dl>
          </div>
        </details>
      </section>

      <div className="border-t border-[var(--dads-border-divider)] pt-3">
        <button
          type="button"
          className="dads-btn w-full"
          data-variant="outline"
          data-size="sm"
          onClick={onNavigateGeotag}
        >
          このログで写真にジオタグを書き込む →
        </button>
      </div>
    </div>
  );
}

function KpiCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-[var(--dads-border-divider)] bg-[var(--dads-surface-50)] px-2.5 py-2">
      <dt className="text-[11px] font-bold text-[var(--dads-ink-sub)]">{label}</dt>
      <dd className="mono mt-0.5 truncate text-[13px] font-bold text-[var(--dads-ink)]" title={value}>
        {value}
      </dd>
    </div>
  );
}

function DiagRow({
  label,
  value,
  warn = false,
}: {
  label: string;
  value: string;
  warn?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-2 border-b border-[var(--dads-border-divider)] py-1 last:border-b-0">
      <dt className="text-[var(--dads-ink-sub)]">{label}</dt>
      <dd
        className={
          warn
            ? "mono font-bold text-[var(--dads-warning)]"
            : "mono font-bold text-[var(--dads-ink)]"
        }
      >
        {value}
      </dd>
    </div>
  );
}
