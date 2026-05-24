import { useState } from 'react'
import { Link } from 'react-router-dom'

import { DeviceCard } from '../components/device-card'
import { Button } from '../components/ui/button'
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
    <div className="space-y-6">
      {error && (
        <div className="rounded-lg border border-[hsl(var(--danger)/0.5)] bg-[hsl(var(--danger)/0.12)] px-4 py-2.5 text-sm text-[hsl(var(--danger))]">
          {error}
        </div>
      )}

      {devices.length === 0 ? (
        <div className="surface rounded-lg border border-dashed border-[hsl(var(--border))] px-6 py-10 text-sm text-[hsl(var(--muted))]">
          还没有拿到设备记录。
        </div>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {devices.map((item) => (
            <div className="space-y-3" key={item.deviceId}>
              <DeviceCard
                device={item}
                isLocalDevice={item.deviceId === device?.deviceId}
              />
              <div className="flex flex-wrap gap-2">
                <Link
                  className="inline-flex h-9 items-center justify-center rounded-lg border border-[hsl(var(--border))] px-4 py-2 text-sm text-[hsl(var(--muted))] transition hover:text-[hsl(var(--text))] hover:bg-[hsl(var(--panel-2))]"
                  to={`/messages?deviceId=${item.deviceId}`}
                >
                  发送消息
                </Link>
                {item.deviceId === device?.deviceId && (
                  <Button
                    disabled={actingId === item.deviceId}
                    onClick={() => void handleRotateKey(item.deviceId)}
                    size="sm"
                    variant="secondary"
                  >
                    轮换密钥
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
