import { useState, useEffect, useRef, useCallback } from "react";
import type { AppConfig, TunnelStatus } from "./lib/types";
import * as api from "./lib/api";

const DEFAULT_CONFIG: AppConfig = {
  bore_server_host: "",
  local_host: "127.0.0.1",
  local_port: 25565,
  remote_port: 0,
};

const STATUS_COLORS: Record<string, string> = {
  idle: "#888",
  starting: "#f0ad4e",
  connected: "#5cb85c",
  failed: "#d9534f",
  stopped: "#888",
};

export default function App() {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [status, setStatus] = useState<TunnelStatus | null>(null);
  const [error, setError] = useState("");
  const [copyFeedback, setCopyFeedback] = useState("");
  const [hasStoredSecret, setHasStoredSecret] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);
  const pollRef = useRef<number | null>(null);

  useEffect(() => {
    loadData();
    return () => { if (pollRef.current) clearInterval(pollRef.current); };
  }, []);

  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [status?.logs]);

  const startPolling = useCallback(() => {
    if (pollRef.current) clearInterval(pollRef.current);
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await api.getStatus();
        setStatus(s);
        if (s.state === "stopped" || s.state === "failed") {
          if (pollRef.current) clearInterval(pollRef.current);
          pollRef.current = null;
        }
      } catch { /* ignore */ }
    }, 2000);
  }, []);

  async function loadData() {
    try {
      const [cfg, has] = await Promise.all([api.loadConfig(), api.hasSecret()]);
      setConfig(cfg);
      setHasStoredSecret(has);
      const s = await api.getStatus();
      setStatus(s);
      if (s.state === "starting" || s.state === "connected") {
        startPolling();
      }
    } catch { /* first load, no config yet */ }
  }

  async function handleSaveConfig() {
    try {
      setError("");
      await api.saveConfig(config);
      if (secret) {
        await api.saveSecret(secret);
        setHasStoredSecret(true);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleStart() {
    try {
      setError("");
      const s = await api.startTunnel(config, secret);
      setStatus(s);
      startPolling();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleStop() {
    try {
      setError("");
      await api.stopTunnel();
      const s = await api.getStatus();
      setStatus(s);
      if (pollRef.current) clearInterval(pollRef.current);
      pollRef.current = null;
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleCopy() {
    try {
      const addr = await api.copyAddress();
      await navigator.clipboard.writeText(addr);
      setCopyFeedback("Copied!");
      setTimeout(() => setCopyFeedback(""), 2000);
    } catch (e) {
      setError(String(e));
    }
  }

  const state = status?.state ?? "idle";
  const isRunning = state === "starting" || state === "connected";
  const remoteAddr = status?.remote_address;

  return (
    <div className="app">
      <h1>Bore Minecraft Tunnel</h1>

      <section className="section">
        <label>
          Bore server host
          <input
            type="text"
            placeholder="bore.example.com"
            value={config.bore_server_host}
            onChange={e => setConfig({ ...config, bore_server_host: e.target.value })}
          />
        </label>

        <label>
          Secret
          <div className="secret-row">
            <input
              type={showSecret ? "text" : "password"}
              placeholder={hasStoredSecret ? "(stored)" : "Optional — leave empty for public servers"}
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
            Local port
            <input
              type="number"
              value={config.local_port}
              onChange={e => setConfig({ ...config, local_port: Number(e.target.value) })}
            />
          </label>
          <label className="small">
            Remote port
            <input type="number" value={config.remote_port} disabled title="0 = random" />
          </label>
        </div>

        <button className="btn-secondary" onClick={handleSaveConfig}>
          Save Config
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
            disabled={isRunning || !config.bore_server_host}
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

          {status?.last_error && (
            <div className="error-inline">{status.last_error}</div>
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
