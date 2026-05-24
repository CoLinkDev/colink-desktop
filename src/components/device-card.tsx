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
    <article className="rounded-xl border bg-[hsl(var(--panel))] p-5 transition-colors hover:bg-[hsl(var(--panel-2)/0.5)]">
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

      <div className="mt-4 grid gap-3 text-[12px] text-[hsl(var(--muted))] md:grid-cols-2">
        <div>
          <div className="text-[11px] uppercase tracking-widest">Last Seen</div>
          <div className="mt-1 text-[hsl(var(--text-secondary))]">{formatLastSeen(device.lastSeen)}</div>
        </div>
        <div>
          <div className="text-[11px] uppercase tracking-widest">Device ID</div>
          <div className="mt-1 break-all font-mono text-[11px] text-[hsl(var(--text-secondary))]">
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
      label: '局域网',
      online: true,
    }
  }

  return {
    Icon: Wifi,
    label: '在线',
    online: true,
  }
}
