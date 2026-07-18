import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

import { resolveLanguage } from '../i18n'

export function formatLastSeen(value: string | null, fallback = 'Never connected', language?: string) {
  if (!value) {
    return fallback
  }

  const timestamp = Date.parse(value)

  if (Number.isNaN(timestamp)) {
    return value
  }

  return new Intl.DateTimeFormat(resolveLanguage(language), {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(timestamp)
}

export function formatTimestamp(value: number, language?: string) {
  return new Intl.DateTimeFormat(resolveLanguage(language), {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(value)
}

export function formatBytes(value: number | null | undefined) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    return '—'
  }

  if (value < 1024) {
    return `${value} B`
  }

  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KB`
  }

  if (value < 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MB`
  }

  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

export function formatPlatformName(platform: string, t?: (key: string) => string) {
  const normalized = platform.toLowerCase()
  if (normalized === 'unknown') {
    return t ? t('devices.platforms.unknown') : 'Unknown'
  }

  const mapping: Record<string, string> = {
    windows: 'Windows',
    macos: 'macOS',
    linux: 'Linux',
    android: 'Android',
    ios: 'iOS',
  }
  return mapping[normalized] ?? platform
}
