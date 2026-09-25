import { listen } from '@tauri-apps/api/event'
import { ArrowUpDown, HardDriveUpload, Paperclip, Send } from 'lucide-react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { FileOfferBubble, TransferBubbleCard } from '../components/transfer-bubble'
import { TransferDetailDialog } from '../components/transfer-detail-dialog'
import { DeviceSidebar, useTargetDevices } from '../components/device-sidebar'
import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { openReceivedFile, pendingFileOffers, respondFileOffer, revealReceivedFile } from '../lib/api'
import { cn, formatTimestamp } from '../lib/utils'
import type { FileOfferRequest, FileTransferRecord, TextMessageRecord, TransferPreparingPayload } from '../lib/types'

const TRANSFER_PREPARING_EVENT = 'transfer-preparing'

interface PendingOffer {
  request: FileOfferRequest
  receivedAt: number
}

type TimelineItem =
  | { kind: 'message'; id: string; timestamp: number; direction: 'inbound' | 'outbound'; data: TextMessageRecord }
  | { kind: 'transfer'; id: string; timestamp: number; direction: 'inbound' | 'outbound'; data: FileTransferRecord }
  | { kind: 'offer'; id: string; timestamp: number; direction: 'inbound'; data: FileOfferRequest }

export function TransfersPage() {
  const { t, i18n } = useTranslation()
  const {
    devices,
    messages,
    transfers,
    transferSpeeds,
    settings,
    pickFiles,
    sendText,
    sendFiles,
    cancelTransfer,
  } = useAppState()
  const {
    devices: targetDevices,
    selectedDeviceId,
    selectedDevice,
    selectDevice,
  } = useTargetDevices({
    sort: 'online-first',
    autoSelectFirst: true,
  })
  const [text, setText] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [preparing, setPreparing] = useState<TransferPreparingPayload | null>(null)
  const [isDragging, setIsDragging] = useState(false)
  const [detailTransfer, setDetailTransfer] = useState<FileTransferRecord | null>(null)
  const [pendingOffers, setPendingOffers] = useState<PendingOffer[]>([])
  const [actingOfferId, setActingOfferId] = useState<string | null>(null)
  const timelineRef = useRef<HTMLDivElement>(null)
  const selectedDeviceIdRef = useRef('')

  useEffect(() => {
    selectedDeviceIdRef.current = selectedDeviceId
  }, [selectedDeviceId])

  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []
    void (async () => {
      try {
        unlisteners.push(await listen<TransferPreparingPayload>(TRANSFER_PREPARING_EVENT, (event) => {
          if (!disposed) setPreparing(event.payload)
        }))
        unlisteners.push(await listen<FileOfferRequest>('file-offer-requested', (event) => {
          if (disposed) return
          if (event.payload.purpose !== 'transfer') return
          setPendingOffers((current) => current.some((item) => item.request.sessionId === event.payload.sessionId)
            ? current
            : [...current, { request: event.payload, receivedAt: Date.now() }])
        }))
        unlisteners.push(await listen<string>('file-offer-ended', (event) => {
          if (!disposed) setPendingOffers((current) => current.filter((item) => item.request.sessionId !== event.payload))
        }))
        const pending = await pendingFileOffers()
        if (!disposed) {
          setPendingOffers(pending
            .filter((request) => request.purpose === 'transfer')
            .map((request) => ({ request, receivedAt: Date.now() })))
        }
      } catch {
        // Ignore browser-mode event failures. The desktop runtime provides these events.
      }
    })()
    return () => {
      disposed = true
      unlisteners.forEach((unlisten) => unlisten())
    }
  }, [])

  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []
    void (async () => {
      try {
        const enter = await listen('tauri://drag-enter', () => { if (!disposed) setIsDragging(true) })
        const leave = await listen('tauri://drag-leave', () => { if (!disposed) setIsDragging(false) })
        const drop = await listen<{ paths: string[] }>('tauri://drag-drop', async (event) => {
          if (disposed) return
          setIsDragging(false)
          await sendPaths(event.payload.paths)
        })
        if (disposed) {
          enter(); leave(); drop()
        } else {
          unlisteners.push(enter, leave, drop)
        }
      } catch {
        // Ignore browser-mode event failures.
      }
    })()
    return () => {
      disposed = true
      unlisteners.forEach((unlisten) => unlisten())
    }
  }, [])

  const timelineItems = useMemo<TimelineItem[]>(() => {
    const items: TimelineItem[] = [
      ...messages
        .filter((item) => item.deviceId === selectedDeviceId)
        .map((data) => ({ kind: 'message' as const, id: data.messageId, timestamp: data.createdAt, direction: data.direction, data })),
      ...transfers
        .filter((item) => item.deviceId === selectedDeviceId)
        .map((data) => ({ kind: 'transfer' as const, id: data.fileId, timestamp: data.updatedAt || data.createdAt, direction: data.direction, data })),
      ...pendingOffers
        .filter((item) => item.request.deviceId === selectedDeviceId)
        .map((item) => ({ kind: 'offer' as const, id: item.request.sessionId, timestamp: item.receivedAt, direction: 'inbound' as const, data: item.request })),
    ]
    return items.sort((left, right) => left.timestamp - right.timestamp || left.id.localeCompare(right.id))
  }, [messages, pendingOffers, selectedDeviceId, transfers])

  const submitLabel = submitting
    ? preparing ? t('transfers.hashingProgress', { current: preparing.current, total: preparing.total }) : t('transfers.preparingSend')
    : t('transfers.selectBtn')

  useLayoutEffect(() => {
    const element = timelineRef.current
    if (element) element.scrollTop = element.scrollHeight
  }, [selectedDeviceId, timelineItems])

  async function sendPaths(paths: string[]) {
    if (!paths.length) return
    const currentDeviceId = selectedDeviceIdRef.current
    if (!currentDeviceId) {
      setError(t('transfers.errorSelectDevice'))
      return
    }
    setSubmitting(true)
    setPreparing(null)
    setError(null)
    try {
      await sendFiles({ deviceId: currentDeviceId, paths })
    } catch (cause) {
      setError(readErrorMessage(cause))
    } finally {
      setSubmitting(false)
      setPreparing(null)
    }
  }

  async function handlePickFiles() {
    try {
      const paths = await pickFiles(true)
      await sendPaths(paths)
    } catch (cause) {
      setError(readErrorMessage(cause))
    }
  }

  async function handleSendText() {
    if (!selectedDeviceId) {
      toast.error(t('messages.errorSelectDevice'))
      return
    }
    const value = text.trim()
    if (!value) {
      toast.error(t('messages.errorEmptyText'))
      return
    }
    setSubmitting(true)
    try {
      await sendText({ deviceId: selectedDeviceId, text: value })
      setText('')
    } catch (cause) {
      toast.error(readErrorMessage(cause))
    } finally {
      setSubmitting(false)
    }
  }

  function handleTextKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault()
      void handleSendText()
    }
  }

  async function handleOffer(request: FileOfferRequest, accepted: boolean) {
    setActingOfferId(request.sessionId)
    try {
      await respondFileOffer(request.sessionId, accepted, accepted ? settings.downloadPath : undefined)
      setPendingOffers((current) => current.filter((item) => item.request.sessionId !== request.sessionId))
    } catch (cause) {
      toast.error(readErrorMessage(cause))
    } finally {
      setActingOfferId(null)
    }
  }

  async function handleOpen(fileId: string) {
    try { await openReceivedFile(fileId) } catch (cause) { toast.error(readErrorMessage(cause)) }
  }

  async function handleReveal(fileId: string) {
    try { await revealReceivedFile(fileId) } catch (cause) { toast.error(readErrorMessage(cause)) }
  }

  return (
    <div className="grid h-full min-h-0 grid-cols-[240px_minmax(0,1fr)] animate-fade-in overflow-hidden">
      <DeviceSidebar
        devices={targetDevices}
        emptyText={t('transfers.emptyDevices')}
        onSelectDevice={selectDevice}
        selectedDeviceId={selectedDeviceId}
        title={t('transfers.sidebarTitle')}
      />

      <section className="flex min-h-0 flex-col gap-3 py-5 px-8">
        <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border bg-[hsl(var(--panel))]">
          <div
            className={cn(
              "pointer-events-none absolute inset-3 z-20 flex items-center justify-center rounded-xl border-2 border-dashed border-[hsl(var(--text)/0.35)] bg-[hsl(var(--panel)/0.92)] backdrop-blur-md transition-opacity duration-200 ease-out",
              isDragging ? "opacity-100" : "opacity-0"
            )}
          >
            <div className="flex flex-col items-center gap-2.5 text-center">
              <div className="flex h-12 w-12 items-center justify-center rounded-full bg-[hsl(var(--panel-2))] shadow-sm">
                <HardDriveUpload className="h-6 w-6 text-[hsl(var(--text))] animate-pulse-soft" />
              </div>
              <span className="text-[13px] font-semibold text-[hsl(var(--text))]">
                {selectedDevice ? t('transfers.dropToDevice', { name: selectedDevice.name }) : t('transfers.errorSelectDevice')}
              </span>
            </div>
          </div>

          <div className="h-full overflow-y-auto px-4 py-5 scrollbar-thin" ref={timelineRef}>
            {timelineItems.length === 0 ? (
              <div className="flex h-full min-h-48 flex-col items-center justify-center gap-2 text-center text-[13px] text-[hsl(var(--muted))]"><ArrowUpDown className="h-6 w-6 opacity-50" /><span>{t('messages.emptyConversation')}</span></div>
            ) : (
              <div className="space-y-3">
                {timelineItems.map((item) => item.kind === 'message' ? (
                  <div className={cn('max-w-[min(78%,560px)] rounded-2xl border px-3.5 py-2.5', item.direction === 'outbound' ? 'ml-auto rounded-br-md border-[hsl(var(--text)/0.08)] bg-[hsl(var(--text)/0.07)]' : 'rounded-bl-md border-[hsl(var(--border))] bg-[hsl(var(--panel-2)/0.35)]')} key={item.id}>
                    <div className="whitespace-pre-wrap break-words text-[13px] text-[hsl(var(--text))]">{item.data.text}</div>
                    <div className="mt-1.5 text-[10px] text-[hsl(var(--muted))]">{formatTimestamp(item.timestamp, i18n.language)}</div>
                  </div>
                ) : item.kind === 'transfer' ? (
                  <TransferBubbleCard key={item.id} onCancel={(fileId) => void cancelTransfer(fileId)} onDetails={setDetailTransfer} onOpen={(fileId) => void handleOpen(fileId)} onReveal={(fileId) => void handleReveal(fileId)} speed={transferSpeeds[item.data.fileId]} transfer={item.data} />
                ) : (
                  <FileOfferBubble acting={actingOfferId === item.data.sessionId} key={item.id} onRespond={(request, accepted) => void handleOffer(request, accepted)} request={item.data} timestamp={item.timestamp} />
                ))}
              </div>
            )}
          </div>
        </div>

        <div className="shrink-0 rounded-xl border bg-[hsl(var(--panel))] p-3">
          {error && <div className="mb-2 text-[12px] text-[hsl(var(--danger))]">{error}</div>}
          <div className="flex items-end gap-2">
            <textarea aria-label={t('messages.inputPlaceholder')} className="max-h-32 min-h-9 flex-1 resize-none rounded-lg border border-transparent bg-[hsl(var(--panel-2))] px-3 py-2 text-[13px] text-[hsl(var(--text))] outline-none placeholder:text-[hsl(var(--muted))] focus:border-[hsl(var(--border))]" disabled={submitting || !selectedDeviceId || !selectedDevice?.online} onChange={(event) => setText(event.target.value)} onKeyDown={handleTextKeyDown} placeholder={t('messages.inputPlaceholder')} value={text} />
            <Button aria-label={t('transfers.selectBtn')} className="h-9 w-9 shrink-0 px-0" disabled={submitting || !selectedDeviceId || !selectedDevice?.online} onClick={() => void handlePickFiles()} title={submitLabel} variant="secondary"><Paperclip className="h-4 w-4" /></Button>
            <Button aria-label={t('messages.send')} className="h-9 w-9 shrink-0 px-0" disabled={submitting || !selectedDeviceId || !selectedDevice?.online || !text.trim()} onClick={() => void handleSendText()} title={t('messages.send')}><Send className="h-4 w-4" /></Button>
          </div>
        </div>
      </section>

      {detailTransfer && <TransferDetailDialog deviceName={devices.find((item) => item.deviceId === detailTransfer.deviceId)?.name ?? null} onClose={() => setDetailTransfer(null)} transfer={detailTransfer} />}
    </div>
  )
}
