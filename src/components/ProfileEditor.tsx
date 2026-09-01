import { useState } from "react";
import { Eye, EyeOff, Save, Trash2 } from "lucide-react";
import type { Profile } from "../lib/types";
import { deleteProfile, saveProfile, setProfileSecret } from "../lib/api";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { Modal } from "./Modal";
import { FieldRow, NumberField, TextField, Toggle } from "./fields";

/** Draft for a brand-new profile (id is replaced by the backend on save). */
export function newDraftProfile(): Profile {
  return {
    id: crypto.randomUUID(),
    name: "New tunnel",
    serverHost: "",
    serverPort: 7835,
    localHost: "127.0.0.1",
    localPort: 25565,
    remotePort: 0,
    autostart: false,
    autoReconnect: true,
  };
}

/**
 * Create/edit modal. The secret field starts blank and stays blank unless
 * typed into — blank means "keep the stored secret", and the stored value is
 * never displayed or echoed back.
 */
export function ProfileEditor({ profile }: { profile: Profile }) {
  // "Creating" = the profile is not in the store yet (fresh draft).
  const creating = useTunnels((s) => !s.profiles.some((p) => p.id === profile.id));
  const [draft, setDraft] = useState<Profile>(profile);
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [autoPort, setAutoPort] = useState(profile.remotePort === 0);
  const [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);

  const upsert = useTunnels((s) => s.upsertProfile);
  const removeProfile = useTunnels((s) => s.removeProfile);
  const close = useUi((s) => s.closeEditor);

  const patch = (p: Partial<Profile>) => setDraft((d) => ({ ...d, ...p }));
  const valid =
    draft.name.trim() !== "" &&
    draft.serverHost.trim() !== "" &&
    draft.localHost.trim() !== "" &&
    draft.serverPort > 0 &&
    draft.localPort > 0 &&
    (autoPort || draft.remotePort > 0);

  async function save() {
    setError("");
    setBusy(true);
    try {
      const toSave: Profile = {
        ...draft,
        remotePort: autoPort ? 0 : draft.remotePort,
      };
      const saved = await saveProfile(toSave);
      if (secret.trim()) await setProfileSecret(saved.id, secret.trim());
      upsert(saved);
      close();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function destroy() {
    setError("");
    setBusy(true);
    try {
      await deleteProfile(draft.id);
      removeProfile(draft.id);
      close();
    } catch (err) {
      // e.g. "profile is running" — surface the backend's reason verbatim.
      setError(String(err));
      setConfirmDelete(false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal title={creating ? "New tunnel" : `Edit ${profile.name}`} onClose={close}>
      <div className="flex flex-col gap-3.5 p-4">
        <TextField
          label="Profile name"
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          autoFocus
        />

        <div>
          <span className="mb-1 block text-[12px] font-medium text-dim">Tunnel server</span>
          <FieldRow>
            <TextField
              label="Host"
              className="col-span-2"
              placeholder="bore.pub or your Spore server"
              value={draft.serverHost}
              onChange={(e) => patch({ serverHost: e.target.value })}
            />
            <NumberField
              label="Port"
              value={draft.serverPort}
              onValue={(serverPort) => patch({ serverPort })}
            />
          </FieldRow>
        </div>

        <div>
          <span className="mb-1 block text-[12px] font-medium text-dim">Local service</span>
          <FieldRow>
            <TextField
              label="Host"
              className="col-span-2"
              value={draft.localHost}
              onChange={(e) => patch({ localHost: e.target.value })}
            />
            <NumberField
              label="Port"
              value={draft.localPort}
              onValue={(localPort) => patch({ localPort })}
            />
          </FieldRow>
        </div>

        <div>
          <span className="mb-1 block text-[12px] font-medium text-dim">Public port</span>
          <div className="flex items-center gap-3">
            <Toggle
              checked={autoPort}
              onChange={setAutoPort}
              label="Auto (random)"
              description="Let the server pick a free public port"
            />
            <div className="w-28">
              <NumberField
                label="Port"
                value={draft.remotePort}
                onValue={(remotePort) => patch({ remotePort })}
                disabled={autoPort}
              />
            </div>
          </div>
        </div>

        <div>
          <span className="mb-1 flex items-center justify-between text-[12px] font-medium text-dim">
            Secret
            <button
              type="button"
              onClick={() => setShowSecret((v) => !v)}
              className="flex items-center gap-1 text-[11px] text-dim transition-colors hover:text-ink"
            >
              {showSecret ? <EyeOff size={12} /> : <Eye size={12} />}
              {showSecret ? "Hide" : "Show"}
            </button>
          </span>
          <input
            type={showSecret ? "text" : "password"}
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={creating ? "Optional — empty for public servers" : "Leave blank to keep the stored secret"}
            className="w-full rounded-md border border-line bg-base px-2.5 py-1.5 font-mono text-[13px] text-ink outline-none transition-colors placeholder:font-sans placeholder:text-dim/70 focus:border-accent/60"
          />
          <p className="mt-1 text-[11.5px] leading-relaxed text-dim">
            Public relays (bore.pub-style) need none; private Spore servers use a shared secret.
          </p>
        </div>

        <div className="border-t border-line pt-2">
          <Toggle
            checked={draft.autoReconnect}
            onChange={(autoReconnect) => patch({ autoReconnect })}
            label="Reconnect automatically"
            description="Retry with backoff when the tunnel drops"
          />
          <Toggle
            checked={draft.autostart}
            onChange={(autostart) => patch({ autostart })}
            label="Start with the app"
            description="Connect this tunnel when Spore Tunnel launches"
          />
        </div>

        {error && (
          <p role="alert" className="rounded-md border border-danger/40 bg-danger/5 px-3 py-2 text-[12.5px] text-danger">
            {error}
          </p>
        )}

        <footer className="flex items-center justify-between border-t border-line pt-3">
          {!creating ? (
            confirmDelete ? (
              <button
                type="button"
                onClick={destroy}
                disabled={busy}
                className="flex items-center gap-1.5 rounded-card bg-danger/15 px-3 py-1.5 text-[12.5px] font-semibold text-danger transition-colors hover:bg-danger/25 disabled:opacity-50"
              >
                <Trash2 size={13} /> Really delete?
              </button>
            ) : (
              <button
                type="button"
                onClick={() => setConfirmDelete(true)}
                className="flex items-center gap-1.5 rounded-card px-2 py-1.5 text-[12.5px] font-medium text-danger transition-colors hover:bg-danger/10"
              >
                <Trash2 size={13} /> Delete
              </button>
            )
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={close}
              className="rounded-card px-3 py-1.5 text-[13px] font-medium text-dim transition-colors hover:text-ink"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={save}
              disabled={busy || !valid}
              className="flex items-center gap-1.5 rounded-card bg-accent/15 px-3.5 py-1.5 text-[13px] font-semibold text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
            >
              <Save size={13} /> Save
            </button>
          </div>
        </footer>
      </div>
    </Modal>
  );
}
