import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownToLine, Check, Copy, Search, ChevronsDown } from "lucide-react";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { formatClock } from "../lib/format";

/** Distance from the bottom (px) under which we are "at the latest". */
const BOTTOM_THRESHOLD = 24;

/**
 * Monospace per-tunnel console. Content = backfill + live `tunnel://log`
 * stream from the store (cap enforced by the reducer); this component only
 * filters and scrolls. Autoscroll can be toggled, and scrolling away from
 * the bottom pauses it until the user jumps back (or scrolls back).
 */
export function LogConsole({ profileId }: { profileId: string }) {
  const entries = useTunnels((s) => s.logs[profileId]);
  const showToast = useUi((s) => s.showToast);
  const [filter, setFilter] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const [paused, setPaused] = useState(false);
  const [copied, setCopied] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const copyTimer = useRef<number | undefined>(undefined);

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const all = entries ?? [];
    return q ? all.filter((e) => e.line.toLowerCase().includes(q)) : all;
  }, [entries, filter]);

  const stick = autoScroll && !paused;

  useEffect(() => {
    const el = scrollRef.current;
    if (stick && el) el.scrollTop = el.scrollHeight;
  }, [stick, visible]);

  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  function handleScroll() {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight < BOTTOM_THRESHOLD;
    if (atBottom && paused) setPaused(false);
    else if (!atBottom && !paused) setPaused(true);
  }

  function jumpToLatest() {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    setPaused(false);
  }

  async function copyAll() {
    try {
      await navigator.clipboard.writeText(visible.map((e) => e.line).join("\n"));
      setCopied(true);
      window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      showToast(String(err));
    }
  }

  return (
    <section className="flex min-h-[280px] flex-col rounded-card border border-line bg-surface">
      <header className="flex items-center gap-2 border-b border-line px-3.5 py-2.5">
        <h2 className="text-[12px] font-semibold uppercase tracking-wider text-dim">Logs</h2>
        <span className="text-[11px] text-dim">
          {visible.length === (entries?.length ?? 0)
            ? `${visible.length}`
            : `${visible.length} of ${entries?.length ?? 0}`}
        </span>

        <div className="ml-auto flex items-center gap-2">
          <label className="flex items-center gap-1.5 rounded-md border border-line bg-base px-2 py-1">
            <Search size={12} className="shrink-0 text-dim" />
            <input
              type="text"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter"
              className="w-28 bg-transparent text-[12px] text-ink outline-none placeholder:text-dim/70"
            />
          </label>
          <button
            type="button"
            onClick={() => {
              const next = !autoScroll;
              setAutoScroll(next);
              if (next) jumpToLatest();
            }}
            title={autoScroll ? "Autoscroll on" : "Autoscroll off"}
            aria-pressed={autoScroll}
            className={`flex h-6.5 items-center gap-1 rounded-md border px-2 text-[11.5px] font-medium transition-colors ${
              autoScroll
                ? "border-accent/50 text-accent"
                : "border-line text-dim hover:text-ink"
            }`}
          >
            <ChevronsDown size={12} /> Auto
          </button>
          <button
            type="button"
            onClick={copyAll}
            disabled={visible.length === 0}
            title="Copy visible lines"
            className="flex h-6.5 items-center gap-1 rounded-md border border-line px-2 text-[11.5px] font-medium text-dim transition-colors hover:text-ink disabled:opacity-40"
          >
            {copied ? <Check size={12} className="text-accent" /> : <Copy size={12} />}
            {copied ? "Copied" : "Copy all"}
          </button>
        </div>
      </header>

      <div className="relative min-h-0 flex-1">
        <div
          ref={scrollRef}
          onScroll={handleScroll}
          className="h-full max-h-[320px] overflow-y-auto px-3.5 py-2.5 font-mono text-[12px] leading-relaxed"
        >
          {visible.length === 0 ? (
            <p className="text-dim">
              {filter ? "No lines match the filter." : "No output yet — start the tunnel."}
            </p>
          ) : (
            visible.map((e) => (
              <div key={e.index} className="flex gap-2.5 whitespace-pre-wrap break-all">
                <span className="shrink-0 select-none text-dim/70">{formatClock(e.ts)}</span>
                <span className={e.level === "error" ? "text-danger" : "text-ink"}>
                  {e.line}
                </span>
              </div>
            ))
          )}
        </div>

        {!stick && visible.length > 0 && (
          <button
            type="button"
            onClick={jumpToLatest}
            className="absolute bottom-2.5 right-3 flex items-center gap-1.5 rounded-full border border-line bg-surface px-3 py-1.5 text-[11.5px] font-medium text-ink shadow-md transition-colors hover:border-accent/50"
          >
            <ArrowDownToLine size={12} /> Jump to latest
          </button>
        )}
      </div>
    </section>
  );
}
