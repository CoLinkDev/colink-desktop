import type { LucideIcon } from 'lucide-react'
import { Computer, Laptop, Monitor, Smartphone, Tablet, Wifi, WifiOff, Key, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import type { DeviceInfo, DevicePlatform } from '../lib/types'
import { formatLastSeen, formatPlatformName } from '../lib/utils'

const iconByType: Record<DevicePlatform, LucideIcon> = {
  windows: Monitor,
  macos: Laptop,
  linux: Laptop,
  android: Smartphone,
  ios: Tablet,
  unknown: Computer,
}

interface DeviceCardProps {
  device: DeviceInfo
  isLocalDevice: boolean
  onRotateKey?: (deviceId: string) => void
  onForgetTrust?: (deviceId: string) => void
  actingId?: string | null
}

export function DeviceCard({ device, isLocalDevice, onRotateKey, onForgetTrust, actingId }: DeviceCardProps) {
  const { t } = useTranslation()
  const Icon = iconByType[device.type]
  const status = getDeviceStatus(device, isLocalDevice, t)

  return (
    <article className="flex flex-col rounded-xl border bg-[hsl(var(--panel))] p-5 transition-all duration-200 hover:border-[hsl(var(--text)/0.2)]">
      <div className="flex items-start justify-between">
        <div className="flex items-center gap-3.5">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
            <Icon className="h-[17px] w-[17px] text-[hsl(var(--text-secondary))]" />
          </div>

          <div>
            <div className="text-[14px] font-medium leading-tight">{device.name}</div>
            <div className="mt-0.5 text-[12px] text-[hsl(var(--muted))]">{formatPlatformName(device.type)}</div>
          </div>
        </div>

        <div
          className={
            status.online
              ? 'flex items-center gap-1.5 text-[12px] text-[hsl(var(--success))]'
              : 'flex items-center gap-1.5 text-[12px] text-[hsl(var(--muted))]'
          }
        >
          <status.Icon className="h-3.5 w-3.5" />
          {status.label}
        </div>
      </div>

      <div className="mt-4 flex-1 space-y-3.5 text-[12px]">
        <div>
          <div className="text-[10px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">{t('devices.lastSeen')}</div>
          <div className="mt-1 text-[13px] font-medium text-[hsl(var(--text-secondary))]">{formatLastSeen(device.lastSeen)}</div>
        </div>
        <div>
          <div className="text-[10px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">{t('devices.deviceId')}</div>
          <div className="mt-1.5 break-all font-mono text-[11px] text-[hsl(var(--text-secondary))] bg-[hsl(var(--panel-2))] px-2.5 py-1.5 rounded-lg border select-all">
            {device.deviceId}
          </div>
        </div>
      </div>

      {(isLocalDevice && onRotateKey || device.type === 'unknown' && onForgetTrust) && (
        <div className="mt-5 flex items-center justify-end gap-2">
          {isLocalDevice && onRotateKey && (
            <button
              onClick={() => onRotateKey(device.deviceId)}
              disabled={actingId === device.deviceId}
              className="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg bg-white dark:bg-[hsl(var(--panel))] px-3 text-[12px] font-medium text-[hsl(var(--text))] border border-[hsl(var(--border))] transition-all hover:bg-[hsl(var(--panel-2))] dark:hover:bg-[hsl(var(--panel-2))] active:scale-[0.98] disabled:opacity-40"
              type="button"
            >
              <Key className="h-3.5 w-3.5 text-[hsl(var(--muted))]" />
              {actingId === device.deviceId ? t('devices.rotating') : t('devices.rotateKey')}
            </button>
          )}
          {device.type === 'unknown' && onForgetTrust && (
            <button
              onClick={() => onForgetTrust(device.deviceId)}
              disabled={actingId === device.deviceId}
              className="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg bg-white dark:bg-[hsl(var(--panel))] px-3 text-[12px] font-medium text-[hsl(var(--danger))] border border-[hsl(var(--border))] transition-all hover:bg-[hsl(var(--danger)/0.08)] active:scale-[0.98] disabled:opacity-40"
              type="button"
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t('devices.forgetTrust', { defaultValue: 'Forget' })}
            </button>
          )}
        </div>
      )}
    </article>
  )
}

function getDeviceStatus(device: DeviceInfo, isLocalDevice: boolean, t: any) {
  if (isLocalDevice) {
    return {
      Icon: Computer,
      label: t('devices.localDevice'),
      online: true,
    }
  }

  if (!device.online) {
    return {
      Icon: WifiOff,
      label: t('devices.offline'),
      online: false,
    }
  }

  if (device.lanAvailable) {
    return {
      Icon: Wifi,
      label: t('devices.lan'),
      online: true,
    }
  }

  return {
    Icon: Wifi,
    label: t('devices.online'),
    online: true,
  }
}
