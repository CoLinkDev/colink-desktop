import { ArrowUpDown, HardDriveUpload, X, CheckCircle2, AlertCircle, ArrowUp, ArrowDown } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { cn, formatBytes, formatPlatformName } from '../lib/utils'

export function TransfersPage() {
  const { t } = useTranslation()
  const [searchParams, setSearchParams] = useSearchParams()
  const { device, devices, transfers, pickFiles, sendFiles, cancelTransfer } = useAppState()
  const [selectedDeviceId, setSelectedDeviceId] = useState(searchParams.get('deviceId') ?? '')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const targetDevices = useMemo(() => devices.filter((i) => i.deviceId !== device?.deviceId), [device?.deviceId, devices])

  useEffect(() => {
    if (selectedDeviceId && targetDevices.some((i) => i.deviceId === selectedDeviceId)) return
    setSelectedDeviceId(searchParams.get('deviceId') ?? targetDevices[0]?.deviceId ?? '')
  }, [searchParams, selectedDeviceId, targetDevices])

  const selectedDevice = targetDevices.find((i) => i.deviceId === selectedDeviceId) ?? null
  const transferItems = useMemo(() => transfers.filter((i) => selectedDeviceId ? i.deviceId === selectedDeviceId : true), [selectedDeviceId, transfers])

  async function handleSendFiles() {
    if (!selectedDeviceId) { setError(t('transfers.errorSelectDevice')); return }
    setSubmitting(true); setError(null)
    try {
      const paths = await pickFiles(true)
      if (paths.length === 0) return
      await sendFiles({ deviceId: selectedDeviceId, paths })
    } catch (e) { setError(readErrorMessage(e)) }
    finally { setSubmitting(false) }
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
        {/* File Drop/Picker Card */}
        <section className="rounded-xl border bg-[hsl(var(--panel))] p-6 text-center">
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
              {t('transfers.selectBtn')}
            </Button>
            {error && (
              <div className="mt-2 text-[12px] text-[hsl(var(--danger))]">
                {error}
              </div>
            )}
          </div>
        </section>

        {/* Transfer Progress and Logs List */}
        <div className="rounded-xl border bg-[hsl(var(--panel))]">
          <div className="px-5 pt-4 pb-3 flex items-center justify-between border-b bg-[hsl(var(--panel-2)/0.2)]">
            <span className="text-[11px] font-semibold uppercase tracking-widest text-[hsl(var(--muted))]">{t('transfers.listTitle', { count: transferItems.length })}</span>
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
                  const isDone = item.status === 'completed'
                  const isFailed = item.status === 'failed' || item.status === 'cancelled'
                  const statusLabel = t(`transfers.status.${item.status}`, { defaultValue: item.status })

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
                          <span className="shrink-0">{formatBytes(item.transferredBytes)} / {formatBytes(item.fileSize)}</span>
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
