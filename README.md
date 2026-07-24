# CoLink Desktop

Desktop client for CoLink — clipboard sync, file transfer, text messaging, now-playing display, and CastBoard.

**Tech stack:** Tauri 2 · Rust · React 19 · TypeScript · Vite · Tailwind CSS · SQLite (rusqlite) · i18next

## Requirements

- [Rust toolchain](https://rustup.rs/)
- Node.js 20+ and [pnpm](https://pnpm.io/)
- Tauri prerequisites for your platform: https://tauri.app/start/prerequisites/

## Development

```sh
pnpm install
pnpm tauri dev
```

Vite serves the frontend on port 1420; Tauri connects to it automatically.

## Build and release

```sh
# Unpackaged debug binary
pnpm tauri:debug-build

# Production installers
pnpm tauri build
```

- Windows: NSIS installer (`.exe`). When updater signing is configured, Tauri also creates a signed update archive (`.nsis.zip`) and its signature (`.nsis.zip.sig`).
- Ubuntu and Debian: Debian package (`.deb`), installed through the system package manager.

### Windows update signing

The updater public key is committed in `src-tauri/tauri.conf.json`. Its corresponding private key must remain outside the repository and must never be replaced after a release has been distributed.

For a local signed Windows build, set the private key and password only for the current PowerShell session:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw "path\to\colink-desktop-updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Read-Host 'Tauri signing key password'
pnpm tauri build --bundles nsis
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

Do not commit the private key or its password. The local build outputs are under `src-tauri/target/release/bundle/nsis/`.

## Architecture

| Layer | Tech | Responsibilities |
|---|---|---|
| Frontend | React + Vite | UI, routing (hash router), server API calls |
| Backend | Rust (Tauri) | LAN networking, crypto, SQLite storage, clipboard, system tray, IPC |

- **LAN discovery**: mdns-sd
- **LAN crypto**: ed25519-dalek (identity), x25519-dalek + hkdf + sha2 (session key), aes-gcm / chacha20poly1305
- **Music sync** (Now Playing / CastBoard): NetEase Cloud Music, QQ Music, Spotify
- **Local storage**: SQLite (device trust store, settings, logs)
- **Credentials**: system keyring
