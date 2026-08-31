# Bore Tunnel GUI

A lightweight Windows desktop app that exposes your local server to the internet in one click using the [Bore](https://github.com/ekzhang/bore) tunnel protocol. No CLI, no port forwarding, no dependencies — just a single installer.

Implements the Bore protocol natively in Rust, so there's no external binaries or sidecars to manage. Works with any Bore-compatible server, including the public [bore.pub](https://bore.pub) relay. Great for making Minecraft servers, game hosts, or any local TCP service publicly accessible.

## Requirements

- Windows 10/11
- A running local TCP service (e.g. a Minecraft Java server on port 25565)
- Access to a Bore server — use the public `bore.pub` or self-host your own
- The server's secret/password (if required)

## Quick start

1. Download the latest installer from the [Releases](../../releases) page.
2. Run the installer. No admin privileges needed.
3. Open the app.
4. Enter the Bore server host — e.g. `bore.pub` for the public relay.
5. Enter the secret/password if the server requires one.
6. Click **Save Config**.
7. Start your local server.
8. Click **Start Tunnel**.
9. Copy the generated public address (e.g. `bore.pub:49152`).
10. Share that address with whoever needs to connect.

## How it works

The app implements the Bore control protocol natively in Rust — no bundled `bore.exe` or sidecar binary. When you start a tunnel:

1. Opens a TCP connection to the Bore server's control port (default 7835).
2. Completes HMAC-SHA256 authentication.
3. Requests a public port (random if set to 0).
4. Listens for incoming connections and proxies each one to your local service.

## Configuration

### App settings

Stored in `%APPDATA%\bore-minecraft-tunnel\config.json`:

```json
{
  "bore_server_host": "bore.pub",
  "local_host": "127.0.0.1",
  "local_port": 25565,
  "remote_port": 0
}
```

- `remote_port: 0` lets the server assign a random available port.

### Secret storage

The Bore secret/password is stored in **Windows Credential Manager** via the OS keyring. It is never saved in plain text or in the config file.

## Development

### Prerequisites

- Node.js 18+
- Rust (via [rustup](https://rustup.rs))
- pnpm (or npm)

### Run in dev mode

```powershell
pnpm install
pnpm tauri dev
```

### Build the installer

```powershell
pnpm tauri build
```

This produces an NSIS installer in `src-tauri/target/release/bundle/nsis/`.

## Troubleshooting

### "Local server not reachable"

Make sure your local server is running on the configured port (default `127.0.0.1:25565`) before starting the tunnel.

### "Invalid secret"

Check that the secret matches what the Bore server expects. Re-enter it in the app and click Save Config.

### "Connection timed out"

Verify the Bore server host is correct and reachable from your network. The default control port is `7835`.

### Windows Firewall

Windows may prompt to allow the app through the firewall. Allow it for the tunnel to work.

### Others cannot connect

- Verify the public address is correct.
- Make sure the assigned port is open on the remote server's firewall.
- Make sure your local server is running and accepting connections.

## Built with

- [Tauri v2](https://v2.tauri.app/) — Rust backend + web frontend
- [Bore protocol](https://github.com/ekzhang/bore) — TCP tunnel protocol
- React + TypeScript + Vite
