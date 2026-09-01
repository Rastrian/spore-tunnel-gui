import { useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Box,
  Gamepad2,
  Globe,
  Radar,
  Rocket,
  Sprout,
  Wrench,
} from "lucide-react";
import type { DetectedService, Profile } from "../lib/types";
import { detectLocalService, saveProfile, setProfileSecret, startTunnel } from "../lib/api";
import { useTunnels } from "../store/tunnels";
import { useUi } from "../store/ui";
import { NumberField, TextField, Toggle } from "../components/fields";

/** What the wizard is hosting — drives the profile name and port defaults. */
interface Preset {
  id: string;
  title: string;
  hint: string;
  icon: typeof Sprout;
  defaultName: string;
  defaultPort: number;
}

const PRESETS: Preset[] = [
  {
    id: "minecraft",
    title: "Minecraft (Java)",
    hint: "Local server on port 25565",
    icon: Gamepad2,
    defaultName: "Minecraft server",
    defaultPort: 25565,
  },
  {
    id: "game",
    title: "Other game",
    hint: "Terraria, Valheim, factorio…",
    icon: Box,
    defaultName: "Game server",
    defaultPort: 0,
  },
  {
    id: "web",
    title: "Web app",
    hint: "Dev server or website",
    icon: Globe,
    defaultName: "Web app",
    defaultPort: 8080,
  },
  {
    id: "custom",
    title: "Custom",
    hint: "Any TCP service",
    icon: Wrench,
    defaultName: "My service",
    defaultPort: 0,
  },
];

const STEPS = ["What are you hosting?", "Local service", "Tunnel server"];

