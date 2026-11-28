import { AlertCircle, AlertTriangle, CheckCircle2, Info } from "lucide-react";
import type { ReactNode } from "react";

export type DadsBannerType = "info" | "success" | "warning" | "error";

/**
 * DADS Notification Banner.
 *
 * Always pairs status colour with an explicit icon and structured text label.
 */
export function DadsBanner({
  type,
  title,
  children,
  className = "",
}: {
  type: DadsBannerType;
  title?: string;
  children: ReactNode;
  className?: string;
}) {
  const role = type === "error" || type === "warning" ? "alert" : "status";

  return (
    <div className={`dads-banner ${className}`} data-type={type} role={role}>
      <BannerIcon type={type} />
      <div className="min-w-0 flex-1 space-y-1 text-[13px] leading-relaxed">
        {title ? <div className="text-[14px] font-bold">{title}</div> : null}
        <div>{children}</div>
      </div>
    </div>
  );
}

function BannerIcon({ type }: { type: DadsBannerType }) {
  switch (type) {
    case "success":
      return (
        <CheckCircle2
          size={18}
          strokeWidth={2}
          aria-hidden="true"
          className="mt-0.5 shrink-0 text-[var(--dads-success)]"
        />
      );
    case "warning":
      return (
        <AlertTriangle
          size={18}
          strokeWidth={2}
          aria-hidden="true"
          className="mt-0.5 shrink-0 text-[var(--dads-warning)]"
        />
      );
    case "error":
      return (
        <AlertCircle
          size={18}
          strokeWidth={2}
          aria-hidden="true"
          className="mt-0.5 shrink-0 text-[var(--dads-error)]"
        />
      );
    case "info":
    default:
      return (
        <Info
          size={18}
          strokeWidth={2}
          aria-hidden="true"
          className="mt-0.5 shrink-0 text-[var(--dads-blue)]"
        />
      );
  }
}
