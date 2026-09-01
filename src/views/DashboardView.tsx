import { useState } from "react";
import { ArrowDown, ArrowUp, Play, RotateCw, Square } from "lucide-react";
import type { Profile } from "../lib/types";
import { startTunnel, stopTunnel } from "../lib/api";
import { formatRate, formatUptime, humanizeBytes } from "../lib/format";
import { STATE_BADGE, STATE_TEXT, isRunning, stateLabel } from "../lib/statusView";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { CopyButton } from "../components/CopyButton";
import { Sparkline } from "../components/Sparkline";
import { StatusRing } from "../components/StatusRing";

/** The hero screen: status, public address, live throughput, controls. */
export function DashboardView({ profile }: { profile: Profile }) {
  const status = useTunnels((s) => s.statuses[profile.id]);
  const samples = useTunnels((s) => s.samples[profile.id]);
  const applyStatus = useTunnels((s) => s.applyStatus);
  const showToast = useUi((s) => s.showToast);
  const [busy, setBusy] = useState(false);

  const state = status?.state ?? "idle";
  const running = isRunning(state);
  const remote = status?.remoteAddress ?? null;
  const last = samples?.[samples.length - 1];

  async function start() {
    setBusy(true);
    try {
      // The returned snapshot is applied immediately; the tunnel://status
      // stream keeps it fresh from here on.
      applyStatus({ profileId: profile.id, status: await startTunnel(profile.id) });
    } catch (err) {
      showToast(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    try {
      // The backend emits a final "stopped" status event right away.
      await stopTunnel(profile.id);
    } catch (err) {
      showToast(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 px-8 py-8">
      {/* Hero */}
      <section className="flex items-center gap-6">
        <div className="relative flex h-24 w-24 shrink-0 items-center justify-center">
          <StatusRing state={state} />
          <span className={`absolute text-[9px] font-bold tracking-widest ${STATE_TEXT[state]}`}>
            {STATE_BADGE[state]}
          </span>
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2.5">
            <h1 className="text-[17px] font-semibold leading-tight">{profile.name}</h1>
            {status?.serverKind && (
              <span
                className={`rounded border px-1.5 py-0.5 font-mono text-[10px] font-bold tracking-wider ${
                  status.serverKind === "Spore"
                    ? "border-accent/50 text-accent"
                    : "border-line text-dim"
                }`}
              >
                {status.serverKind === "Spore" ? "SPORE" : "BORE"}
              </span>
            )}
          </div>
          <p className={`text-[13px] ${STATE_TEXT[state]}`}>{stateLabel(state)}</p>

          {/* Public address chip */}
          <div className="mt-3 flex items-center gap-2">
            {remote ? (
              <span className="flex min-w-0 items-center gap-2 rounded-card border border-line bg-surface px-3 py-2">
                <span className="truncate font-mono text-[15px]">{remote}</span>
                <CopyButton text={remote} title="Copy address" onCopied={(e) => showToast(String(e))} />
              </span>
            ) : (
              <span className="rounded-card border border-dashed border-line px-3 py-2 font-mono text-[13px] text-dim">
                {running ? "waiting for the server to assign a port…" : "not connected"}
              </span>
            )}
          </div>
        </div>

        {/* Controls */}
        <div className="flex shrink-0 flex-col gap-2">
          <button
            type="button"
            onClick={start}
            disabled={busy || running}
            className="flex items-center justify-center gap-1.5 rounded-card bg-accent/15 px-4 py-2 text-[13px] font-semibold text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
          >
            <Play size={14} /> Start
          </button>
          <button
            type="button"
            onClick={stop}
            disabled={busy || !running}
            className="flex items-center justify-center gap-1.5 rounded-card border border-line px-4 py-2 text-[13px] font-semibold text-ink transition-colors hover:bg-surface disabled:cursor-not-allowed disabled:opacity-40"
          >
            <Square size={13} /> Stop
          </button>
        </div>
      </section>

      {/* Failure card */}
      {state === "failed" && status?.lastError && (
        <section className="rounded-card border border-danger/40 bg-danger/5 p-4">
          <p className="text-[12px] font-semibold uppercase tracking-wider text-danger">
            Tunnel failed
          </p>
          <p className="mt-1 break-words font-mono text-[12.5px] leading-relaxed text-ink">
            {status.lastError}
          </p>
          <button
            type="button"
            onClick={start}
            disabled={busy}
            className="mt-3 flex items-center gap-1.5 rounded-card bg-danger/15 px-3.5 py-1.5 text-[13px] font-semibold text-danger transition-colors hover:bg-danger/25 disabled:opacity-50"
          >
            <RotateCw size={14} /> Retry now
          </button>
        </section>
      )}

      {/* Live stats */}
      <section className="grid grid-cols-3 gap-3">
        <Stat label="Uptime" value={formatUptime(status?.uptimeSecs ?? 0)} />
        <Stat
          label="Reconnects"
          value={String(status?.reconnects ?? 0)}
          hint={status?.reconnects ? "tunnel re-established" : undefined}
        />
        <Stat
          label="Transferred"
          value={`${humanizeBytes(status?.bytesUp ?? 0)} / ${humanizeBytes(status?.bytesDown ?? 0)}`}
          hint="up / down"
        />
      </section>

      {/* Throughput */}
      <section className="rounded-card border border-line bg-surface p-4">
        <div className="mb-2 flex items-baseline justify-between">
          <h2 className="text-[12px] font-semibold uppercase tracking-wider text-dim">
            Throughput
          </h2>
          <div className="flex items-center gap-3 font-mono text-[12px]">
            <span className="flex items-center gap-1 text-accent">
              <ArrowUp size={12} /> {formatRate(last?.up ?? 0)}
            </span>
            <span className="flex items-center gap-1 text-dim">
              <ArrowDown size={12} /> {formatRate(last?.down ?? 0)}
            </span>
          </div>
        </div>
        <Sparkline samples={samples ?? []} />
        <p className="mt-1.5 text-[11px] text-dim">last 60 seconds, sampled once per second</p>
      </section>
    </div>
  );
}

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="rounded-card border border-line bg-surface p-3.5">
      <p className="text-[11px] font-semibold uppercase tracking-wider text-dim">{label}</p>
      <p className="mt-1 font-mono text-[15px] font-medium">{value}</p>
      {hint && <p className="text-[11px] text-dim">{hint}</p>}
    </div>
  );
}
