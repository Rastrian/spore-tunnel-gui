// Theme resolution: the active theme is a `data-theme="light"` attribute on
// <html> (absent = dark). "system" tracks the OS preference live.

import type { Theme, UiPrefs } from "./types";

export const DEFAULT_UI_PREFS: UiPrefs = {
  theme: "system",
  startMinimized: false,
  closeToTray: true,
};

export function resolvedTheme(theme: Theme): "dark" | "light" {
  if (theme !== "system") return theme;
  return window.matchMedia("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

/** Apply a theme choice to the document root. */
export function applyTheme(theme: Theme): void {
  const light = resolvedTheme(theme) === "light";
  if (light) {
    document.documentElement.setAttribute("data-theme", "light");
  } else {
    document.documentElement.removeAttribute("data-theme");
  }
}

/**
 * Watch OS scheme changes while the app follows "system". Returns a cleanup
 * that stops following (call when the user picks an explicit theme).
 */
export function watchSystemTheme(onChange: () => void): () => void {
  const mq = window.matchMedia("(prefers-color-scheme: light)");
  const listener = () => onChange();
  mq.addEventListener("change", listener);
  return () => mq.removeEventListener("change", listener);
}
