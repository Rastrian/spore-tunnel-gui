import { STATE_BG } from "../lib/statusView";
import type { TunnelState } from "../lib/types";

/** Small status dot for list rows. "starting" pulses for a live feel. */
export function StatusDot({ state }: { state: TunnelState }) {
  return (
    <span
      aria-hidden
      className={`inline-block h-2.5 w-2.5 shrink-0 rounded-full ${STATE_BG[state]}${
        state === "starting" ? " animate-pulse" : ""
      }`}
    />
  );
}
