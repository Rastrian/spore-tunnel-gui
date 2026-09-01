// UI-preferences persistence: optimistic store update, then update_ui_prefs;
// on failure revert and surface the error string as a toast.

import { updateUiPrefs } from "./api";
import { DEFAULT_UI_PREFS } from "./theme";
import type { UiPrefs } from "./types";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";

export async function persistUiPrefs(prefs: UiPrefs): Promise<void> {
  const previous = useTunnels.getState().uiPrefs;
  useTunnels.getState().setUiPrefs(prefs);
  try {
    const saved = await updateUiPrefs(prefs);
    useTunnels.getState().setUiPrefs(saved);
  } catch (err) {
    useTunnels.getState().setUiPrefs(previous ?? { ...DEFAULT_UI_PREFS });
    useUi.getState().showToast(String(err));
  }
}

/** Current prefs, or the defaults before hydration finished. */
export function currentPrefs(): UiPrefs {
  return useTunnels.getState().uiPrefs ?? { ...DEFAULT_UI_PREFS };
}
