import { Moon, Sun } from "lucide-react";
import { useTunnels } from "../store/tunnels";
import { resolvedTheme } from "../lib/theme";
import { currentPrefs, persistUiPrefs } from "../lib/prefs";

/**
 * Quick dark/light toggle in the sidebar footer. Clicking always pins the
 * explicit theme opposite to the currently resolved one (a "system" follower
 * gets a concrete choice on first click).
 */
export function ThemeToggle() {
  const theme = useTunnels((s) => s.uiPrefs?.theme);
  const resolved = resolvedTheme(theme ?? "system");

  function toggle() {
    const next = resolved === "light" ? "dark" : "light";
    void persistUiPrefs({ ...currentPrefs(), theme: next });
  }

  return (
    <button
      type="button"
      onClick={toggle}
      title={resolved === "light" ? "Switch to dark theme" : "Switch to light theme"}
      className="flex h-8 w-8 items-center justify-center rounded-md text-dim transition-colors hover:bg-surface hover:text-ink"
    >
      {resolved === "light" ? <Moon size={16} /> : <Sun size={16} />}
    </button>
  );
}
