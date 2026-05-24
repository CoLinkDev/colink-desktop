import { Send } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { cn, formatTimestamp, formatPlatformName } from '../lib/utils'

export function MessagesPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { device, devices, messages, sendText } = useAppState()
  const [selectedDeviceId, setSelectedDeviceId] = useState(searchParams.get('deviceId') ?? '')
  const [text, setText] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const targetDevices = useMemo(() => devices.filter((i) => i.deviceId !== device?.deviceId), [device?.deviceId, devices])

  useEffect(() => {
    if (selectedDeviceId && targetDevices.some((i) => i.deviceId === selectedDeviceId)) return
    setSelectedDeviceId(searchParams.get('deviceId') ?? targetDevices[0]?.deviceId ?? '')
  }, [searchParams, selectedDeviceId, targetDevices])

  const selectedDevice = targetDevices.find((i) => i.deviceId === selectedDeviceId) ?? null
  const conversation = useMemo(() => messages.filter((i) => i.deviceId === selectedDeviceId).sort((a, b) => a.createdAt - b.createdAt), [messages, selectedDeviceId])

  async function handleSendText() {
    if (!selectedDeviceId) { setError('先选目标设备'); return }
    const t = text.trim()
    if (!t) { setError('消息不能为空'); return }
    setSubmitting(true); setError(null)
    try { await sendText({ deviceId: selectedDeviceId, text: t }); setText('') }
    catch (e) { setError(readErrorMessage(e)) }
    finally { setSubmitting(false) }
  }

  return (
    <div className="grid h-full gap-5 animate-fade-in xl:grid-cols-[240px_minmax(0,1fr)]">
      <aside className="space-y-1">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">设备</div>
        {targetDevices.length === 0 ? (
          <div className="py-8 text-center text-[13px] text-[hsl(var(--muted))]">无其他设备</div>
        ) : targetDevices.map((item) => (
          <button
            className={cn("w-full rounded-lg px-3 py-2.5 text-left transition-colors", item.deviceId === selectedDeviceId ? "bg-[hsl(var(--panel-2))]" : "hover:bg-[hsl(var(--panel-2)/0.5)]")}
            key={item.deviceId}
            onClick={() => { setSelectedDeviceId(item.deviceId); setSearchParams({ deviceId: item.deviceId }) }}
            type="button"
          >
            <div className="flex items-center justify-between">
              <span className="text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
              <span className={cn("text-[11px]", item.online ? "text-[hsl(var(--success))]" : "text-[hsl(var(--muted))]")}>{item.online ? '在线' : '离线'}</span>
            </div>
            <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{formatPlatformName(item.type)}</div>
          </button>
        ))}
      </aside>

      <div className="flex flex-col gap-5 overflow-hidden">
        <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
          <div className="flex items-center justify-between">
            <div className="text-[14px] font-medium">{selectedDevice?.name ?? '未选择'}</div>
          </div>
          <div className="mt-4 rounded-lg bg-[hsl(var(--panel-2)/0.6)] p-3">
            <textarea className="min-h-20 w-full resize-none bg-transparent text-[13px] outline-none placeholder:text-[hsl(var(--muted))]" onChange={(e) => setText(e.target.value)} placeholder="输入消息…" value={text} />
            <div className="mt-3 flex items-center justify-between">
              <span className="text-[11px] text-[hsl(var(--muted))]">{text.length}</span>
              <Button disabled={submitting || !selectedDeviceId} onClick={() => void handleSendText()} size="sm">
                <Send className="h-3 w-3" />发送
              </Button>
            </div>
          </div>
          {error && <div className="mt-3 text-[12px] text-[hsl(var(--danger))]">{error}</div>}
        </section>

        <div className="flex flex-1 flex-col overflow-hidden rounded-xl border bg-[hsl(var(--panel))]">
          <div className="shrink-0 px-5 pt-4 pb-3 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">消息记录</div>
          <div className="flex-1 space-y-2 overflow-y-auto px-5 pb-5">
            {conversation.length === 0 ? (
              <div className="py-8 text-center text-[13px] text-[hsl(var(--muted))]">暂无消息</div>
            ) : conversation.map((item) => (
              <div className={cn("max-w-[80%] rounded-2xl px-3.5 py-2.5", item.direction === 'outbound' ? "ml-auto rounded-br-md bg-[hsl(var(--text))] text-[hsl(var(--panel))]" : "rounded-bl-md bg-[hsl(var(--panel-2))]")} key={item.messageId}>
                <div className="whitespace-pre-wrap text-[13px]">{item.text}</div>
                <div className={cn("mt-1.5 text-[10px]", item.direction === 'outbound' ? "opacity-50" : "text-[hsl(var(--muted))]")}>{formatTimestamp(item.createdAt)}</div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
