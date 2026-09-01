// UI-only slice: navigation, selection, open modals, transient errors.
// Deliberately separate from the event-fed tunnel data in ./tunnels.

import { create } from "zustand";
import { setActiveProfile } from "../lib/api";

export type MainView = "dashboard" | "settings";

interface UiStore {
  view: MainView;
  selectedProfileId: string | null;
  /** Profile editor modal; `profileId: null` = creating a new profile. */
  editor: { open: boolean; profileId: string | null };
  /** Onboarding wizard replaces the main area (first run / no profiles). */
  wizardOpen: boolean;
  /** Transient error toast (invoke failures surfaced as strings). */
  toast: string | null;

  selectProfile(profileId: string): void;
  /** Silent selection (auto-select after hydration / after a delete). */
  setSelection(profileId: string | null): void;
  clearSelection(): void;
  openSettings(): void;
  openDashboard(): void;
  openEditor(profileId: string | null): void;
  closeEditor(): void;
  openWizard(): void;
  closeWizard(): void;
  showToast(message: string): void;
  dismissToast(): void;
}

/** Auto-clear toast after this long (display feedback, not polling). */
const TOAST_MS = 4000;
let toastTimer: number | undefined;

export const useUi = create<UiStore>()((set) => ({
  view: "dashboard",
  selectedProfileId: null,
  editor: { open: false, profileId: null },
  wizardOpen: false,
  toast: null,

  selectProfile: (profileId) => {
    set({ view: "dashboard", selectedProfileId: profileId });
    // Keep the backend's "active profile" notion in sync (it backs the
    // omitted-argument commands); a failure here is harmless — surfaced as
    // a toast so it is never fully silent.
    setActiveProfile(profileId).catch((err: unknown) =>
      useUi.getState().showToast(String(err)),
    );
  },

  clearSelection: () => set({ selectedProfileId: null }),
  setSelection: (profileId) => set({ selectedProfileId: profileId }),
  openSettings: () => set({ view: "settings" }),
  openDashboard: () => set({ view: "dashboard" }),
  openEditor: (profileId) => set({ editor: { open: true, profileId } }),
  closeEditor: () => set({ editor: { open: false, profileId: null } }),
  openWizard: () => set({ view: "dashboard", wizardOpen: true }),
  closeWizard: () => set({ wizardOpen: false }),

  showToast: (message) => {
    set({ toast: message });
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => set({ toast: null }), TOAST_MS);
  },
  dismissToast: () => {
    window.clearTimeout(toastTimer);
    set({ toast: null });
  },
}));
