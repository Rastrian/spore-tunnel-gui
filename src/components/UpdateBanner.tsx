import { ArrowUpCircle, X } from "lucide-react";
import { CopyButton } from "./CopyButton";
import { useUi } from "../store/ui";

/**
 * Shell-wide banner shown while an update check found a newer release
 * (result of the Settings "Check for updates" button). No opener plugin
 * is bundled, so the release URL is offered as a copy affordance.
 * Dismissing hides it until the next check.
 */
export function UpdateBanner() {
  const update = useUi((s) => s.updateBanner);
  const dismiss = useUi((s) => s.dismissUpdateBanner);

  if (!update?.updateAvailable) return null;

  return (
    <div
      role="status"
      className="flex items-center justify-between gap-3 border-b border-accent/25 bg-accent/10 px-8 py-2.5"
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <ArrowUpCircle size={15} className="shrink-0 text-accent" />
        <p className="truncate text-[13px]">
          Spore Tunnel{" "}
          <span className="font-mono font-semibold text-accent">{update.latest}</span> is
          available — you have {update.current}.
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <span className="hidden max-w-64 truncate font-mono text-[11.5px] text-dim md:inline">
          {update.url}
        </span>
        <CopyButton text={update.url} title="Copy download link" />
        <button
          type="button"
          onClick={dismiss}
          aria-label="Dismiss update notice"
          title="Dismiss"
          className="flex h-8 w-8 items-center justify-center rounded-md border border-line text-dim transition-colors hover:text-ink"
        >
          <X size={14} />
        </button>
      </div>
    </div>
  );
}
