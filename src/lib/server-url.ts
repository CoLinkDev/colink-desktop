const supportedServerProtocols = new Set(['http:', 'https:'])

export function normalizeServerUrl(value: string): string | null {
  const normalized = value.trim().replace(/\/+$/, '')
  if (!normalized) {
    return null
  }

  try {
    const url = new URL(normalized)
    if (!supportedServerProtocols.has(url.protocol) || !url.hostname) {
      return null
    }
    return normalized
  } catch {
    return null
  }
}

export function isValidServerUrl(value: string): boolean {
  return normalizeServerUrl(value) !== null
}
