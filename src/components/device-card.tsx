import type { LucideIcon } from 'lucide-react'
import { Cloud, Computer, Laptop, Monitor, Network, Smartphone, Tablet, WifiOff, Key, Trash2, Info } from 'lucide-react'
import type { TFunction } from 'i18next'
import { useTranslation } from 'react-i18next'

import type { DeviceInfo, DevicePlatform } from '../lib/types'
import { cn, formatLastSeen, formatPlatformName } from '../lib/utils'

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
  onViewDetails?: (device: DeviceInfo) => void
  onRotateKey?: (deviceId: string) => void
  onForgetTrust?: (deviceId: string) => void
  actingId?: string | null
}

export function DeviceCard({
  device,
  isLocalDevice,
  onViewDetails,
  onRotateKey,
  onForgetTrust,
  actingId,
}: DeviceCardProps) {
  const { t } = useTranslation()
  const Icon = iconByType[device.type]
  const statuses = getDeviceStatuses(device, isLocalDevice, t)
  const canForgetTrust = device.deviceSources.includes('trusted_peer_key') && Boolean(onForgetTrust)

  return (
    <article className="flex flex-col rounded-xl border bg-[hsl(var(--panel))] p-5 transition-all duration-200 hover:border-[hsl(var(--text)/0.2)]">
      <div className="flex items-start justify-between">
        <div className="flex items-center gap-3.5">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
            <Icon className="h-[17px] w-[17px] text-[hsl(var(--text-secondary))]" />
          </div>

          <div>
            <div className="text-[14px] font-medium leading-tight">{device.name}</div>
            <div className="mt-0.5 text-[12px] text-[hsl(var(--muted))]">{formatPlatformName(device.type, t)}</div>
          </div>
        </div>

        <div className="flex max-w-[45%] flex-wrap justify-end gap-2">
          {statuses.map((status) => (
            <div
              className={cn(
                'flex items-center gap-1.5 text-[12px]',
                status.tone === 'success' && 'text-[hsl(var(--success))]',
                status.tone === 'warning' && 'text-[hsl(var(--warning))]',
                status.tone === 'muted' && 'text-[hsl(var(--muted))]',
              )}
              key={status.label}
            >
              <status.Icon className="h-3.5 w-3.5" />
              {status.label}
            </div>
          ))}
        </div>
      </div>

      <div className="mt-4 flex-1 space-y-3.5 text-[12px]">
        <div>
          <div className="text-[10px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">{t('devices.lastSeen')}</div>
          <div className="mt-1 text-[13px] font-medium text-[hsl(var(--text-secondary))]">{formatLastSeen(device.lastSeen, t('devices.neverConnected'))}</div>
        </div>
        <div>
          <div className="text-[10px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">{t('devices.deviceId')}</div>
          <div className="mt-1.5 break-all font-mono text-[11px] text-[hsl(var(--text-secondary))] bg-[hsl(var(--panel-2))] px-2.5 py-1.5 rounded-lg border select-all">
            {device.deviceId}
          </div>
        </div>
      </div>

      <div className="mt-5 flex items-center justify-end gap-2">
        {onViewDetails && (
          <button
            onClick={() => onViewDetails(device)}
            className="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg bg-white dark:bg-[hsl(var(--panel))] px-3 text-[12px] font-medium text-[hsl(var(--text))] border border-[hsl(var(--border))] transition-all hover:bg-[hsl(var(--panel-2))] dark:hover:bg-[hsl(var(--panel-2))] active:scale-[0.98]"
            type="button"
          >
            <Info className="h-3.5 w-3.5 text-[hsl(var(--muted))]" />
            {t('devices.details')}
          </button>
        )}

        {(isLocalDevice && onRotateKey || canForgetTrust) && (
          <>
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
            {canForgetTrust && (
              <button
                onClick={() => onForgetTrust?.(device.deviceId)}
                disabled={actingId === device.deviceId}
                className="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg bg-white dark:bg-[hsl(var(--panel))] px-3 text-[12px] font-medium text-[hsl(var(--danger))] border border-[hsl(var(--border))] transition-all hover:bg-[hsl(var(--danger)/0.08)] active:scale-[0.98] disabled:opacity-40"
                type="button"
              >
                <Trash2 className="h-3.5 w-3.5" />
                {t('devices.forgetTrust')}
              </button>
            )}
          </>
        )}
      </div>
    </article>
  )
}

function getDeviceStatuses(device: DeviceInfo, isLocalDevice: boolean, t: TFunction) {
  const statuses = []

  if (isLocalDevice) {
    statuses.push({ Icon: Computer, label: t('devices.localDevice'), tone: 'success' })
  }

  if (device.cloudAvailable) {
    statuses.push({ Icon: Cloud, label: t('devices.cloud'), tone: 'success' })
  }

  if (device.lanAvailable) {
    statuses.push({
      Icon: Network,
      label: device.lanState === 'suspect' ? t('devices.lanSuspect') : t('devices.lan'),
      tone: device.lanState === 'suspect' ? 'warning' : 'success',
    })
  }

  return statuses.length > 0
    ? statuses
    : [{ Icon: WifiOff, label: t('devices.offline'), tone: 'muted' }]
}
