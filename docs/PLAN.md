# Spore Tunnel GUI — Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Build `spore-tunnel-gui` — a polished Tauri v2 desktop app (Rust + React/TS) that natively speaks both the **Bore** and **Spore** tunnel protocols, with multi-tunnel management, real-time stats/logs, system tray, autostart/reconnect, and a modern UI — replacing `bore-tunnel-gui` (kept untouched as legacy).

**Architecture:** Fresh repo seeded from the bore-tunnel-gui code. Rust backend grows from a single `bore.rs` into a proper `tunnel/` module (protocol framing, client handshake, data-plane forwarder, multi-tunnel manager with heartbeat + reconnect), pushing state to the UI via Tauri events (no polling). Frontend becomes a sidebar + dashboard layout (Tailwind, dark-first). Windows-first (NSIS), same CI pattern as today.

**Tech Stack:** Tauri v2, Rust (tokio, serde_json, hmac/sha2, keyring, dirs, tracing), React 18 + TypeScript + Vite 6, Tailwind CSS v4, zustand, lucide-react. Test: `cargo test` with an in-process mock bore/spore server; vitest on the frontend.

**Repo:** `github.com/Rastrian/spore-tunnel-gui` (MIT; keep a "based on bore-tunnel-gui" attribution in the README; commits/PRs in English, identity `Rastrian <10719452+Rastrian@users.noreply.github.com>`).

---

## Current context (what exists in bore-tunnel-gui today)

- Tauri v2 app, product "Bore Minecraft Tunnel", identifier `com.bore-minecraft-tunnel`, Windows NSIS only, CI `build.yml` via `tauri-apps/tauri-action` on `windows-latest`.
- `src-tauri/src/bore.rs`: native Bore client — 3s timeout consts, `TunnelStatus { state, remote_address, logs }`, `BoreClient { start/stop/status/is_running }`. Framing + HMAC auth already work against real bore servers.
- `src-tauri/src/commands.rs`: tauri commands `load/save_config`, `save/has_secret`, `start_tunnel`, `stop_tunnel`, `get_status`, `copy_address`, `open_config_folder`. State = single `Arc<Mutex<BoreClient>>` (one tunnel max).
- `src-tauri/src/config.rs`: single config `%APPDATA%\bore-minecraft-tunnel\config.json` (`bore_server_host`, optional `bore_server_port`, `local_host`, `local_port` (25565), `remote_port` (0=random)), secret in OS keyring (service `bore-minecraft-tunnel`, user `bore-secret`).
- Frontend: single `App.tsx` form + 2s `getStatus` polling, raw log div, minimal CSS. No tray, no profiles, no events, no stats.
- Protocol knowledge we can rely on (from the Spore server repo, `lib/spore/client.ex`):
  - Client sends legacy `{"Hello": port}` first; a Rust bore server **drops the connection** on unknown variants like `HelloEx`.
  - Spore server answers `{"Hello": port}` (legacy mode) or `{"HelloEx": {"port": N}}`; extended retry message is `{"HelloEx": {"port": N, "version": "spore/1", "features": [...]}}`.
  - Spore sends periodic heartbeat ACKs on the control connection; a client that ignores ACK death leaves **ghost ports** on the server (this burned us on the fleet). The GUI must treat missing ACKs within a window as death → full teardown → retry with backoff.

## Assumptions

