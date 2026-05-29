import { ArrowUpDown, HardDriveUpload, X, CheckCircle2, AlertCircle, ArrowUp, ArrowDown, LoaderCircle, Trash2 } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useMemo, useState, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { cn, formatBytes, formatPlatformName } from '../lib/utils'
import type { TransferPreparingPayload } from '../lib/types'

const TRANSFER_PREPARING_EVENT = 'transfer-preparing'

export function TransfersPage() {
  const { t } = useTranslation()
  const [searchParams, setSearchParams] = useSearchParams()
  const { device, devices, transfers, transferSpeeds, pickFiles, sendFiles, cancelTransfer, clearTransfers } = useAppState()
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [preparing, setPreparing] = useState<TransferPreparingPayload | null>(null)
  const [isDragging, setIsDragging] = useState(false)

  const targetDevices = useMemo(() => devices.filter((i) => i.deviceId !== device?.deviceId), [device?.deviceId, devices])
  const selectedDeviceId = useMemo(() => {
    const requestedDeviceId = searchParams.get('deviceId')
    if (requestedDeviceId && targetDevices.some((item) => item.deviceId === requestedDeviceId)) {
      return requestedDeviceId
    }
    return targetDevices[0]?.deviceId ?? ''
  }, [searchParams, targetDevices])

  const selectedDevice = targetDevices.find((i) => i.deviceId === selectedDeviceId) ?? null
  const transferItems = useMemo(() => transfers.filter((i) => selectedDeviceId ? i.deviceId === selectedDeviceId : true), [selectedDeviceId, transfers])
  const hasClearableTransfers = useMemo(() => {
    return transferItems.some((i) => i.status === 'completed' || i.status === 'failed' || i.status === 'cancelled' || i.status === 'rejected')
  }, [transferItems])
  const submitLabel = submitting
    ? preparing
      ? t('transfers.hashingProgress', {
        current: preparing.current,
        total: preparing.total,
        defaultValue: `Calculating file hashes (${preparing.current}/${preparing.total})...`,
      })
      : t('transfers.preparingSend', { defaultValue: 'Preparing to send...' })
    : t('transfers.selectBtn')

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        unlisten = await listen<TransferPreparingPayload>(TRANSFER_PREPARING_EVENT, (event) => {
          if (!disposed) {
            setPreparing(event.payload)
          }
        })
      } catch {
        // Ignore browser-mode event failures. The desktop runtime provides this event.
      }
    })()

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  // Ref to hold selectedDeviceId for the async drop listener closure
  const selectedDeviceIdRef = useRef(selectedDeviceId)
  useEffect(() => {
    selectedDeviceIdRef.current = selectedDeviceId
  }, [selectedDeviceId])

  // System Drag and Drop listener setup
  useEffect(() => {
    let disposed = false
    const unlisteners: (() => void)[] = []

    void (async () => {
      try {
        const enter = await listen('tauri://drag-enter', () => {
          if (!disposed) setIsDragging(true)
        })
        if (disposed) {
          enter()
        } else {
          unlisteners.push(enter)
        }

        const leave = await listen('tauri://drag-leave', () => {
          if (!disposed) setIsDragging(false)
        })
        if (disposed) {
          leave()
        } else {
          unlisteners.push(leave)
        }

        const drop = await listen<{ paths: string[] }>('tauri://drag-drop', async (event) => {
          if (disposed) return
          setIsDragging(false)

          const paths = event.payload.paths
          const currentDeviceId = selectedDeviceIdRef.current
          if (paths && paths.length > 0) {
            if (!currentDeviceId) {
              setError(t('transfers.errorSelectDevice'))
              return
            }
            setSubmitting(true)
            setPreparing(null)
            setError(null)
            try {
              await sendFiles({ deviceId: currentDeviceId, paths })
            } catch (e) {
              setError(readErrorMessage(e))
            } finally {
              setSubmitting(false)
              setPreparing(null)
            }
          }
        })
        if (disposed) {
          drop()
        } else {
          unlisteners.push(drop)
        }
      } catch {
        // Ignore browser mode failures
      }
    })()

    return () => {
      disposed = true
      unlisteners.forEach((fn) => fn())
    }
  }, [])

  async function handleSendFiles() {
    if (!selectedDeviceId) { setError(t('transfers.errorSelectDevice')); return }
    setSubmitting(true); setPreparing(null); setError(null)
    try {
      const paths = await pickFiles(true)
      if (paths.length === 0) return
      await sendFiles({ deviceId: selectedDeviceId, paths })
    } catch (e) { setError(readErrorMessage(e)) }
    finally { setSubmitting(false); setPreparing(null) }
  }

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] gap-6 animate-fade-in overflow-hidden">
      {/* Device List Sidebar */}
      <aside className="h-full overflow-y-auto py-6 pl-8 pr-1.5 space-y-1 scrollbar-thin">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">{t('transfers.sidebarTitle')}</div>
        {targetDevices.length === 0 ? (
          <div className="py-8 text-center text-[13px] text-[hsl(var(--muted))]">{t('transfers.emptyDevices')}</div>
        ) : targetDevices.map((item) => (
          <button
            className={cn(
              "w-full rounded-lg px-3 py-2.5 text-left border transition-all",
              item.deviceId === selectedDeviceId
                ? "border-[hsl(var(--text)/0.25)] bg-[hsl(var(--panel))] shadow-sm"
                : "border-transparent hover:bg-[hsl(var(--panel-2)/0.5)] bg-transparent"
            )}
            key={item.deviceId}
            onClick={() => { setSearchParams({ deviceId: item.deviceId }) }}
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
        <section 
          className={cn(
            "rounded-xl border p-6 text-center transition-all duration-300 relative overflow-hidden",
            isDragging 
              ? "border-[hsl(var(--text)/0.5)] bg-[hsl(var(--panel-2)/0.3)] shadow-[0_0_20px_rgba(255,255,255,0.05)]" 
              : "border-[hsl(var(--border))] bg-[hsl(var(--panel))]"
          )}
        >
          {isDragging && (
            <div className="absolute inset-0 bg-[hsl(var(--panel-2)/0.4)] backdrop-blur-[2px] flex flex-col items-center justify-center pointer-events-none z-10 animate-fade-in border-2 border-dashed border-[hsl(var(--text)/0.35)] rounded-xl">
              <div className="flex h-12 w-12 items-center justify-center rounded-full bg-[hsl(var(--text)/0.08)] border border-[hsl(var(--text)/0.15)] mb-2">
                <HardDriveUpload className="h-6 w-6 text-[hsl(var(--text))]" />
              </div>
              <p className="text-[13px] font-semibold text-[hsl(var(--text))]">
                {selectedDevice 
                  ? t('transfers.dropToDevice', { name: selectedDevice.name, defaultValue: `Release to send files to ${selectedDevice.name}` })
                  : t('transfers.errorSelectDevice')}
              </p>
            </div>
          )}

          <div className={cn("transition-all duration-300", isDragging && "opacity-0")}>
            <div className="mx-auto flex h-11 w-11 items-center justify-center rounded-full bg-[hsl(var(--panel-2))]">
              <HardDriveUpload className="h-5 w-5 text-[hsl(var(--text-secondary))]" />
            </div>
            <div className="mt-3">
              <h2 className="text-[14px] font-semibold">
                {t('transfers.sendTitle', { name: selectedDevice?.name || t('transfers.notSelected') })}
              </h2>
              <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">
                {t('transfers.sendSubtitle')}
              </p>
            </div>
            <div className="mt-4 flex flex-col items-center justify-center gap-2">
              <Button
                disabled={submitting || !selectedDeviceId}
                onClick={() => void handleSendFiles()}
                className="px-6"
              >
                {submitting && <LoaderCircle className="h-3.5 w-3.5 animate-spin" />}
                {submitLabel}
              </Button>
              {error && (
                <div className="mt-2 text-[12px] text-[hsl(var(--danger))]">
                  {error}
                </div>
              )}
            </div>
          </div>
        </section>

        {/* Transfer Progress and Logs List */}
        <div className="rounded-xl border bg-[hsl(var(--panel))]">
          <div className="px-5 pt-4 pb-3 flex items-center justify-between border-b bg-[hsl(var(--panel-2)/0.2)]">
            <span className="text-[11px] font-semibold uppercase tracking-widest text-[hsl(var(--muted))]">{t('transfers.listTitle', { count: transferItems.length })}</span>
            {hasClearableTransfers && (
              <Button
                variant="ghost"
                onClick={() => void clearTransfers()}
                className="h-6 px-2 text-[11px] font-medium text-[hsl(var(--muted))] hover:text-[hsl(var(--danger))] hover:bg-[hsl(var(--danger)/0.08)] transition-all flex items-center gap-1.5"
              >
                <Trash2 className="h-3.5 w-3.5" />
                {t('transfers.clearBtn', { defaultValue: 'Clear Completed' })}
              </Button>
            )}
          </div>

          <div className="p-5">
            {transferItems.length === 0 ? (
              <div className="py-20 text-center flex flex-col items-center justify-center gap-2.5">
                <ArrowUpDown className="h-6 w-6 text-[hsl(var(--muted))/0.5]" />
                <div className="text-[13px] text-[hsl(var(--muted))]">{t('transfers.emptyList')}</div>
              </div>
            ) : (
              <div className="divide-y divide-[hsl(var(--border))]">
                {transferItems.map((item) => {
                  const progress = item.fileSize > 0 ? item.transferredBytes / item.fileSize : 0
                  const active = item.status === 'offered' || item.status === 'sending' || item.status === 'receiving'
                  const inFlight = item.status === 'sending' || item.status === 'receiving'
                  const isDone = item.status === 'completed'
                  const isFailed = item.status === 'failed' || item.status === 'cancelled'
                  const statusLabel = t(`transfers.status.${item.status}`, { defaultValue: item.status })
                  const speed = inFlight ? transferSpeeds[item.fileId] : null
                  const routeLabel = item.route === 'lan'
                    ? t('transfers.routeLan', { defaultValue: 'Transferred via LAN' })
                    : item.route === 'cloud'
                      ? t('transfers.routeCloud', { defaultValue: 'Transferred via Cloud relay' })
                      : item.route || '-'

                  return (
                    <div className="py-4 first:pt-0 last:pb-0 flex flex-col sm:flex-row sm:items-center justify-between gap-4" key={item.fileId}>
                      {/* Left side: File icon & details */}
                      <div className="flex items-center gap-3 min-w-0 flex-1">
                        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]">
                          {item.direction === 'outbound' ? (
                            <ArrowUp className="h-4 w-4 text-[hsl(var(--text))]" />
                          ) : (
                            <ArrowDown className="h-4 w-4 text-[hsl(var(--muted))]" />
                          )}
                        </div>
                        <div className="min-w-0 flex-1">
                          <div className="truncate text-[13px] font-medium text-[hsl(var(--text))]" title={item.fileName}>
                            {item.fileName}
                          </div>
                          <div className="mt-1 text-[11px] text-[hsl(var(--muted))] flex items-center gap-1.5">
                            <span className="font-semibold">{item.direction === 'outbound' ? t('transfers.directionSend') : t('transfers.directionReceive')}</span>
                            <span>•</span>
                            <span>{formatBytes(item.fileSize)}</span>
                            <span>•</span>
                            <span>{routeLabel}</span>
                          </div>
                        </div>
                      </div>

                      {/* Middle side: Progress and status */}
                      <div className="flex-1 max-w-xs w-full">
                        <div className="flex items-center justify-between text-[11px] text-[hsl(var(--muted))] mb-1.5">
                          <span className={cn(
                            "font-medium truncate max-w-[160px]",
                            isDone ? "text-[hsl(var(--success))]" : isFailed ? "text-[hsl(var(--danger))]" : "text-[hsl(var(--text-secondary))]"
                          )} title={isFailed ? (item.error || statusLabel) : statusLabel}>
                            {isFailed ? (item.error || statusLabel) : statusLabel}
                          </span>
                          <span className="shrink-0">
                            {formatBytes(item.transferredBytes)} / {formatBytes(item.fileSize)}
                            {speed !== null ? ` • ${formatBytes(Math.max(0, speed))}/s` : ''}
                          </span>
                        </div>
                        {/* Progress Bar */}
                        <div className="h-1 w-full overflow-hidden rounded-full bg-[hsl(var(--border))]">
                          <div
                            className={cn(
                              "h-full rounded-full transition-all duration-300",
                              isFailed ? "bg-[hsl(var(--danger))]" : isDone ? "bg-[hsl(var(--success))]" : "bg-[hsl(var(--text-secondary))]"
                            )}
                            style={{ width: `${Math.max(4, Math.min(100, progress * 100))}%` }}
                          />
                        </div>
                      </div>

                      {/* Right side: Actions & indicators */}
                      <div className="flex h-7 w-7 shrink-0 items-center justify-center">
                        {active ? (
                          <button
                            className="rounded-md p-1.5 text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
                            onClick={() => void cancelTransfer(item.fileId)}
                            type="button"
                            title={t('transfers.cancelTitle')}
                          >
                            <X className="h-3.5 w-3.5" />
                          </button>
                        ) : isDone ? (
                          <CheckCircle2 className="h-4 w-4 text-[hsl(var(--success))]" />
                        ) : isFailed ? (
                          <AlertCircle className="h-4 w-4 text-[hsl(var(--danger))]" />
                        ) : null}
                      </div>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
