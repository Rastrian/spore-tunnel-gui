import { useEffect, useState } from "react";
import { FolderOpen, Import, Monitor, Moon, RefreshCw, Sun } from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as autostartIsEnabled,
} from "@tauri-apps/plugin-autostart";
import { checkForUpdates, importLegacy, openConfigFolder } from "../lib/api";
import {
  defaultStorage,
  formatLastChecked,
  readLastChecked,
  writeLastChecked,
} from "../lib/updates";
import type { Theme } from "../lib/types";
import { currentPrefs, persistUiPrefs } from "../lib/prefs";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { Toggle } from "../components/fields";

const THEME_OPTIONS: { id: Theme; label: string; icon: typeof Sun }[] = [
  { id: "dark", label: "Dark", icon: Moon },
  { id: "light", label: "Light", icon: Sun },
  { id: "system", label: "System", icon: Monitor },
];

export function SettingsView() {
  const prefs = useTunnels((s) => s.uiPrefs);
  const hasLegacy = useTunnels((s) => s.hasLegacy);
  const upsert = useTunnels((s) => s.upsertProfile);
  const setHasLegacy = useTunnels((s) => s.setHasLegacy);
  const setSelection = useUi((s) => s.setSelection);
  const showToast = useUi((s) => s.showToast);
  const showUpdateBanner = useUi((s) => s.showUpdateBanner);
  const dismissUpdateBanner = useUi((s) => s.dismissUpdateBanner);

  const [version, setVersion] = useState("");
  const [importBusy, setImportBusy] = useState(false);
  const [importMessage, setImportMessage] = useState("");
  const [importError, setImportError] = useState("");
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateMessage, setUpdateMessage] = useState("");
  const [lastChecked, setLastChecked] = useState<number | null>(() =>
    readLastChecked(defaultStorage()),
  );
  // OS launch entry is the source of truth (no config mirror to drift).
  const [autostart, setAutostart] = useState(false);

  useEffect(() => {
    autostartIsEnabled()
      .then(setAutostart)
      .catch((err: unknown) => console.error("autostart isEnabled failed", err));
  }, []);

  /** Optimistic toggle; re-reads the OS state so a failed write reverts. */
  async function setAutostartEnabled(enabled: boolean) {
    const previous = autostart;
    setAutostart(enabled);
    try {
      if (enabled) {
        await enableAutostart();
      } else {
        await disableAutostart();
      }
      setAutostart(await autostartIsEnabled());
    } catch (err) {
      setAutostart(previous);
      showToast(String(err));
    }
  }

  async function doUpdateCheck() {
    setUpdateBusy(true);
    setUpdateMessage("");
    try {
      const status = await checkForUpdates();
      const now = Date.now();
      writeLastChecked(now, defaultStorage());
      setLastChecked(now);
      if (status.updateAvailable) {
        showUpdateBanner(status);
        setUpdateMessage(`Version ${status.latest} is available.`);
      } else {
        // A calm re-check clears any banner from an earlier check.
        dismissUpdateBanner();
        setUpdateMessage(`You're up to date (${status.current}).`);
      }
    } catch (err) {
      setUpdateMessage(String(err));
    } finally {
      setUpdateBusy(false);
    }
  }

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch((err: unknown) => console.error("getVersion failed", err));
  }, []);

  const theme = prefs?.theme ?? "system";

  async function doImport() {
    setImportBusy(true);
    setImportMessage("");
    setImportError("");
    try {
      const profile = await importLegacy();
      if (profile) {
        upsert(profile);
        setSelection(profile.id);
        // The legacy config was consumed; hide the card.
        setHasLegacy(false);
        setImportMessage(`Imported "${profile.name}" — it is selected in the sidebar.`);
      } else {
        setHasLegacy(false);
        setImportMessage("Nothing to import.");
      }
    } catch (err) {
      setImportError(String(err));
    } finally {
      setImportBusy(false);
    }
  }

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-5 px-8 py-8">
      <h1 className="text-[17px] font-semibold">Settings</h1>

      {/* Appearance */}
      <Section title="Appearance">
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[13px] font-medium">Theme</p>
            <p className="text-[11.5px] text-dim">
              "System" follows your OS color scheme and keeps following it.
            </p>
          </div>
          <div className="flex overflow-hidden rounded-card border border-line">
            {THEME_OPTIONS.map((opt) => (
              <button
                key={opt.id}
                type="button"
                onClick={() => void persistUiPrefs({ ...currentPrefs(), theme: opt.id })}
                aria-pressed={theme === opt.id}
                className={`flex items-center gap-1.5 px-3 py-1.5 text-[12.5px] font-medium transition-colors ${
                  theme === opt.id
                    ? "bg-accent/15 text-accent"
                    : "text-dim hover:bg-base hover:text-ink"
                }`}
              >
                <opt.icon size={13} /> {opt.label}
              </button>
            ))}
          </div>
        </div>
      </Section>

      {/* Behavior */}
      <Section title="Behavior">
        <Toggle
          checked={autostart}
          onChange={(enabled) => void setAutostartEnabled(enabled)}
          label="Start Spore Tunnel at login"
          description="Launch the app when you sign in; profiles set to “Start with the app” connect automatically"
        />
        <Toggle
          checked={prefs?.startMinimized ?? false}
          onChange={(startMinimized) =>
            void persistUiPrefs({ ...currentPrefs(), startMinimized })
          }
          label="Start minimized"
          description="Launch into the system tray instead of opening the window"
        />
        <Toggle
          checked={prefs?.closeToTray ?? true}
          onChange={(closeToTray) => void persistUiPrefs({ ...currentPrefs(), closeToTray })}
          label="Close to tray"
          description="Keep tunnels running when the window is closed"
        />
        <div className="flex items-center justify-between gap-4 pt-1">
          <div>
            <p className="text-[13px] font-medium">Updates</p>
            <p className="text-[11.5px] text-dim">
              {updateMessage || formatLastChecked(lastChecked)}
            </p>
          </div>
          <button
            type="button"
            onClick={() => void doUpdateCheck()}
            disabled={updateBusy}
            className="flex items-center gap-1.5 rounded-card border border-line bg-base px-3.5 py-1.5 text-[12.5px] font-medium text-ink transition-colors hover:border-accent/50 disabled:opacity-50"
          >
            <RefreshCw size={13} className={updateBusy ? "animate-spin" : undefined} />
            {updateBusy ? "Checking…" : "Check for updates"}
          </button>
        </div>
      </Section>

      {/* Legacy import — only while an old config is around. */}
      {hasLegacy && (
        <Section title="Import">
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium">Bore Minecraft Tunnel</p>
              <p className="text-[11.5px] text-dim">
                An old bore-tunnel-gui config was found on this machine.
              </p>
            </div>
            <button
              type="button"
              onClick={doImport}
              disabled={importBusy}
              className="flex items-center gap-1.5 rounded-card border border-line bg-base px-3.5 py-1.5 text-[12.5px] font-medium text-ink transition-colors hover:border-accent/50 disabled:opacity-50"
            >
              <Import size={13} /> {importBusy ? "Importing…" : "Import"}
            </button>
          </div>
          {importError && (
            <p role="alert" className="mt-2 text-[12.5px] text-danger">{importError}</p>
          )}
        </Section>
      )}
      {importMessage && !hasLegacy && (
        // Keep the confirmation visible after the card disappears.
        <p className="text-[12.5px] text-accent">{importMessage}</p>
      )}

      {/* Storage */}
      <Section title="Storage">
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[13px] font-medium">Config folder</p>
            <p className="text-[11.5px] text-dim">
              Profiles live in config.json; secrets stay in the OS keyring.
            </p>
          </div>
          <button
            type="button"
            onClick={() =>
              openConfigFolder().catch((err: unknown) => showToast(String(err)))
            }
            className="flex items-center gap-1.5 rounded-card border border-line bg-base px-3.5 py-1.5 text-[12.5px] font-medium text-ink transition-colors hover:border-accent/50"
          >
            <FolderOpen size={13} /> Open
          </button>
        </div>
      </Section>

      {/* About */}
      <Section title="About">
        <div className="flex items-baseline gap-2">
          <p className="text-[13px] font-semibold">Spore Tunnel</p>
          <p className="font-mono text-[12px] text-dim">{version || "…"}</p>
        </div>
        <p className="mt-1 text-[12.5px] leading-relaxed text-dim">
          Expose local TCP services through Spore and Bore tunnel servers, natively —
          no external binaries.
        </p>
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-card border border-line bg-surface p-4">
      <h2 className="mb-2.5 text-[12px] font-semibold uppercase tracking-wider text-dim">
        {title}
      </h2>
      <div className="flex flex-col gap-1.5">{children}</div>
    </section>
  );
}
