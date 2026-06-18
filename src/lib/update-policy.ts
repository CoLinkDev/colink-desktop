import { fallbackVersion } from './app-meta'

export function isBreakingVersionUpdate(latestVersion: string, currentVersion = fallbackVersion) {
  const currentMajor = semanticMajor(currentVersion)
  const latestMajor = semanticMajor(latestVersion)
  return currentMajor !== null && latestMajor !== null && latestMajor > currentMajor
}

function semanticMajor(version: string) {
  const normalized = version.trim().replace(/^v/i, '')
  const major = normalized.split('.')[0]
  const parsed = Number.parseInt(major, 10)
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null
}
