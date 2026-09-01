import type { Profile } from "../lib/types";
import { useTunnels } from "../store/tunnels";
import { STATE_TEXT, stateLabel } from "../lib/statusView";
import { StatusDot } from "../components/StatusDot";

/** Minimal first-cut dashboard (hero screen lands with the stats task). */
export function DashboardView({ profile }: { profile: Profile }) {
  const status = useTunnels((s) => s.statuses[profile.id]);
  const state = status?.state ?? "idle";

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4 px-6 py-8">
      <div className="flex items-center gap-2.5">
        <h1 className="text-lg font-semibold">{profile.name}</h1>
        <span className={`text-[13px] font-medium ${STATE_TEXT[state]}`}>
          {stateLabel(state)}
        </span>
      </div>
      <div className="rounded-card border border-line bg-surface p-4">
        <p className="text-[12px] uppercase tracking-wider text-dim">Local service</p>
        <p className="mt-1 font-mono text-[14px]">{status?.localAddress ?? `${profile.localHost}:${profile.localPort}`}</p>
        <div className="mt-3 flex items-center gap-2">
          <StatusDot state={state} />
          <span className="font-mono text-[12.5px] text-dim">{status?.remoteAddress ?? "not connected"}</span>
        </div>
      </div>
    </div>
  );
}
