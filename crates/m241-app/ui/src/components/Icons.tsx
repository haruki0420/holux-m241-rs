import type { SVGProps } from "react";

/*
 * The four screen icons, drawn on a 20×20 grid with a 1.5 stroke.
 *
 * They are line drawings rather than an icon font or emoji: an emoji renders
 * with the platform's colour emoji font (or as a box when the subset font is
 * missing the glyph), which would make the rail look different on every OS.
 * Only the four screens P0 needs are here; later screens add their own.
 */

type IconProps = SVGProps<SVGSVGElement>;

const base = {
  width: 20,
  height: 20,
  viewBox: "0 0 20 20",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
} as const;

/** Connect: a USB plug. */
export function ConnectIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M7.5 2.5v4M12.5 2.5v4" />
      <path d="M6 6.5h8v3a4 4 0 0 1-8 0v-3Z" />
      <path d="M10 13.5v4" />
    </svg>
  );
}

/** Log: a recorded list. */
export function LogIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M7 5h10M7 10h10M7 15h10" />
      <circle cx="3.5" cy="5" r="1" />
      <circle cx="3.5" cy="10" r="1" />
      <circle cx="3.5" cy="15" r="1" />
    </svg>
  );
}

/** Firmware: a flash chip. */
export function FirmwareIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <rect x="6" y="6" width="8" height="8" rx="1.5" />
      <path d="M8 2.5V6M12 2.5V6M8 14v3.5M12 14v3.5M2.5 8H6M2.5 12H6M14 8h3.5M14 12h3.5" />
    </svg>
  );
}

/** Settings: two sliders. */
export function SettingsIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M3 6h14M3 14h14" />
      <circle cx="7.5" cy="6" r="2" />
      <circle cx="12.5" cy="14" r="2" />
    </svg>
  );
}
