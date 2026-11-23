import { StageTrail } from "../components/StageTrail";

const STAGES = ["ポートを選ぶ", "端末情報を読む", "地図で確かめる"] as const;

/**
 * The map has nothing to show until a track exists, so this panel is the empty
 * state of the only visual the product has. The topographic hairlines are drawn
 * in CSS, not an image, and the stage trail runs across them: the route is the
 * screen's signature, and it is also the status summary.
 */
export function MapPanel({ stage }: { stage: number }) {
  return (
    <section
      aria-labelledby="map-title"
      className="card relative flex min-h-40 flex-1 flex-col overflow-hidden"
    >
      <div aria-hidden="true" className="contour absolute inset-0" />

      <div className="relative flex flex-1 flex-col justify-between gap-6 p-5">
        <div>
          <h2 id="map-title" className="text-md font-medium text-ink">
            地図
          </h2>
          <p className="mt-1 text-xs text-ink-3">ログ画面（P2）で表示します</p>
        </div>

        <StageTrail stages={STAGES} current={stage} />
      </div>
    </section>
  );
}
