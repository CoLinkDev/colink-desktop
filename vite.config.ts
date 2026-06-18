import { readFileSync } from 'node:fs'

import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

const packageJson = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as { version?: string }
const tauriDebugEnv = process.env.TAURI_ENV_DEBUG
const isTauriDebugBuild = tauriDebugEnv != null && /^(1|true|yes)$/i.test(tauriDebugEnv)
const isReleaseBuild = tauriDebugEnv == null ? process.env.NODE_ENV === 'production' : !isTauriDebugBuild

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_'],
  define: {
    __APP_BUILD_TIME__: JSON.stringify(new Date().toISOString()),
    __APP_PROJECT_URL__: JSON.stringify('https://github.com/CoLinkDev/colink-desktop'),
    __APP_FALLBACK_VERSION__: JSON.stringify(packageJson.version ?? '0.0.0'),
    __APP_IS_RELEASE__: JSON.stringify(isReleaseBuild),
  },
  server: {
    port: 1420,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST ?? '127.0.0.1',
  },
})
