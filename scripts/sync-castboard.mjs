import { cp, rm, stat } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(fileURLToPath(new URL('..', import.meta.url)))
const source = resolve(root, '..', 'colink-castboard', 'src')
const target = resolve(root, 'public', 'castboard')

await rm(target, { recursive: true, force: true })

try {
  if ((await stat(source)).isDirectory()) {
    await cp(source, target, { recursive: true })
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error
}
