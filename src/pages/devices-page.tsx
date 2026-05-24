import { RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router-dom'

import { DeviceCard } from '../components/device-card'
import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { formatTimestamp } from '../lib/utils'

export function DevicesPage() {
  const {
    cloud,
    devices,
    device,
    settings,
    logs,
    refreshDevices,
    updateDeviceName,
    deleteDevice,
    rotateDeviceKey,
  } = useAppState()
  const [refreshing, setRefreshing] = useState(false)
  const [actingId, setActingId] = useState<string | null>(null)
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

  async function handleRename(deviceId: string, currentName: string) {
    const nextName = window.prompt('输入新的设备名', currentName)?.trim()
    if (!nextName || nextName === currentName) {
      return
    }

    setActingId(deviceId)
    setError(null)
    try {
      await updateDeviceName(deviceId, nextName)
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  async function handleDelete(deviceId: string, name: string) {
    if (!window.confirm(`确认删除设备 ${name} 吗？`)) {
      return
    }

    setActingId(deviceId)
    setError(null)
    try {
      await deleteDevice(deviceId)
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

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
    <div className="space-y-8">
      <section className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_320px]">
        <div className="surface rounded-lg border border-[hsl(var(--border))] p-6">
          <div className="text-sm text-[hsl(var(--muted))]">当前状态</div>
          <div className="mt-2 text-2xl font-semibold">设备总览</div>
          <div className="mt-4 flex flex-wrap items-center gap-3 text-sm text-[hsl(var(--muted))]">
            <span>本机：{device?.name ?? '未注册'}</span>
            <span>远端：{settings.serverUrl}</span>
            <span>设备数：{devices.length}</span>
            <span>云端：{getCloudSummary(cloud.state, cloud.attempt)}</span>
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
          {cloud.lastError && (
            <div className="mt-4 text-sm text-[hsl(var(--muted))]">{cloud.lastError}</div>
          )}
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
              <div className="space-y-3" key={item.deviceId}>
                <DeviceCard device={item} />
                <div className="flex flex-wrap gap-2">
                  <Link
                    className="rounded-lg border border-[hsl(var(--border))] px-3 py-2 text-sm text-[hsl(var(--muted))] transition hover:text-[hsl(var(--text))]"
                    to={`/messages?deviceId=${item.deviceId}`}
                  >
                    去消息
                  </Link>
                  <Button
                    disabled={actingId === item.deviceId}
                    onClick={() => void handleRename(item.deviceId, item.name)}
                    size="sm"
                    variant="secondary"
                  >
                    改名
                  </Button>
                  <Button
                    disabled={actingId === item.deviceId}
                    onClick={() => void handleRotateKey(item.deviceId)}
                    size="sm"
                    variant="secondary"
                  >
                    轮换密钥
                  </Button>
                  <Button
                    disabled={actingId === item.deviceId}
                    onClick={() => void handleDelete(item.deviceId, item.name)}
                    size="sm"
                    variant="ghost"
                  >
                    删除
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      <section>
        <div className="mb-4">
          <div className="text-lg font-semibold">运行日志</div>
          <div className="mt-1 text-sm text-[hsl(var(--muted))]">
            最近的云端、消息、文件和剪贴板事件。
          </div>
        </div>

        <div className="surface rounded-lg border border-[hsl(var(--border))]">
          {logs.length === 0 ? (
            <div className="px-6 py-10 text-sm text-[hsl(var(--muted))]">还没有日志。</div>
          ) : (
            <div className="divide-y divide-[hsl(var(--border))]">
              {logs.slice(0, 20).map((item) => (
                <div className="px-6 py-4" key={item.id}>
                  <div className="flex flex-wrap items-center gap-3 text-xs text-[hsl(var(--muted))]">
                    <span>{item.level}</span>
                    <span>{item.source}</span>
                    <span>{formatTimestamp(item.createdAt)}</span>
                  </div>
                  <div className="mt-2 text-sm text-[hsl(var(--text))]">{item.message}</div>
                </div>
              ))}
            </div>
          )}
        </div>
      </section>
    </div>
  )
}

function getCloudSummary(state: string, attempt: number) {
  if (state === 'connected') {
    return '已连接'
  }

  if (state === 'connecting') {
    return '连接中'
  }

  if (state === 'reconnecting') {
    return `重连中 #${attempt}`
  }

  return '未连接'
}