- Windows 10/11 is the release target (v0.1.0). macOS/Linux builds = backlog (unsigned macOS is a poor first impression).
- Bore/Spore servers used in tests: in-process mock (tokio) reproducing both message dialects; no live-server dependency in CI.
- TLS data plane (`features: ["tls"]`) is **out of scope** for v0.1 — noted as backlog until the server-side story is settled.
- New app config dir: `%APPDATA%\spore-tunnel-gui\` ; app identifier: `dev.rastrian.sporetunnel` ; product name: **Spore Tunnel**.

---

## Proposed approach

1. **Seed** the new repo from the old one (history clean, single bootstrap commit + attribution), rename everything, get CI green on day 1.
2. **Harden the protocol core first** (pure Rust, fully unit-tested with a mock server): framing extraction, Hello/HelloEx negotiation, auth, heartbeat supervision, reconnect with backoff, per-connection byte counting. This is the part that must not rot.
3. **Multi-tunnel manager** behind Tauri commands + emitted events; config becomes profiles (JSON file + keyring per profile).
4. **UI rebuild** on the new event stream: shell (sidebar + dashboard), onboarding wizard, logs, settings, tray.
5. **Release**: icon, installer metadata, updater-check, README, tag `v0.1.0`.

Phases 1–2 are pure backend (testable headless). Phase 3 is the contract freeze (event + config schemas). Phase 4 is pure frontend against the frozen contract. That keeps the risky bits in small, verifiable tasks.

---

## Phase 0 — Bootstrap repo (green CI on day 1)

### Task 0.1: Create repo and seed from bore-tunnel-gui
- `gh repo create Rastrian/spore-tunnel-gui --clone` (visibility: see Open Questions; default private until v0.1.0).
- Copy all files from bore-tunnel-gui (clone both, `rsync` old over new, drop `.git`).
- Attribution: top of README gets `> Forked from [bore-tunnel-gui](https://github.com/Rastrian/bore-tunnel-gui) (MIT).`

### Task 0.2: Rename everything
- `package.json`: name `spore-tunnel-gui`.
- `src-tauri/tauri.conf.json`: productName `Spore Tunnel`, identifier `dev.rastrian.sporetunnel`, window title `Spore Tunnel`, size 980×680 (new layout needs more room), minWidth/minHeight 860×600.
- `src-tauri/Cargo.toml`: package name `spore-tunnel-gui`.
- Global rename `bore-minecraft-tunnel` → `spore-tunnel-gui` in `config.rs` constants (config dir + keyring service).
- Verify: `npm install && npm run build` (frontend compiles); `cargo check` in `src-tauri` passes.

### Task 0.3: CI pipeline rename + first commit
- `build.yml` name → "Build Spore Tunnel"; add `permissions: contents: write` (already there) and keep `tauri-action` as-is.
- Commit (English): `chore: seed spore-tunnel-gui from bore-tunnel-gui`.
- Verify: push to `main`, CI green, artifact NSIS builds.

---

## Phase 1 — Protocol core (backend, TDD with mock server)

### Task 1.1: Extract wire framing into `src-tauri/src/tunnel/protocol.rs`
**Files:** Create `src-tauri/src/tunnel/mod.rs`, `src-tauri/src/tunnel/protocol.rs`; slim `bore.rs` into a re-export shim (deleted at end of Phase 1).

**Step 1: failing tests** (`src-tauri/src/tunnel/protocol.rs` `#[cfg(test)]`):
```rust
#[test] fn frame_roundtrip() {
    let msg = serde_json::json!({"Hello": 1234});
    let bytes = encode_frame(&msg);
    // length prefix (u32 LE) + serde_json body
    let (msg2, rest) = decode_frame(&bytes).unwrap();
    assert_eq!(msg, msg2); assert!(rest.is_empty());
}
#[test] fn frame_partial_then_rest() { /* split buffer mid-prefix and mid-body; decoder buffers and returns next frame + remainder */ }
#[test] fn frame_too_large_rejected() { /* cap (e.g. 1 MiB) → ProtocolError::FrameTooLarge */ }
```
**Step 2:** `cargo test` → compile error (missing fns). **Step 3:** implement `encode_frame`/`decode_frame` (move byte logic out of `bore.rs` verbatim; keep u32-LE prefix). **Step 4:** `cargo test -p spore-tunnel-gui` green. **Step 5:** commit `refactor(tunnel): extract length-delimited JSON framing`.

### Task 1.2: Message dialects — `ClientMessage` / `ServerMessage`
**Files:** `src-tauri/src/tunnel/protocol.rs`.

```rust
pub enum ClientMessage { Hello(u16), HelloEx { port: u16, version: String, features: Vec<String> } }
pub enum ServerMessage { Hello(u16), HelloEx { port: u16, features: Vec<String> }, Challenge(Vec<u8>), Error(String), Ack }
// serde as untagged JSON exactly like the wire: {"Hello": p} | {"HelloEx": {...}} | "Challenge" | {"Challenge": [..]} | {"Error": msg} | "Ack"
```
- Tests: roundtrip each variant; **deny unknown fields on `HelloEx` parsing is OFF** (forward-compat); legacy `Hello` response parses when server is plain bore.
- Commit: `feat(tunnel): bore/spore message dialects`.

### Task 1.3: Mock tunnel server for tests — `src-tauri/src/tunnel/mock_server.rs`
- Tokio task binding a random localhost port; configurable behavior:
  ```rust
  pub struct MockServer { pub addr: SocketAddr, /* builder-style flags: */
      pub dialect: Dialect,            // Bore | Spore
      pub require_auth: bool,
      pub ack_interval: Option<Duration>,  // None = never ack (ghost-port scenario)
      pub drop_on_hello_ex: bool,          // true reproduces Rust bore server
      pub assigned_port: u16,
  }
  ```
  - On `Hello`: if `require_auth` → send `Challenge(nonce)`, verify HMAC-SHA256(secret, nonce) reply, then `Hello(assigned_port)`; if `drop_on_hello_ex` and it ever receives `HelloEx` → close socket (this is exactly the Rust bore behavior we must respect).
  - Spore dialect: also answer `HelloEx` when probed, send `Ack` frames every `ack_interval`, and close idle data connections cleanly.
  - Data plane: accept TCP on `assigned_port`, echo/swap bytes so forwarder tests can assert payloads.
- Test: client `hello()` against Bore mock → `ServerMessage::Hello(port)`; against Spore mock → detects `HelloEx`.
- Commit: `test(tunnel): in-process bore/spore mock server`.

### Task 1.4: Handshake + auth in `src-tauri/src/tunnel/client.rs`
- Move/rewrite `BoreClient::start` handshake into `TunnelClient::connect(cfg, secret) -> (AssignedPort, ServerInfo)`:
  1. TCP connect (timeout 3s, same as today).
  2. Send `Hello(remote_port)`. If server closes / undecodable → **retry once with `HelloEx`** only if `cfg.allow_spore` (mirrors Spore's own client: legacy-first, because Rust bore servers drop on unknown variants). Record `ServerInfo { kind: Bore|Spore, features: Vec<String> }`.
  3. If `Challenge` → HMAC-SHA256 with secret → reply → expect `Hello`/`HelloEx`.
  4. `Error(msg)` → typed error `TunnelError::ServerRejected(msg)` (UI shows it verbatim, e.g. invalid secret / quota / port taken).
- Tests (mock server): legacy path, spore path, auth path, wrong secret → `ServerRejected`, drop-on-HelloEx fallback works, secret-less server with non-empty secret still connects (server ignores it).
- Commit: `feat(tunnel): handshake with HelloEx probing and HMAC auth`.

### Task 1.5: Data plane forwarder + byte counting — `src-tauri/src/tunnel/forward.rs`
- One tokio task per inbound connection: open TCP to `(local_host, local_port)`, `tokio::io::copy_bidirectional`, count bytes both ways into `Arc<AtomicU64>` pairs.
- Local preflight: before control connect, `TcpStream::connect(local)` → if refused, `TunnelError::LocalServiceDown(local_addr)` (replaces whatever string check exists today).
- Tests: mock server + local echo server → client receives echoed payload; counters match bytes sent; local down → `LocalServiceDown`.
- Commit: `feat(tunnel): data-plane forwarder with byte accounting`.

### Task 1.6: Heartbeat supervision + reconnect — `src-tauri/src/tunnel/supervisor.rs`
This is the ghost-port killer. Lessons from the Spore fleet are encoded here:
```rust
pub struct Supervisor { /* owns control conn + forwarder JoinHandles */ }
impl Supervisor {
    // Control loop:
    // - Spore server (HelloEx seen): expect Ack frames; if none within ACK_TIMEOUT (10s)
    //   since last traffic, OR control read returns EOF/half-close probe fails → declare dead.
    // - Bore server (no Acks in protocol): rely on TCP health + optional half-close probe.
    // On death: close control socket AND all forwarders (do not leak), emit TunnelEvent::Died,
    // then if cfg.auto_reconnect: retry connect with backoff 5s→10s→…→60s cap, jitter ±20%,
    // forever while enabled. Assigned port may change after reconnect → re-emit address.
}
```
- Tests (with mock): server stops acking → client tears down ≤ ACK_TIMEOUT + grace and enters backoff; server returns → reconnects and reports new port; manual `stop()` cancels backoff and frees everything (assert all sockets closed via mock's accept-count going quiet).
- Commit: `feat(tunnel): heartbeat supervision and auto-reconnect`.

### Task 1.7: Delete old `bore.rs` plumbing, keep public types
- `TunnelStatus` evolves to:
  ```rust
  pub struct TunnelStatus { state: TunnelState, server: ServerKind, remote: Option<SocketAddr>,
                            local: SocketAddr, uptime_secs: u64, bytes_up: u64, bytes_down: u64,
                            reconnects: u32, last_error: Option<String> }
  ```
- `cargo test` full suite green; `cargo check` no warnings. Commit: `refactor(tunnel): retire legacy bore.rs module`.

---

## Phase 2 — Multi-tunnel manager, config profiles, Tauri commands + events

### Task 2.1: Profiles config — `src-tauri/src/config.rs` rewrite
```rust
pub struct Profile { id: Uuid, name: String, server_host: String, server_port: u16 /*7835*/,
                     local_host: String, local_port: u16, remote_port: u16 /*0=random*/,
                     autostart: bool, auto_reconnect: bool }
pub struct AppConfig { profiles: Vec<Profile>, active_profile_id: Option<Uuid>,
                       ui: UiPrefs { theme: Dark|Light|System, start_minimized: bool } }
```
- Stored at `%APPDATA%\spore-tunnel-gui\config.json`; secrets stay in keyring: service `spore-tunnel-gui`, user `profile:<id>`.
- **Migration:** if `%APPDATA%\bore-minecraft-tunnel\config.json` exists, offer `import_legacy_cmd()` (never automatic on first run without consent): creates profile `Imported · <host>`, copies keyring secret from old service `bore-minecraft-tunnel`/`bore-secret`.
- Tests: roundtrip, defaults, legacy import (tempdir + fake keyring behind trait `SecretStore` so tests don't touch the real OS store).
- Commit: `feat(config): multi-profile config with legacy import`.

### Task 2.2: `TunnelManager` — `src-tauri/src/tunnel/manager.rs`
```rust
pub struct TunnelManager { tunnels: HashMap<Uuid, RunningTunnel> } // RunningTunnel = Supervisor + stats handles + JoinHandle
// commands (all emit events on change):
// start_tunnel(profile_id) / stop_tunnel(profile_id) / get_all_status() / get_tunnel_log(profile_id)
```
- Replaces the single `Arc<Mutex<BoreClient>>` state. Ring buffer (1024 lines) of tracing log lines per tunnel for the log view.
- Commit: `feat(tunnel): multi-tunnel manager`.

### Task 2.3: Event contract (freeze this)
Events emitted Rust → frontend (`app.emit(...)`), payload serde-camelCase:
| Event | Payload | When |
|---|---|---|
| `tunnel://status` | `{ profileId, status: TunnelStatus }` | any state change (≤ 1/s coalesced for stat ticks) |
| `tunnel://log` | `{ profileId, line, level, ts }` | log line |
| `tunnel://stats` | `{ profileId, bytesUp, bytesDown, uptimeSecs }` | every 1s while running |
Frontend never polls. `invoke` commands: `list_profiles`, `save_profile`, `delete_profile`, `import_legacy`, `start_tunnel`, `stop_tunnel`, `get_all_status`, `get_tunnel_log`, `detect_local_service`, `check_for_updates`, `open_config_folder`.
- Tests: integration test in `src-tauri/tests/events.rs` spins manager against mock server, asserts event ordering (Starting → Connected(port) → Died → Connected(new port) after mock restart).
- Commit: `feat(app): event contract + tauri command surface`.

### Task 2.4: Port auto-detect helper
- `detect_local_service` scans `127.0.0.1` on [25565, 25575, 8123, 3000, 8000, 8080, 5000, 27015, 7777] (TCP connect, 150ms timeout each, parallel) → returns hits with well-known names (Minecraft, Minecraft RCON, Dynmap, generic web, Terraria-ish…). Pure helper + unit tests. Used by the wizard (Phase 4).
- Commit: `feat(app): local service detection helper`.

---

## Phase 3 — UI overhaul (frontend)

### Task 3.1: Tailwind v4 + design tokens
- `npm i tailwindcss @tailwindcss/vite lucide-react zustand` ; vite plugin in `vite.config.ts`; single `@import "tailwindcss"` in `src/styles.css` (drop old CSS).
- Tokens (dark-first): bg `#0b0f14`, panel `#121821`, border `#1f2937`, text `#e5edf5`, muted `#8b98a9`; accent **spore-green** `#4ade80` (connected), amber `#fbbf24` (starting), red `#f87171` (failed/error); radius `12px`; font Inter (bundled, no CDN — offline-friendly).
- Light theme via `data-theme` attr + CSS vars. Commit: `feat(ui): tailwind v4 setup and design tokens`.

### Task 3.2: App shell
- Layout: left sidebar (260px): app logo "Spore Tunnel", tunnel list (name, status dot, address when live, `+` button), bottom row: settings gear + theme toggle. Main area: active tunnel dashboard or empty state.
- `src/store.ts`: zustand store fed exclusively by `listen("tunnel://...")` events + initial `get_all_status()`.
- `src/lib/api.ts`: fully typed invoke wrappers matching the Task 2.3 contract; `src/lib/types.ts` mirrors Rust types (manual, keep in sync checklist in PR template).
- Commit: `feat(ui): app shell, sidebar, zustand event store`.

### Task 3.3: Tunnel dashboard (the hero screen)
- Status hero: animated state ring (idle/starting/connected/failed), giant monospace public address chip `host:port` with copy button (flash "Copied"), server badge `SPORE`/`BORE` (from ServerInfo), uptime, reconnect count, throughput (▲ up / ▼ down, humanized, 1s tick from `tunnel://stats`).
- Buttons: Start / Stop (primary), and when failed: inline error card + "Retry now".
- Sparkline of throughput last 60s (tiny hand-rolled SVG, no chart lib — keep bundle small).
- Commit: `feat(ui): tunnel dashboard with live stats`.

### Task 3.4: Wizard + profile editor
- First run (no profiles): 3-step wizard: (1) what are you hosting? [Minecraft Java (25565) | Other game | Web app | Custom] → (2) detect local service (uses `detect_local_service`, manual override) → (3) pick server (free-text host + optional secret; help text about `bore.pub`-style public relays and self-hosted Spore) → creates profile + optional "Connect now".
- Profile editor modal reuses the same fields (name, host, ports incl. "Auto (random)" toggle for remote port, autostart, auto-reconnect).
- Commit: `feat(ui): onboarding wizard and profile editor`.

### Task 3.5: Logs view
- Per-tunnel log console (reads `tunnel://log` + `get_tunnel_log` backfill): monospace, level colors, filter box, pause-on-scroll, "Copy all", auto-scroll toggle. 1k-line cap client-side.
- Commit: `feat(ui): per-tunnel log console`.

### Task 3.6: Settings screen
- Theme (dark/light/system), start minimized, close-to-tray, "Check for updates" (button + last-checked), config folder link, legacy import card (shown only when old config detected), About (version from `app.getVersion()`).
- Commit: `feat(ui): settings screen`.

### Task 3.7: System tray
- Tauri v2 tray icon (capabilities: `tray-icon` feature) with 3 state variants (idle gray / running green / error red): menu = Open, per-tunnel Start/Stop, Quit. Close-to-tray by default (setting).
- Manual test checklist in PR description (tray can't be covered by CI): start from tray, stop from tray, tooltip shows address.
- Commit: `feat(ui): system tray with per-tunnel controls`.

---

## Phase 4 — Release engineering

### Task 4.1: Icons + branding
- Generate icon set (mushroom/spore motif, green-on-dark) into `src-tauri/icons/` via `tauri icon` from a single 1024px source. Commit: `chore: app icons`.

### Task 4.2: Update checker
- `check_for_updates`: GET `https://api.github.com/repos/Rastrian/spore-tunnel-gui/releases/latest`, semver-compare vs `app.getVersion()`, banner in UI + link (no auto-install — YAGNI). Unit test with mocked JSON. Commit: `feat(app): update checker`.

### Task 4.3: Autostart
- `tauri-plugin-autostart`; setting "Start Spore Tunnel at login" + profiles with `autostart: true` auto-connect on launch (after `start_minimized`). Commit: `feat(app): autostart and launch auto-connect`.

### Task 4.4: README + docs
- New README (English): quick start, screenshots (dashboard + wizard), how it works (native protocol, no binaries), Spore vs Bore server support matrix, config/secrets storage, dev setup, troubleshooting (carried over + new: ghost-port explanation, reconnect semantics). Attribution line to bore-tunnel-gui. Commit: `docs: rewrite README for Spore Tunnel`.

### Task 4.5: Tag v0.1.0
- Bump `tauri.conf.json` + `package.json` to `0.1.0`, tag `v0.1.0`, CI publishes release with NSIS installer. Verify: download installer on a Windows box, wizard → connect to a real Spore server (one of the lab-managed ones with a test secret) → friend connects on the exposed port. That live e2e is the release gate.

---

## Files likely to change (new repo)

```
src-tauri/src/main.rs            (setup: tray, autostart, state)
src-tauri/src/commands.rs        (rewritten: profile + manager commands)
src-tauri/src/config.rs          (rewritten: profiles, migration, SecretStore trait)
src-tauri/src/tunnel/mod.rs      (new)
src-tauri/src/tunnel/protocol.rs (new: framing + dialects + tests)
src-tauri/src/tunnel/client.rs   (new: handshake/auth)
src-tauri/src/tunnel/forward.rs  (new: data plane + counters)
src-tauri/src/tunnel/supervisor.rs (new: heartbeat/reconnect)
src-tauri/src/tunnel/manager.rs  (new: multi-tunnel + events)
src-tauri/src/tunnel/mock_server.rs (test-only)
src-tauri/src/bore.rs            (deleted end of Phase 1)
src/App.tsx → src/{main.tsx, store.ts, components/*, views/*, lib/{api,types}.ts}
src/styles.css                   (tailwind tokens)
.github/workflows/build.yml      (rename + matrix hook for future OSes)
tauri.conf.json, Cargo.toml, package.json, README.md, icons/
```

## Tests / validation

- Backend: `cargo test` (unit + mock-server integration) — target: every protocol edge above has a named test; `cargo clippy -- -D warnings` clean.
- Frontend: `npm run build` (tsc strict) + `vitest` for store reducer + api mapping.
- CI: same `tauri-action` pattern, artifact per push; release on tag.
- Manual gates per phase: P1/P2 run headless; P3 needs `npm run tauri dev` walkthrough (wizard→connect→copy address→connect from another host on LAN→stop→tray quit); P4 the Windows installer live e2e.

## Risks & tradeoffs

1. **Protocol dialect risk (highest):** Rust bore servers drop the connection on `HelloEx`. Mitigation: legacy-first probing exactly like the Spore Elixir client; `drop_on_hello_ex` mock test proves the fallback. No GUI ever sends `HelloEx` to a server that hasn't failed a legacy hello.
2. **Ghost ports:** client crash leaving server-side listeners. Mitigation: aggressive ACK supervision + full teardown on death; document that assigned ports can change after reconnect (UI already re-renders address from events).
3. **Keyring on Windows:** Credential Manager occasionally fails in CI/test → `SecretStore` trait, tests use in-memory store; production path retries once then degrades to "secret not remembered".
4. **Event flood:** stats at 1s per tunnel with many tunnels → coalesce status+stats into one event per tick per tunnel.
5. **Scope creep:** TLS data plane, macOS/Linux, auto-update install — all explicitly backlog; v0.1 ships Windows-only with the features above.
6. **Tray/custom titlebar polish** is the most likely place to burn time — tray is in-scope (simple menu), custom titlebar is not (keep native decorations).

## Open questions (defaults chosen; flip before starting if wanted)

1. Repo visibility — default: **private until v0.1.0**, then public (matches Spore's pattern).
2. macOS/Linux CI targets — default: **backlog** (unsigned macOS dmg confuses users; revisit v0.2).
3. Name shown in UI — default: **Spore Tunnel** (window title "Spore Tunnel", tray tooltip "Spore Tunnel — N running").
4. TLS when Spore advertises `features:["tls"]` — default: **show badge only**, don't implement (until server-side story settles).

## Execution handoff

Recommended: implement via subagent-driven-development, one fresh subagent per task, two-stage review (spec compliance → code quality), phases as review milestones. Phases 0–2 can run fully autonomously; Phase 3 needs a `tauri dev` human pass at the end; Phase 4 needs Rastrian's Windows machine for the live e2e gate.
