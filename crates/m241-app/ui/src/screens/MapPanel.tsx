import { ArrowRight } from "lucide-react";

import { StageTrail } from "../components/StageTrail";

const STAGES = ["ポートを選ぶ", "端末情報を読む", "地図で確かめる"] as const;

/**
 * The map has nothing to show until a record exists, so this panel is the
 * Connect screen's empty state of the only visual the product has. It now also
 * carries the way there: the map itself lives on the Log screen, and this strip
 * says so without naming a phase.
 */
export function MapPanel({ stage, onOpenLog }: { stage: number; onOpenLog: () => void }) {
  return (
    <section
      aria-labelledby="map-title"
      className="card relative flex min-h-40 flex-1 flex-col overflow-hidden"
    >
      <div aria-hidden="true" className="contour absolute inset-0" />

      <div className="relative flex flex-1 flex-col justify-between gap-6 p-5">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h2 id="map-title" className="text-md font-medium text-ink">
              地図
            </h2>
            <p className="mt-1 text-xs text-ink-3">記録を開くと、地図で軌跡を確かめられます</p>
          </div>
          <button type="button" className="btn-quiet -mr-2 shrink-0" onClick={onOpenLog}>
            ログを開く
            <ArrowRight size={16} strokeWidth={1.5} aria-hidden="true" />
          </button>
        </div>

        <StageTrail stages={STAGES} current={stage} />
      </div>
    </section>
  );
}
