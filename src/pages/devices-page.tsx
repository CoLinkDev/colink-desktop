import { RefreshCw } from 'lucide-react'
import { useState } from 'react'

import { DeviceCard } from '../components/device-card'
import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

export function DevicesPage() {
  const { devices, device, settings, refreshDevices } = useAppState()
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleRefresh() {
    setRefreshing(true)
    setError(null)

    try {
      await refreshDevices()
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <div className="space-y-8">
      <section className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_320px]">
        <div className="surface rounded-lg border border-[hsl(var(--border))] p-6">
          <div className="text-sm text-[hsl(var(--muted))]">当前状态</div>
          <div className="mt-2 text-2xl font-semibold">设备总览</div>
          <div className="mt-4 flex flex-wrap items-center gap-3 text-sm text-[hsl(var(--muted))]">
            <span>本机：{device?.name ?? '未注册'}</span>
            <span>远端：{settings.serverUrl}</span>
            <span>设备数：{devices.length}</span>
          </div>
        </div>

        <div className="surface rounded-lg border border-[hsl(var(--border))] p-6">
          <div className="text-sm text-[hsl(var(--muted))]">同步动作</div>
          <Button
            className="mt-4 w-full"
            disabled={refreshing}
            onClick={() => void handleRefresh()}
            variant="secondary"
          >
            <RefreshCw className="h-4 w-4" />
            {refreshing ? '正在刷新' : '刷新设备列表'}
          </Button>
          {error && <div className="mt-4 text-sm text-[hsl(var(--danger))]">{error}</div>}
        </div>
      </section>

      <section>
        <div className="mb-4">
          <div className="text-lg font-semibold">账户设备</div>
          <div className="mt-1 text-sm text-[hsl(var(--muted))]">
            在线状态来自服务端设备列表。
          </div>
        </div>

        {devices.length === 0 ? (
          <div className="surface rounded-lg border border-dashed border-[hsl(var(--border))] px-6 py-10 text-sm text-[hsl(var(--muted))]">
            还没有拿到设备记录。
          </div>
        ) : (
          <div className="grid gap-4 lg:grid-cols-2">
            {devices.map((item) => (
              <DeviceCard device={item} key={item.deviceId} />
            ))}
          </div>
        )}
      </section>
    </div>
  )
}
