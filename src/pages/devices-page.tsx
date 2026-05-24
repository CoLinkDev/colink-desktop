import { useState } from 'react'
import { createPortal } from 'react-dom'
import { toast } from 'sonner'

import { DeviceCard } from '../components/device-card'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { Button } from '../components/ui/button'

export function DevicesPage() {
  const {
    devices,
    device,
    rotateDeviceKey,
  } = useAppState()
  const [actingId, setActingId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [rotateConfirmId, setRotateConfirmId] = useState<string | null>(null)

  function handleInitiateRotate(deviceId: string) {
    setRotateConfirmId(deviceId)
    setError(null)
  }

  async function handleConfirmRotate() {
    if (!rotateConfirmId) return

    setActingId(rotateConfirmId)
    setError(null)
    try {
      await rotateDeviceKey(rotateConfirmId)
      setRotateConfirmId(null)
      toast.success('安全密钥已成功轮换')
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  const rotatingDevice = devices.find((d) => d.deviceId === rotateConfirmId)

  return (
    <div className="space-y-5 animate-fade-in">
      {error && !rotateConfirmId && (
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
            <DeviceCard
              key={item.deviceId}
              device={item}
              isLocalDevice={item.deviceId === device?.deviceId}
              onRotateKey={handleInitiateRotate}
              actingId={actingId}
            />
          ))}
        </div>
      )}

      {/* Rotate Key Confirmation Modal */}
      {rotateConfirmId && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">轮换密钥确认</div>
            <p className="mt-2 text-[13px] text-[hsl(var(--text-secondary))] leading-relaxed">
              确定要轮换设备 <strong className="font-semibold text-[hsl(var(--text))]">“{rotatingDevice?.name || '未知设备'}”</strong> 的安全密钥吗？此操作将重新生成并同步该设备的安全凭证。
            </p>

            {error && (
              <div className="mt-4 rounded-lg bg-[hsl(var(--danger)/0.08)] px-3.5 py-2.5 text-[12px] text-[hsl(var(--danger))] border border-[hsl(var(--danger)/0.15)]">
                {error}
              </div>
            )}

            <div className="mt-6 flex justify-end gap-2">
              <Button
                variant="secondary"
                onClick={() => {
                  setRotateConfirmId(null)
                  setError(null)
                }}
                disabled={!!actingId}
              >
                取消
              </Button>
              <Button
                variant="primary"
                onClick={handleConfirmRotate}
                disabled={!!actingId}
              >
                {actingId ? '轮换中...' : '确认轮换'}
              </Button>
            </div>
          </div>
        </div>,
        document.body
      )}
    </div>
  )
}
