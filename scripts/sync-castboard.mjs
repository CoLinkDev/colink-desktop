import { spawn } from 'node:child_process'
import { cp, mkdir, rm, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(fileURLToPath(new URL('..', import.meta.url)))
const castboard = resolve(root, 'castboard')
const source = resolve(castboard, 'dist')
const target = resolve(root, 'public', 'castboard')
const isTauriDebugBuild = /^(1|true|yes)$/i.test(process.env.TAURI_ENV_DEBUG ?? '')

await rm(target, { recursive: true, force: true })

if (isTauriDebugBuild) {
  await mkdir(target, { recursive: true })
  await writeFile(resolve(target, '.gitkeep'), '')
  process.exit(0)
}

const pnpm = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'
await new Promise((resolveBuild, rejectBuild) => {
  const command = process.platform === 'win32' ? 'cmd.exe' : pnpm
  const args = process.platform === 'win32' ? ['/d', '/s', '/c', 'pnpm build'] : ['build']
  const child = spawn(command, args, {
    cwd: castboard,
    stdio: 'inherit',
  })
  child.once('error', rejectBuild)
  child.once('exit', (code, signal) => {
    if (code === 0) {
      resolveBuild()
      return
    }
    rejectBuild(new Error(`CastBoard build failed with ${signal ?? `exit code ${code}`}`))
  })
})

await cp(source, target, { recursive: true })
