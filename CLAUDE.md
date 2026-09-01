# Project: Spore Tunnel GUI

Polished Tauri v2 desktop app (Windows-first) that exposes local TCP services through
**Bore** and **Spore** tunnel servers, natively (no external binaries). Based on
`bore-tunnel-gui` (same author). Full implementation plan: `docs/PLAN.md` (if present).

## Author identity (REQUIRED for all commits)

```
git config user.name "Rastrian"
git config user.email "10719452+Rastrian@users.noreply.github.com"
```

Commit messages and PR titles/descriptions: **English only**. Conventional commits
(`feat:`, `fix:`, `refactor:`, `test:`, `chore:`, `docs:`).

## Commands

```bash
npm install            # frontend deps (run once per checkout/worktree)
npm run build          # tsc + vite build -> dist/
cargo check            # inside src-tauri/
cargo test             # inside src-tauri/ (unit + mock-server integration)
cargo clippy -- -D warnings
```

**IMPORTANT:** `cargo check/test` requires `dist/` to exist (Tauri's `generate_context!`
reads it at compile time). If missing: `mkdir -p dist` is enough for check/test;
run `npm run build` for real UI work.

`CARGO_TARGET_DIR` may point to a shared dir to reuse compiled deps across worktrees.

## Architecture

- `src-tauri/src/tunnel/` — protocol stack. **Wire framing: JSON messages delimited
  by a single NUL byte (0x00), max frame 256 bytes** — matches ekzhang/bore
  (`AnyDelimiterCodec`, delims `[0]`, `MAX_FRAME_LENGTH = 256`) and Spore's
  `lib/spore/shared.ex`. NEVER use length-prefix framing (u32 LE or BE) — verified
  wrong against a real bore server on 2026-09-01 (server logs `unable to parse
  message`; the original bore-tunnel-gui `bore.rs` was NUL-delimited and correct).
- `src-tauri/src/commands.rs` — Tauri commands; state currently single `Arc<Mutex<BoreClient>>`.
- `src-tauri/src/config.rs` — JSON config + OS keyring secret (service `bore-minecraft-tunnel`,
  being renamed to `spore-tunnel-gui`).
- `src/` — React 18 + TypeScript + Vite frontend. Polling `get_status` every 2s today;
  moving to Tauri events (`tunnel://status`, `tunnel://log`, `tunnel://stats`).

## Protocol facts (verified against Spore server source — do not guess)

1. **Legacy-first handshake.** Rust bore servers decode `ClientMessage` as a closed
   serde enum and **drop the connection** on unknown variants like `HelloEx`. A client
   must send `{"Hello": port}` first and only retry with
   `{"HelloEx":{"port":N,"version":"spore/1","features":[...]}}` if the legacy frame
   failed (connection dropped / undecodable reply).
2. Spore server answers `{"Hello": port}` (legacy mode) or `{"HelloEx": {"port": N}}`.
3. `Challenge` (string) = server requires HMAC-SHA256(secret, nonce) auth.
4. Spore sends periodic `Ack` frames on the control connection. A client that ignores
   ACK death leaves **ghost ports** on the server. Supervisor must treat missing ACKs
   within a window as death -> full teardown (control + all forwarders) -> retry with
   backoff (5s -> 10s -> ... -> 60s cap, +-20% jitter).
5. Plain bore servers never send ACKs — supervision there relies on TCP health only.

## Conventions

- Windows 10/11 is the release target; NSIS installer, `installMode: currentUser`.
- Keep code modular: protocol layer must stay UI-agnostic and fully unit-tested
  (mock server in `src-tauri/src/tunnel/mock_server.rs`, tokio-based, test-only).
- Secrets never in plain config files — OS keyring only.
- Tests must not require network or real tunnel servers — mock server only.
- Every task: tests written first or alongside, `cargo test` green, clippy clean,
  then commit. Never leave the repo in a non-compiling state at commit time.
