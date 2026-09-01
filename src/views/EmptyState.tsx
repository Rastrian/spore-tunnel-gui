import { Plus, Sprout } from "lucide-react";
import { useUi } from "../store/ui";
import { useTunnels } from "../store/tunnels";

/** Main-area placeholder when nothing is selected. */
export function EmptyState() {
  const hasProfiles = useTunnels((s) => s.profiles.length > 0);
  const openEditor = useUi((s) => s.openEditor);
  const openWizard = useUi((s) => s.openWizard);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 px-8 text-center">
      <span className="flex h-14 w-14 items-center justify-center rounded-card bg-surface text-dim">
        <Sprout size={26} />
      </span>
      <div>
        <h2 className="text-[15px] font-semibold">
          {hasProfiles ? "No tunnel selected" : "Welcome to Spore Tunnel"}
        </h2>
        <p className="mt-1 max-w-sm text-[13.5px] leading-relaxed text-dim">
          {hasProfiles
            ? "Pick a tunnel from the list to see its live status, address and logs."
            : "Expose a local service — a Minecraft server, a dev site, anything TCP — through a Spore or Bore tunnel server."}
        </p>
      </div>
      {hasProfiles ? (
        <button
          type="button"
          onClick={() => openEditor(null)}
          className="flex items-center gap-1.5 rounded-card border border-line px-3.5 py-2 text-[13px] font-medium text-ink transition-colors hover:bg-surface"
        >
          <Plus size={15} /> New tunnel
        </button>
      ) : (
        <button
          type="button"
          onClick={() => openWizard()}
          className="rounded-card bg-accent/15 px-4 py-2 text-[13px] font-semibold text-accent transition-colors hover:bg-accent/25"
        >
          Create your first tunnel
        </button>
      )}
    </div>
  );
}
