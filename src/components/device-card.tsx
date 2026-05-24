import type { LucideIcon } from 'lucide-react'
import { Computer, Laptop, Monitor, Smartphone, Tablet, Wifi, WifiOff } from 'lucide-react'

import type { DeviceInfo, DevicePlatform } from '../lib/types'
import { formatLastSeen, formatPlatformName } from '../lib/utils'

const iconByType: Record<DevicePlatform, LucideIcon> = {
  windows: Monitor,
  macos: Laptop,
  linux: Laptop,
  android: Smartphone,
  ios: Tablet,
}

interface DeviceCardProps {
  device: DeviceInfo
  isLocalDevice: boolean
}

export function DeviceCard({ device, isLocalDevice }: DeviceCardProps) {
  const Icon = iconByType[device.type]
  const status = getDeviceStatus(device, isLocalDevice)

  return (
    <article className="surface rounded-lg border border-[hsl(var(--border))] p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-start gap-4">
          <div className="surface-muted flex h-11 w-11 items-center justify-center rounded-lg border border-[hsl(var(--border))]">
            <Icon className="h-5 w-5 text-[hsl(var(--text))]" />
          </div>

          <div>
            <div className="text-base font-medium">{device.name}</div>
            <div className="mt-1 text-sm text-[hsl(var(--muted))]">{formatPlatformName(device.type)}</div>
          </div>
        </div>

        <div
          className={
            status.online
              ? 'flex items-center gap-2 text-sm text-[hsl(var(--accent))]'
              : 'flex items-center gap-2 text-sm text-[hsl(var(--muted))]'
          }
        >
          <status.Icon className="h-4 w-4" />
          {status.label}
        </div>
      </div>

      <div className="mt-5 grid gap-4 text-sm text-[hsl(var(--muted))] md:grid-cols-2">
        <div>
          <div className="text-xs uppercase tracking-[0.1em]">Last Seen</div>
          <div className="mt-2 text-[hsl(var(--text))]">{formatLastSeen(device.lastSeen)}</div>
        </div>
        <div>
          <div className="text-xs uppercase tracking-[0.1em]">Device ID</div>
          <div className="mt-2 break-all font-mono text-xs text-[hsl(var(--text))]">
            {device.deviceId}
          </div>
        </div>
      </div>
    </article>
  )
}

function getDeviceStatus(device: DeviceInfo, isLocalDevice: boolean) {
  if (isLocalDevice) {
    return {
      Icon: Computer,
      label: '本机',
      online: true,
    }
  }

  if (!device.online) {
    return {
      Icon: WifiOff,
      label: '离线',
      online: false,
    }
  }

  if (device.lanAvailable) {
    return {
      Icon: Wifi,
      label: '云端在线 · 局域网在线',
      online: true,
    }
  }

  return {
    Icon: Wifi,
    label: '云端在线',
    online: true,
  }
}
