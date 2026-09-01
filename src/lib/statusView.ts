// Status → presentation mapping. Complete class strings only — Tailwind's
// scanner must see whole literals (no `bg-${x}` construction).

import type { TunnelState } from "./types";

/** Text color per tunnel state (spec: idle/stopped dim, starting warn,
 * connected accent, failed danger). */
export const STATE_TEXT: Record<TunnelState, string> = {
  idle: "text-dim",
  starting: "text-warn",
  connected: "text-accent",
  failed: "text-danger",
  stopped: "text-dim",
};

/** Background/border color per tunnel state, for dots and rings. */
export const STATE_BG: Record<TunnelState, string> = {
  idle: "bg-dim",
  starting: "bg-warn",
  connected: "bg-accent",
  failed: "bg-danger",
  stopped: "bg-dim",
};

/** SVG stroke color per tunnel state (Tailwind `stroke-*` utilities apply
 * to SVG shapes and follow the theme variables). */
export const STATE_STROKE: Record<TunnelState, string> = {
  idle: "stroke-dim",
  starting: "stroke-warn",
  connected: "stroke-accent",
  failed: "stroke-danger",
  stopped: "stroke-dim",
};

/** Very short label that fits inside the status ring. */
export const STATE_BADGE: Record<TunnelState, string> = {
  idle: "IDLE",
  starting: "CONN",
  connected: "LIVE",
  failed: "FAIL",
  stopped: "STOP",
};

export function stateLabel(state: TunnelState): string {
  switch (state) {
    case "starting":
      return "Connecting";
    case "connected":
      return "Connected";
    case "failed":
      return "Failed";
    case "stopped":
      return "Stopped";
    default:
      return "Idle";
  }
}

/** Is the tunnel in a running (startable-then-stoppable) state? */
export function isRunning(state: TunnelState): boolean {
  return state === "starting" || state === "connected";
}
