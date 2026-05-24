import { getVersion } from '@tauri-apps/api/app'

export const projectUrl = __APP_PROJECT_URL__
export const buildTime = __APP_BUILD_TIME__
export const fallbackVersion = __APP_FALLBACK_VERSION__

export async function readAppVersion() {
  try {
    return await getVersion()
  } catch {
    return fallbackVersion
  }
}

export function formatBuildTime(value: string) {
  const date = new Date(value)

  if (Number.isNaN(date.getTime())) {
    return value
  }

  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }).format(date)
}
