# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| latest `main` / newest release | yes |
| anything older | no — upgrade |

Pre-1.0 project: only the newest build is supported, and security fixes land
on `main` first.

## Scope

Spore Tunnel is a local desktop client: it opens outbound TCP connections to
tunnel servers you configure and accepts proxied connections from them. Not in
scope (by design): the tunnel servers themselves, and what you choose to expose
through a tunnel — that is your service's security posture.

Secrets are stored in the OS keyring (Windows Credential Manager) and never in
config files or logs.

## Reporting a vulnerability

Please do **not** open a public issue for anything security-sensitive. Use
GitHub's private vulnerability reporting on this repository
(*Security → Report a vulnerability*), which notifies the maintainer privately.
Include reproduction steps and the app version (Settings → About). You will
hear back within a few days; a fix (or a workaround advisory) follows in the
next release.
