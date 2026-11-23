import { cn } from "../lib/cn";

/**
 * The signature: a stage trail drawn as one thin line with nodes on it.
 *
 * It is the only progress the Connect screen has, and it doubles as the status
 * summary — a done node is filled, the current one is an open ring, the rest
 * are hairline. The labels are the stages of a read, so the line always says
 * what has happened and what comes next without a sentence.
 */
export function StageTrail({
  stages,
  current,
}: {
  stages: readonly string[];
  current: number;
}) {
  const span = stages.length - 1;
  const progress = span <= 0 ? 0 : Math.min(Math.max(current, 0), span) / span;
  const last = stages.length - 1;

  return (
    <div className="relative">
      {/* The track runs from the first node's centre to the last one's, so the
          line and the dots share one axis. */}
      <span aria-hidden="true" className="absolute left-[4px] right-[4px] top-[3px] h-[2px] bg-hair" />
      {progress > 0 ? (
        <span
          aria-hidden="true"
          className="absolute left-[4px] top-[3px] h-[2px] bg-accent"
          style={{ width: `calc((100% - 8px) * ${progress})` }}
        />
      ) : null}

      <ol className="relative grid grid-cols-3">
        {stages.map((label, index) => {
          const state = index < current ? "done" : index === current ? "current" : "future";
          return (
            <li
              key={label}
              className={cn(
                "flex flex-col gap-2",
                index === 0 ? "items-start" : index === last ? "items-end" : "items-center",
              )}
            >
              <span
                aria-hidden="true"
                className={cn(
                  "size-2 rounded-full",
                  state === "done" && "bg-accent",
                  state === "current" && "border-2 border-accent bg-card",
                  state === "future" && "border-2 border-hair-strong bg-card",
                )}
              />
              <span
                className={cn(
                  "text-xs",
                  state === "current" && "font-medium text-ink",
                  state === "done" && "text-ink-2",
                  state === "future" && "text-ink-3",
                )}
              >
                {label}
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
