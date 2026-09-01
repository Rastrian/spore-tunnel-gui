import { Plus, Settings, Sprout } from "lucide-react";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { StatusDot } from "./StatusDot";
import { ThemeToggle } from "./ThemeToggle";
import { stateLabel } from "../lib/statusView";

/** Fixed 260px app sidebar: brand, tunnel list, footer controls. */
export function Sidebar() {
  const profiles = useTunnels((s) => s.profiles);
  const statuses = useTunnels((s) => s.statuses);
  const selectedId = useUi((s) => s.selectedProfileId);
  const view = useUi((s) => s.view);
  const selectProfile = useUi((s) => s.selectProfile);
  const openSettings = useUi((s) => s.openSettings);
  const openEditor = useUi((s) => s.openEditor);
  const openWizard = useUi((s) => s.openWizard);

  const onNewTunnel = () => {
    // No profiles yet -> the full onboarding wizard; otherwise the editor.
    if (profiles.length === 0) openWizard();
    else openEditor(null);
  };

  return (
    <aside className="flex h-full w-[260px] shrink-0 flex-col border-r border-line bg-surface">
      <div className="flex items-center gap-2.5 px-4 py-4">
        <span className="flex h-8 w-8 items-center justify-center rounded-card bg-accent/15 text-accent">
          <Sprout size={18} />
        </span>
        <span className="text-[15px] font-semibold tracking-tight">Spore Tunnel</span>
      </div>

      <div className="flex items-center justify-between px-4 pb-1.5">
        <span className="text-[11px] font-medium uppercase tracking-wider text-dim">
          Tunnels
        </span>
        <button
          type="button"
          onClick={onNewTunnel}
          title="New tunnel"
          className="flex h-6 w-6 items-center justify-center rounded-md text-dim transition-colors hover:bg-base hover:text-ink"
        >
          <Plus size={15} />
        </button>
      </div>

      <nav className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {profiles.length === 0 && (
          <p className="px-2 py-2 text-[13px] leading-relaxed text-dim">
            No tunnels yet. Add one to expose a local service through a Spore or
            Bore server.
          </p>
        )}
        <ul className="space-y-0.5">
          {profiles.map((p) => {
            const status = statuses[p.id];
            const state = status?.state ?? "idle";
            const selected = view === "dashboard" && selectedId === p.id;
            return (
              <li key={p.id}>
                <button
                  type="button"
                  onClick={() => selectProfile(p.id)}
                  className={`w-full rounded-card px-2.5 py-2 text-left transition-colors ${
                    selected ? "bg-base" : "hover:bg-base/60"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <StatusDot state={state} />
                    <span
                      className={`truncate text-[13.5px] font-medium ${
                        selected ? "text-ink" : "text-ink/90"
                      }`}
                    >
                      {p.name}
                    </span>
                  </span>
                  <span className="mt-0.5 block truncate font-mono text-[11.5px] text-dim">
                    {status?.remoteAddress ?? stateLabel(state)}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      </nav>

      <div className="flex items-center justify-between border-t border-line px-3 py-2.5">
        <ThemeToggle />
        <button
          type="button"
          onClick={openSettings}
          title="Settings"
          aria-label="Settings"
          aria-current={view === "settings" ? "page" : undefined}
          className={`flex h-8 w-8 items-center justify-center rounded-md transition-colors hover:bg-base hover:text-ink ${
            view === "settings" ? "text-ink" : "text-dim"
          }`}
        >
          <Settings size={16} />
        </button>
      </div>
    </aside>
  );
}
