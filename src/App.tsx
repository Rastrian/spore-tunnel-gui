import { useState, useEffect, useRef, useCallback } from "react";
import type { Profile, TunnelStatus } from "./lib/types";
import * as api from "./lib/api";

const STATUS_COLORS: Record<string, string> = {
  idle: "#888",
  starting: "#f0ad4e",
  connected: "#5cb85c",
  failed: "#d9534f",
  stopped: "#888",
};

/** Draft for the very first profile (nothing saved on the backend yet). */
function newDraftProfile(): Profile {
  return {
    id: crypto.randomUUID(),
    name: "My tunnel",
    serverHost: "",
    serverPort: 7835,
    localHost: "127.0.0.1",
    localPort: 25565,
    remotePort: 0,
    autostart: false,
    autoReconnect: true,
  };
}

export default function App() {
  // The single-tunnel UI edits the working profile (the first configured
  // one; the backend keeps it active). A fresh draft is created when no
  // profile exists yet.
  const [profile, setProfile] = useState<Profile>(newDraftProfile);
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [status, setStatus] = useState<TunnelStatus | null>(null);
  const [error, setError] = useState("");
  const [copyFeedback, setCopyFeedback] = useState("");
  const logRef = useRef<HTMLDivElement>(null);
  const pollRef = useRef<number | null>(null);

  useEffect(() => {
    load();
    return () => { if (pollRef.current) clearInterval(pollRef.current); };
  }, []);

  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [status?.logs]);

  const startPolling = useCallback((profileId: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await api.getStatus(profileId);
        setStatus(s);
        if (s.state === "stopped" || s.state === "failed") {
          if (pollRef.current) clearInterval(pollRef.current);
          pollRef.current = null;
        }
      } catch { /* ignore */ }
    }, 2000);
  }, []);

  async function load() {
    try {
      const [profiles, all] = await Promise.all([api.listProfiles(), api.getAllStatus()]);
      if (profiles.length === 0) return;
      const working = profiles[0];
      setProfile(working);
      const current = all.find(s => s.profileId === working.id)?.status ?? null;
      setStatus(current);
      if (current && (current.state === "starting" || current.state === "connected")) {
        startPolling(working.id);
      }
    } catch { /* first load, no config yet */ }
  }

  /** Save the form as the profile (plus the secret, when entered). */
  async function persistProfile(): Promise<Profile> {
    const saved = await api.saveProfile(profile);
    setProfile(saved);
    if (secret.trim()) {
      await api.setProfileSecret(saved.id, secret.trim());
    }
    return saved;
  }

  async function handleSave() {
    try {
      setError("");
      await persistProfile();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleStart() {
    try {
      setError("");
      // Start always saves first: the tunnel runs what the form shows.
      const saved = await persistProfile();
      const s = await api.startTunnel(saved.id, secret.trim() || undefined);
      setStatus(s);
      startPolling(saved.id);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleStop() {
    try {
      setError("");
      await api.stopTunnel(profile.id);
      const s = await api.getStatus(profile.id);
      setStatus(s);
      if (pollRef.current) clearInterval(pollRef.current);
      pollRef.current = null;
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleCopy() {
    try {
      const addr = await api.copyAddress(profile.id);
      await navigator.clipboard.writeText(addr);
      setCopyFeedback("Copied!");
      setTimeout(() => setCopyFeedback(""), 2000);
    } catch (e) {
      setError(String(e));
    }
  }

  const state = status?.state ?? "idle";
  const isRunning = state === "starting" || state === "connected";
  const remoteAddr = status?.remoteAddress;

  return (
    <div className="app">
      <h1>Spore Tunnel</h1>

      <section className="section">
        <label>
          Profile name
          <input
            type="text"
            value={profile.name}
            onChange={e => setProfile({ ...profile, name: e.target.value })}
          />
        </label>

        <label>
          Tunnel server host
          <input
            type="text"
            placeholder="bore.example.com"
            value={profile.serverHost}
            onChange={e => setProfile({ ...profile, serverHost: e.target.value })}
          />
        </label>

        <label>
          Secret
          <div className="secret-row">
            <input
              type={showSecret ? "text" : "password"}
              placeholder="Optional — leave empty for public servers"
              value={secret}
              onChange={e => setSecret(e.target.value)}
            />
            <button className="btn-small" onClick={() => setShowSecret(!showSecret)}>
              {showSecret ? "Hide" : "Show"}
            </button>
          </div>
        </label>

        <div className="row">
          <label className="small">
            Server port
            <input
              type="number"
              value={profile.serverPort}
              onChange={e => setProfile({ ...profile, serverPort: Number(e.target.value) })}
            />
          </label>
          <label className="small">
            Local port
            <input
              type="number"
              value={profile.localPort}
              onChange={e => setProfile({ ...profile, localPort: Number(e.target.value) })}
            />
          </label>
          <label className="small">
            Remote port
            <input type="number" value={profile.remotePort} disabled title="0 = random" />
          </label>
        </div>

        <label>
          <input
            type="checkbox"
            checked={profile.autoReconnect}
            onChange={e => setProfile({ ...profile, autoReconnect: e.target.checked })}
          />
          Reconnect automatically when the tunnel drops
        </label>

        <button className="btn-secondary" onClick={handleSave}>
          Save Profile
        </button>
        <button className="btn-secondary" onClick={() => api.openConfigFolder()}>
          Open Config Folder
        </button>
      </section>

      <section className="section">
        <div className="button-row">
          <button
            className="btn-primary"
            onClick={handleStart}
            disabled={isRunning || !profile.serverHost.trim()}
          >
            Start Tunnel
          </button>
          <button
            className="btn-danger"
            onClick={handleStop}
            disabled={!isRunning}
          >
            Stop Tunnel
          </button>
        </div>

        <div className="status-box">
          <div className="status-row">
            <span>Status:</span>
            <span style={{ color: STATUS_COLORS[state] ?? "#888" }}>
              {state.charAt(0).toUpperCase() + state.slice(1)}
            </span>
          </div>

          {remoteAddr && (
            <div className="status-row">
              <span>Public address:</span>
              <span className="address">{remoteAddr}</span>
              <button className="btn-small" onClick={handleCopy}>
                {copyFeedback || "Copy"}
              </button>
            </div>
          )}

          {status?.lastError && (
            <div className="error-inline">{status.lastError}</div>
          )}
        </div>
      </section>

      {error && <div className="error">{error}</div>}

      <section className="section logs-section">
        <h3>Logs</h3>
        <div className="logs" ref={logRef}>
          {(status?.logs ?? []).map((line, i) => (
            <div key={i} className="log-line">{line}</div>
          ))}
          {(!status?.logs?.length) && <div className="log-line muted">No logs yet.</div>}
        </div>
      </section>
    </div>
  );
}
