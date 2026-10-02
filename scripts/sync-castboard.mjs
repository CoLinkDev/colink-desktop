import { cp, mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import extract from 'extract-zip'

const root = resolve(fileURLToPath(new URL('..', import.meta.url)))
const target = resolve(root, 'public', 'castboard')
const cacheRoot = resolve(root, 'node_modules', '.cache', 'colink', 'castboard')
const isTauriDebugBuild = /^(1|true|yes)$/i.test(process.env.TAURI_ENV_DEBUG ?? '')

async function isFile(path) {
  try {
    return (await stat(path)).isFile()
  } catch {
    return false
  }
}

async function resolveLocalDist(configuredPath) {
  const localRoot = resolve(root, configuredPath)
  for (const candidate of [localRoot, resolve(localRoot, 'dist')]) {
    if (await isFile(resolve(candidate, 'index.html'))) return candidate
  }
  throw new Error(`COLINK_CASTBOARD_LOCAL_PATH must point to a CastBoard dist directory or a project with dist/index.html: ${localRoot}`)
}

async function restoreRelease(version) {
  const cacheDirectory = resolve(cacheRoot, version)
  const cachedIndex = resolve(cacheDirectory, 'dist', 'index.html')
  if (await isFile(cachedIndex)) return resolve(cacheDirectory, 'dist')

  await rm(cacheDirectory, { recursive: true, force: true })
  await mkdir(cacheRoot, { recursive: true })
  const temporaryRoot = await mkdtemp(resolve(cacheRoot, `.tmp-${version}-`))
  const archivePath = resolve(temporaryRoot, 'castboard-dist.zip')
  const extractedPath = resolve(temporaryRoot, 'extracted')
  const releaseUrl = `https://github.com/CoLinkDev/colink-castboard/releases/download/v${version}/castboard-dist.zip`

  try {
    const response = await fetch(releaseUrl)
    if (!response.ok) {
      throw new Error(`CastBoard ${version} download failed: ${response.status} ${response.statusText}`)
    }
    await writeFile(archivePath, Buffer.from(await response.arrayBuffer()))
    await extract(archivePath, { dir: extractedPath })
    if (!(await isFile(resolve(extractedPath, 'dist', 'index.html')))) {
      throw new Error(`CastBoard ${version} release does not contain dist/index.html`)
    }
    await rename(extractedPath, cacheDirectory)
    return resolve(cacheDirectory, 'dist')
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true })
  }
}

if (isTauriDebugBuild) {
  await rm(target, { recursive: true, force: true })
  await mkdir(target, { recursive: true })
  await writeFile(resolve(target, '.gitkeep'), '')
  process.exit(0)
}

const packageMetadata = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'))
const version = packageMetadata.castboardVersion
if (!/^\d+\.\d+\.\d+$/.test(version ?? '')) {
  throw new Error(`package.json castboardVersion must be a semantic version: ${String(version)}`)
}

const localPath = process.env.COLINK_CASTBOARD_LOCAL_PATH?.trim()
const source = localPath
  ? await resolveLocalDist(localPath)
  : await restoreRelease(version)

await rm(target, { recursive: true, force: true })
await cp(source, target, { recursive: true })