/** First-run onboarding: hosting preset -> local service -> server + save. */
export function WizardView() {
  const [step, setStep] = useState(0);
  const [localHost, setLocalHost] = useState("127.0.0.1");
  const [localPort, setLocalPort] = useState(0);
  const [detected, setDetected] = useState<DetectedService[] | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [detectError, setDetectError] = useState("");
  const [name, setName] = useState("");
  const [serverHost, setServerHost] = useState("");
  const [serverPort, setServerPort] = useState(7835);
  const [secret, setSecret] = useState("");
  const [connectNow, setConnectNow] = useState(true);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const upsert = useTunnels((s) => s.upsertProfile);
  const applyStatus = useTunnels((s) => s.applyStatus);
  const setSelection = useUi((s) => s.setSelection);
  const closeWizard = useUi((s) => s.closeWizard);

  function choosePreset(p: Preset) {
    setLocalPort(p.defaultPort);
    setName(p.defaultName);
    setStep(1);
  }

  async function detect() {
    setDetecting(true);
    setDetectError("");
    try {
      setDetected(await detectLocalService());
    } catch (err) {
      setDetectError(String(err));
    } finally {
      setDetecting(false);
    }
  }

  const localValid = localHost.trim() !== "" && localPort > 0 && localPort <= 65535;
  const serverValid = serverHost.trim() !== "" && serverPort > 0 && name.trim() !== "";

  async function create() {
    setError("");
    setBusy(true);
    try {
      const profile: Profile = {
        id: crypto.randomUUID(),
        name: name.trim(),
        serverHost: serverHost.trim(),
        serverPort,
        localHost: localHost.trim(),
        localPort,
        remotePort: 0, // wizard always lets the server assign
        autostart: false,
        autoReconnect: true,
      };
      const saved = await saveProfile(profile);
      if (secret.trim()) await setProfileSecret(saved.id, secret.trim());
      upsert(saved);
      setSelection(saved.id);
      closeWizard();
      if (connectNow) {
        // Apply the returned snapshot immediately; the event stream takes
        // over from there. A connect failure is a toast, not a dead end —
        // the profile exists and can be retried from the dashboard.
        applyStatus({
          profileId: saved.id,
          status: await startTunnel(saved.id, secret.trim() || undefined),
        });
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col px-8 py-8">
      {/* Stepper */}
      <div className="mb-6 flex items-center gap-2">
        {STEPS.map((title, i) => (
          <div key={title} className="flex items-center gap-2">
            <span
              className={`flex h-6 w-6 items-center justify-center rounded-full border text-[11px] font-bold ${
                i < step
                  ? "border-accent/60 text-accent"
                  : i === step
                    ? "border-accent text-accent"
                    : "border-line text-dim"
              }`}
            >
              {i + 1}
            </span>
            <span className={`text-[12px] ${i === step ? "font-medium text-ink" : "text-dim"}`}>
              {title}
            </span>
            {i < STEPS.length - 1 && <span className="mx-1 h-px w-6 bg-line" />}
          </div>
        ))}
      </div>

      {step === 0 && (
        <section>
          <h1 className="text-[17px] font-semibold">What are you hosting?</h1>
          <p className="mt-1 text-[13px] text-dim">
            This just pre-fills the profile — everything stays editable.
          </p>
          <div className="mt-4 grid grid-cols-2 gap-3">
            {PRESETS.map((p) => (
              <button
                key={p.id}
                type="button"
                onClick={() => choosePreset(p)}
                className="flex items-start gap-3 rounded-card border border-line bg-surface p-4 text-left transition-colors hover:border-accent/50 hover:bg-base"
              >
                <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-accent/10 text-accent">
                  <p.icon size={18} />
                </span>
                <span>
                  <span className="block text-[13.5px] font-semibold">{p.title}</span>
                  <span className="block text-[12px] text-dim">{p.hint}</span>
                </span>
              </button>
            ))}
          </div>

          <div className="mt-5 border-t border-line pt-4">
            <button
              type="button"
              onClick={closeWizard}
              className="text-[12.5px] text-dim underline-offset-2 transition-colors hover:text-ink hover:underline"
            >
              Skip for now
            </button>
          </div>
        </section>
      )}

      {step === 1 && (
        <section className="flex flex-col gap-4">
          <div>
            <h1 className="text-[17px] font-semibold">Where is it running?</h1>
            <p className="mt-1 text-[13px] text-dim">
              Look for a listening service on this machine, or enter the address by hand.
            </p>
          </div>

          <div>
            <button
              type="button"
              onClick={detect}
              disabled={detecting}
              className="flex items-center gap-2 rounded-card border border-line bg-surface px-3.5 py-2 text-[13px] font-medium text-ink transition-colors hover:bg-base disabled:opacity-50"
            >
              <Radar size={15} className={detecting ? "animate-spin" : ""} />
              {detecting ? "Scanning…" : "Detect local services"}
            </button>
            {detectError && <p className="mt-2 text-[12.5px] text-danger">{detectError}</p>}
            {detected && detected.length === 0 && (
              <p className="mt-2 text-[12.5px] text-dim">
                Nothing detected — enter the address manually below.
              </p>
            )}
            {detected && detected.length > 0 && (
              <ul className="mt-2 divide-y divide-line overflow-hidden rounded-card border border-line">
                {detected.map((d) => (
                  <li key={d.port}>
                    <button
                      type="button"
                      onClick={() => setLocalPort(d.port)}
                      className={`flex w-full items-center justify-between px-3.5 py-2.5 text-left text-[13px] transition-colors hover:bg-base ${
                        localPort === d.port ? "bg-base text-accent" : ""
                      }`}
                    >
                      <span>{d.name}</span>
                      <span className="font-mono text-[12.5px] text-dim">:{d.port}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="grid grid-cols-3 gap-3">
            <TextField
              label="Local host"
              className="col-span-2"
              value={localHost}
              onChange={(e) => setLocalHost(e.target.value)}
            />
            <NumberField label="Local port" value={localPort} onValue={setLocalPort} />
          </div>

          <Nav row={
            <>
              <BackButton onBack={() => setStep(0)} />
              <NextButton onNext={() => setStep(2)} disabled={!localValid} />
            </>
          } />
        </section>
      )}

      {step === 2 && (
        <section className="flex flex-col gap-4">
          <div>
            <h1 className="text-[17px] font-semibold">Tunnel server</h1>
            <p className="mt-1 text-[13px] text-dim">
              A public relay (no secret) or your own Spore/Bore server.
            </p>
          </div>

          <div className="grid grid-cols-3 gap-3">
            <TextField
              label="Server host"
              className="col-span-2"
              placeholder="bore.pub or spore.example.com"
              value={serverHost}
              onChange={(e) => setServerHost(e.target.value)}
              autoFocus
            />
            <NumberField label="Port" value={serverPort} onValue={setServerPort} />
          </div>

          <TextField
            label="Secret (optional)"
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            hint="Public relays need none; private Spore servers use a shared secret."
          />

          <TextField
            label="Profile name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />

          <div className="rounded-card border border-line bg-surface px-3 py-2">
            <Toggle
              checked={connectNow}
              onChange={setConnectNow}
              label="Connect now"
              description="Start the tunnel as soon as the profile is created"
            />
          </div>

          {error && (
            <p role="alert" className="rounded-md border border-danger/40 bg-danger/5 px-3 py-2 text-[12.5px] text-danger">
              {error}
            </p>
          )}

          <Nav row={
            <>
              <BackButton onBack={() => setStep(1)} />
              <button
                type="button"
                onClick={create}
                disabled={busy || !serverValid}
                className="flex items-center gap-1.5 rounded-card bg-accent/15 px-4 py-2 text-[13px] font-semibold text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {connectNow ? <Rocket size={14} /> : <Sprout size={14} />}
                {connectNow ? "Create and connect" : "Create profile"}
              </button>
            </>
          } />
        </section>
      )}
    </div>
  );
}

function Nav({ row }: { row: React.ReactNode }) {
  return <div className="mt-1 flex items-center justify-between border-t border-line pt-4">{row}</div>;
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      type="button"
      onClick={onBack}
      className="flex items-center gap-1.5 rounded-card px-3 py-2 text-[13px] font-medium text-dim transition-colors hover:text-ink"
    >
      <ArrowLeft size={14} /> Back
    </button>
  );
}

function NextButton({ onNext, disabled }: { onNext: () => void; disabled: boolean }) {
  return (
    <button
      type="button"
      onClick={onNext}
      disabled={disabled}
      className="flex items-center gap-1.5 rounded-card bg-accent/15 px-4 py-2 text-[13px] font-semibold text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
    >
      Continue <ArrowRight size={14} />
    </button>
  );
}
