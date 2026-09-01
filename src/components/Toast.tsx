import { CircleAlert, X } from "lucide-react";
import { useUi } from "../store/ui";

/** Corner toast for surfaced invoke errors (auto-dismisses, see store). */
export function Toast() {
  const toast = useUi((s) => s.toast);
  const dismiss = useUi((s) => s.dismissToast);
  if (!toast) return null;
  return (
    <div
      role="alert"
      className="absolute bottom-4 left-1/2 z-50 flex max-w-[80%] -translate-x-1/2 items-start gap-2 rounded-card border border-danger/40 bg-surface px-3.5 py-2.5 text-[13px] text-ink shadow-lg"
    >
      <CircleAlert size={16} className="mt-0.5 shrink-0 text-danger" />
      <span className="min-w-0 break-words">{toast}</span>
      <button
        type="button"
        onClick={dismiss}
        aria-label="Dismiss"
        className="ml-1 shrink-0 text-dim transition-colors hover:text-ink"
      >
        <X size={14} />
      </button>
    </div>
  );
}
