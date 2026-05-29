import { Send } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { cn, formatTimestamp, formatPlatformName } from '../lib/utils'

export function MessagesPage() {
  const { t, i18n } = useTranslation()
  const [searchParams, setSearchParams] = useSearchParams()
  const { device, devices, messages, sendText } = useAppState()
  const [selectedDeviceId, setSelectedDeviceId] = useState(searchParams.get('deviceId') ?? '')
  const [text, setText] = useState('')
  const [submitting, setSubmitting] = useState(false)

  const targetDevices = useMemo(() => devices.filter((i) => i.deviceId !== device?.deviceId), [device?.deviceId, devices])

  useEffect(() => {
    if (selectedDeviceId && targetDevices.some((i) => i.deviceId === selectedDeviceId)) return
    setSelectedDeviceId(searchParams.get('deviceId') ?? targetDevices[0]?.deviceId ?? '')
  }, [searchParams, selectedDeviceId, targetDevices])

  const selectedDevice = targetDevices.find((i) => i.deviceId === selectedDeviceId) ?? null
  const conversation = useMemo(() => messages.filter((i) => i.deviceId === selectedDeviceId).sort((a, b) => a.createdAt - b.createdAt), [messages, selectedDeviceId])

  async function handleSendText() {
    if (!selectedDeviceId) { toast.error(t('messages.errorSelectDevice')); return }
    const tVal = text.trim()
    if (!tVal) { toast.error(t('messages.errorEmptyText')); return }
    setSubmitting(true)
    try {
      await sendText({ deviceId: selectedDeviceId, text: tVal })
      setText('')
    } catch (e) {
      toast.error(readErrorMessage(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] gap-6 animate-fade-in overflow-hidden">
      <aside className="h-full overflow-y-auto py-6 pl-8 pr-1.5 space-y-1 scrollbar-thin">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">{t('messages.sidebarTitle')}</div>
        {targetDevices.length === 0 ? (
          <div className="py-8 text-center text-[13px] text-[hsl(var(--muted))]">{t('messages.emptyDevices')}</div>
        ) : targetDevices.map((item) => (
          <button
            className={cn(
              "w-full rounded-lg px-3 py-2.5 text-left border transition-all",
              item.deviceId === selectedDeviceId
                ? "border-[hsl(var(--text)/0.25)] bg-[hsl(var(--panel))] shadow-sm"
                : "border-transparent hover:bg-[hsl(var(--panel-2)/0.5)] bg-transparent"
            )}
            key={item.deviceId}
            onClick={() => { setSelectedDeviceId(item.deviceId); setSearchParams({ deviceId: item.deviceId }) }}
            type="button"
          >
            <div className="flex items-center justify-between">
              <span className="text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
              <span className={cn("text-[11px]", item.online ? "text-[hsl(var(--success))]" : "text-[hsl(var(--muted))]")}>{item.online ? t('devices.online') : t('devices.offline')}</span>
            </div>
            <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{formatPlatformName(item.type)}</div>
          </button>
        ))}
      </aside>

      <div className="h-full overflow-y-auto py-6 pr-8 pl-1 space-y-5 scrollbar-thin">
        <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
          <div className="flex items-center justify-between">
            <div className="text-[14px] font-medium">{selectedDevice?.name ?? t('messages.notSelected')}</div>
          </div>
          <div className="mt-4 rounded-lg bg-[hsl(var(--panel-2)/0.6)] p-3">
            <textarea className="min-h-20 w-full resize-none bg-transparent text-[13px] outline-none placeholder:text-[hsl(var(--muted))]" onChange={(e) => setText(e.target.value)} placeholder={t('messages.inputPlaceholder')} value={text} />
            <div className="mt-3 flex items-center justify-between">
              <span className="text-[11px] text-[hsl(var(--muted))]">{text.length}</span>
              <Button disabled={submitting || !selectedDeviceId} onClick={() => void handleSendText()} size="sm">
                <Send className="h-3 w-3" />{t('messages.send')}
              </Button>
            </div>
          </div>
        </section>

        <div className="rounded-xl border bg-[hsl(var(--panel))]">
          <div className="px-5 pt-4 pb-3 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">{t('messages.titleRecord')}</div>
          <div className="space-y-2 px-5 pb-5">
            {conversation.length === 0 ? (
              <div className="py-8 text-center text-[13px] text-[hsl(var(--muted))]">{t('messages.emptyConversation')}</div>
            ) : conversation.map((item) => (
              <div className={cn("max-w-[80%] rounded-2xl px-3.5 py-2.5 border transition-all", item.direction === 'outbound' ? "ml-auto rounded-br-md border-[hsl(var(--text)/0.04)] bg-[hsl(var(--text)/0.07)] text-[hsl(var(--text))]" : "rounded-bl-md border-[hsl(var(--border))] bg-[hsl(var(--panel-2)/0.3)] text-[hsl(var(--text))]")} key={item.messageId}>
                <div className="whitespace-pre-wrap text-[13px]">{item.text}</div>
                <div className={cn("mt-1.5 text-[10px]", item.direction === 'outbound' ? "opacity-50" : "text-[hsl(var(--muted))]")}>{formatTimestamp(item.createdAt, i18n.language)}</div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
