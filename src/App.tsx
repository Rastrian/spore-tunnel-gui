import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { Sidebar } from "./components/Sidebar";
import { Toast } from "./components/Toast";
import { ProfileEditor, newDraftProfile } from "./components/ProfileEditor";
import { DashboardView } from "./views/DashboardView";
import { EmptyState } from "./views/EmptyState";
import { WizardView } from "./views/WizardView";
import { initTunnelEvents } from "./store/events";
import { useTunnels } from "./store/tunnels";
import { useUi } from "./store/ui";
import { applyTheme, watchSystemTheme } from "./lib/theme";

/** Temporary pane for views landing in later commits of this phase. */
function ComingSoon({ title }: { title: string }) {
  return (
    <div className="flex h-full items-center justify-center text-[13.5px] text-dim">
      {title} — landing in the next change.
    </div>
  );
}

function LoadingScreen() {
  return (
    <div className="flex h-full items-center justify-center text-[13.5px] text-dim">
      Loading…
    </div>
  );
}

export default function App() {
  const hydrated = useTunnels((s) => s.hydrated);
  const profiles = useTunnels((s) => s.profiles);
  const theme = useTunnels((s) => s.uiPrefs?.theme);
  const view = useUi((s) => s.view);
  const selectedProfileId = useUi((s) => s.selectedProfileId);
  const wizardOpen = useUi((s) => s.wizardOpen);
  const editor = useUi((s) => s.editor);
  const setSelection = useUi((s) => s.setSelection);

  // Event subscriptions + one-shot hydration. StrictMode-safe (module-level
  // promise cache inside initTunnelEvents).
  useEffect(() => {
    initTunnelEvents().catch((err) => console.error("event init failed", err));
  }, []);

  // Theme: apply the persisted choice; "system" follows OS changes live.
  const effectiveTheme = theme ?? "system";
  useEffect(() => {
    applyTheme(effectiveTheme);
    if (effectiveTheme !== "system") return;
    return watchSystemTheme(() => applyTheme("system"));
  }, [effectiveTheme]);

  // True first run (hydration found no profiles): open the onboarding wizard
  // exactly once. Skipping leaves the empty state with its own CTA.
  const wizardAutoOpened = useRef(false);
  useEffect(() => {
    if (hydrated && profiles.length === 0 && !wizardAutoOpened.current) {
      wizardAutoOpened.current = true;
      useUi.getState().openWizard();
    }
  }, [hydrated, profiles.length]);

  // Keep a sensible selection without touching the backend's active-profile
  // notion (that write happens on explicit user clicks only).
  useEffect(() => {
    if (!hydrated) return;
    if (selectedProfileId && profiles.some((p) => p.id === selectedProfileId)) {
      return;
    }
    setSelection(profiles[0]?.id ?? null);
  }, [hydrated, profiles, selectedProfileId, setSelection]);

  const selected = profiles.find((p) => p.id === selectedProfileId) ?? null;
  const editing = editor.profileId
    ? (profiles.find((p) => p.id === editor.profileId) ?? null)
    : null;

  let main: ReactNode;
  if (!hydrated) main = <LoadingScreen />;
  else if (wizardOpen) main = <WizardView />;
  else if (view === "settings") main = <ComingSoon title="Settings" />;
  else if (selected) main = <DashboardView profile={selected} />;
  else main = <EmptyState />;

  return (
    <div className="relative flex h-full w-full overflow-hidden bg-base font-sans text-ink">
      <Sidebar />
      <main className="relative min-w-0 flex-1 overflow-y-auto">{main}</main>
      {/* New profile = fresh draft (the editor copies it into local state
          on mount, so re-renders never reset what the user typed). */}
      {editor.open && <ProfileEditor profile={editing ?? newDraftProfile()} />}
      <Toast />
    </div>
  );
}
