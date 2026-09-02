import { useState } from "react";
import { Pencil, Plus, Settings, Sprout, Trash2 } from "lucide-react";
import { deleteProfile } from "../lib/api";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import type { Profile, TunnelStatus } from "../lib/types";
import { StatusDot } from "./StatusDot";
import { ThemeToggle } from "./ThemeToggle";
import { stateLabel } from "../lib/statusView";

/** Fixed 260px app sidebar: brand, tunnel list, footer controls. */
export function Sidebar() {
  const profiles = useTunnels((s) => s.profiles);
  const statuses = useTunnels((s) => s.statuses);
  const selectedId = useUi((s) => s.selectedProfileId);
  const view = useUi((s) => s.view);
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
          {profiles.map((p) => (
            <SidebarItem
              key={p.id}
              profile={p}
              status={statuses[p.id]}
              selected={view === "dashboard" && selectedId === p.id}
            />
          ))}
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

/**
 * One tunnel row: the main select button plus hover-revealed edit and delete
 * actions. Extracted so the two-click delete confirmation state stays local
 * to a single item.
 */
function SidebarItem({
  profile: p,
  status,
  selected,
}: {
  profile: Profile;
  status?: TunnelStatus;
  selected: boolean;
}) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);

  const selectProfile = useUi((s) => s.selectProfile);
  const openEditor = useUi((s) => s.openEditor);
  const showToast = useUi((s) => s.showToast);
  const removeProfile = useTunnels((s) => s.removeProfile);

  const state = status?.state ?? "idle";

  async function destroy() {
    setBusy(true);
    try {
      await deleteProfile(p.id);
      removeProfile(p.id);
    } catch (err) {
      // e.g. "Stop the tunnel for this profile before deleting it." —
      // surface the backend's reason verbatim and disarm.
      showToast(String(err));
      setConfirmDelete(false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <li className="group relative" onMouseLeave={() => setConfirmDelete(false)}>
      <button
        type="button"
        onClick={() => selectProfile(p.id)}
        className={`w-full rounded-card py-2 pl-2.5 pr-14 text-left transition-colors ${
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
      {/* Discreet per-item edit: revealed on hover/focus. Selects
          first so the dashboard behind the editor matches. */}
      <button
        type="button"
        onClick={() => {
          selectProfile(p.id);
          openEditor(p.id);
        }}
        title="Edit tunnel"
        aria-label={`Edit ${p.name}`}
        className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-dim opacity-0 transition hover:bg-surface hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
      >
        <Pencil size={12} />
      </button>
      {/* Two-click delete: the icon arms an inline "Really delete?" pill
          in the same spot; the second click runs it. Leaving the row
          disarms; errors (e.g. tunnel running) toast the reason. Same
          button node morphs between the two states so focus is kept. */}
      <button
        type="button"
        onClick={confirmDelete ? destroy : () => setConfirmDelete(true)}
        disabled={busy}
        title={confirmDelete ? "Confirm delete" : `Delete ${p.name}`}
        aria-label={confirmDelete ? "Really delete?" : `Delete ${p.name}`}
        className={
          confirmDelete
            ? "absolute right-8 top-1/2 flex h-6 -translate-y-1/2 items-center rounded-md bg-danger/15 px-2 text-[11px] font-semibold text-danger transition hover:bg-danger/25 disabled:opacity-50"
            : "absolute right-8 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-dim opacity-0 transition hover:bg-surface hover:text-danger focus-visible:opacity-100 group-hover:opacity-100"
        }
      >
        {confirmDelete ? "Really delete?" : <Trash2 size={12} />}
      </button>
    </li>
  );
}
