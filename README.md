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

## Build

```sh
# Unpackaged debug binary
pnpm tauri:debug-build

# Production installer (NSIS on Windows)
pnpm tauri build
```

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
