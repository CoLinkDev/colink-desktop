import { useState } from 'react'
import { Link } from 'react-router-dom'

import { DeviceCard } from '../components/device-card'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

export function DevicesPage() {
  const {
    devices,
    device,
    rotateDeviceKey,
  } = useAppState()
  const [actingId, setActingId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function handleRotateKey(deviceId: string) {
    if (!window.confirm('确认轮换这台设备的公钥吗？')) {
      return
    }

    setActingId(deviceId)
    setError(null)
    try {
      await rotateDeviceKey(deviceId)
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  return (
    <div className="space-y-5 animate-fade-in">
      {error && (
        <div className="rounded-lg bg-[hsl(var(--danger)/0.08)] px-4 py-2.5 text-[13px] text-[hsl(var(--danger))]">
          {error}
        </div>
      )}

      {devices.length === 0 ? (
        <div className="py-16 text-center text-[13px] text-[hsl(var(--muted))]">
          还没有设备记录
        </div>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          {devices.map((item) => (
            <div className="space-y-2" key={item.deviceId}>
              <DeviceCard
                device={item}
                isLocalDevice={item.deviceId === device?.deviceId}
              />
              <div className="flex gap-1.5 pl-1">
                <Link
                  className="inline-flex h-7 items-center rounded-md px-2.5 text-[12px] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
                  to={`/messages?deviceId=${item.deviceId}`}
                >
                  发消息
                </Link>
                {item.deviceId === device?.deviceId && (
                  <button
                    className="inline-flex h-7 items-center rounded-md px-2.5 text-[12px] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
                    disabled={actingId === item.deviceId}
                    onClick={() => void handleRotateKey(item.deviceId)}
                    type="button"
                  >
                    轮换密钥
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
