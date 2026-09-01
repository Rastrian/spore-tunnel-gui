import { STATE_STROKE } from "../lib/statusView";
import type { TunnelState } from "../lib/types";

const RADIUS = 42;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/**
 * Circular status badge behind the dashboard state label. The whole ring
 * takes the state color (idle dim / starting warn / connected accent /
 * failed danger); "starting" pulses. Track uses the border color.
 */
export function StatusRing({ state }: { state: TunnelState }) {
  return (
    <svg
      viewBox="0 0 96 96"
      className={`h-24 w-24 ${state === "starting" ? "animate-pulse" : ""}`}
      role="img"
      aria-label={`Status: ${state}`}
    >
      <circle cx="48" cy="48" r={RADIUS} fill="none" strokeWidth="6" className="stroke-line" />
      <circle
        cx="48"
        cy="48"
        r={RADIUS}
        fill="none"
        strokeWidth="6"
        strokeLinecap="round"
        strokeDasharray={CIRCUMFERENCE}
        transform="rotate(-90 48 48)"
        className={STATE_STROKE[state]}
      />
    </svg>
  );
}
