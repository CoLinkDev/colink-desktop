import { cp, rm } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(fileURLToPath(new URL('..', import.meta.url)))
const source = resolve(root, '..', 'colink-castboard', 'src')
const target = resolve(root, 'public', 'castboard')

await rm(target, { recursive: true, force: true })
await cp(source, target, { recursive: true })
