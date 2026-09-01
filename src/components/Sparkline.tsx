import type { ThroughputSample } from "../store/reducers";

/**
 * Hand-rolled SVG sparkline for per-second throughput (no chart lib).
 * Two polylines — up (accent) and down (dim) — right-aligned in a fixed
 * 60-slot window so the curve grows left-to-right as samples arrive.
 */
export function Sparkline({
  samples,
  width = 240,
  height = 44,
}: {
  samples: ThroughputSample[];
  width?: number;
  height?: number;
}) {
  const slots = 60;
  const n = Math.min(samples.length, slots);
  const view = samples.slice(-slots);
  const max = Math.max(1, ...view.map((s) => Math.max(s.up, s.down)));

  const points = (pick: (s: ThroughputSample) => number): string =>
    view
      .map((s, i) => {
        const x = (width * (slots - n + i)) / (slots - 1);
        const y = height - 2 - ((height - 4) * pick(s)) / max;
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-11 w-full"
      preserveAspectRatio="none"
      aria-hidden
    >
      {/* Baseline so an idle flat line is still visible. */}
      <line x1="0" y1={height - 2} x2={width} y2={height - 2} className="stroke-line" strokeWidth="1" />
      <polyline
        points={points((s) => s.down)}
        fill="none"
        strokeWidth="1.5"
        className="stroke-dim"
      />
      <polyline
        points={points((s) => s.up)}
        fill="none"
        strokeWidth="1.5"
        className="stroke-accent"
      />
    </svg>
  );
}
