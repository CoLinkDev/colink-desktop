import { useEffect, useMemo, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { useAppState, readErrorMessage } from '../hooks/use-app-state'
import { getPendingShareFiles, sendFiles } from '../lib/api'
import type { SystemShareFile } from '../lib/types'
import { cn, formatBytes } from '../lib/utils'
import { Button } from './ui/button'

function mergeFiles(current: SystemShareFile[], incoming: SystemShareFile[]) {
  const seen = new Set(current.map((file) => file.path.toLowerCase()))
  return [...current, ...incoming.filter((file) => {
    const key = file.path.toLowerCase()
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })]
}

export function DestinationDeviceDialog() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { device, devices } = useAppState()
  const [files, setFiles] = useState<SystemShareFile[]>([])
  const [sendingTo, setSendingTo] = useState<string | null>(null)

  const onlineDevices = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId && item.online),
    [device?.deviceId, devices],
  )

  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []

    const addFiles = (incoming: SystemShareFile[]) => {
      if (!disposed && incoming.length > 0) {
        setFiles((current) => mergeFiles(current, incoming))
      }
    }

    const drainPendingFiles = async () => {
      try {
        addFiles(await getPendingShareFiles())
      } catch {
        // Desktop runtime only.
      }
    }

    void (async () => {
      try {
        unlisteners.push(await listen('system-share-files', () => {
          void drainPendingFiles()
        }))
        await drainPendingFiles()
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      disposed = true
      unlisteners.forEach((unlisten) => unlisten())
    }
  }, [])

  if (files.length === 0) return null

  async function handleSend(deviceId: string) {
    setSendingTo(deviceId)
    try {
      await sendFiles({ deviceId, paths: files.map((file) => file.path) })
      setFiles([])
      navigate(`/transfers?deviceId=${encodeURIComponent(deviceId)}`)
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setSendingTo(null)
    }
  }

  return (
    <div className="fixed inset-0 z-[75] flex items-center justify-center bg-black/45 p-4 backdrop-blur-sm">
      <div className="w-full max-w-lg rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl">
        <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('systemShare.title')}</div>
        <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">
          {t('systemShare.fileCount', { count: files.length })}
        </div>

        <div className="mt-5 max-h-48 space-y-1.5 overflow-y-auto rounded-lg border bg-[hsl(var(--panel-2))] p-2 scrollbar-thin">
          {files.map((file) => (
            <div className="flex items-center justify-between gap-3 rounded-md px-2.5 py-2" key={file.path}>
              <span className="min-w-0 truncate text-[13px] text-[hsl(var(--text))]" title={file.path}>{file.name}</span>
              <span className="shrink-0 text-[11px] text-[hsl(var(--muted))]">{formatBytes(file.size)}</span>
            </div>
          ))}
        </div>

        <div className="mt-5 text-[12px] font-medium text-[hsl(var(--text-secondary))]">{t('systemShare.selectDevice')}</div>
        <div className="mt-2 grid gap-2">
          {onlineDevices.length === 0 ? (
            <div className="rounded-lg border border-dashed px-3 py-4 text-center text-[12px] text-[hsl(var(--muted))]">
              {t('systemShare.noDevices')}
            </div>
          ) : onlineDevices.map((item) => (
            <button
              className={cn(
                'flex items-center justify-between gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors',
                sendingTo ? 'cursor-wait opacity-60' : 'hover:bg-[hsl(var(--panel-2))]',
              )}
              disabled={sendingTo !== null}
              key={item.deviceId}
              onClick={() => void handleSend(item.deviceId)}
              type="button"
            >
              <span className="min-w-0 truncate text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
              <span className="shrink-0 text-[11px] text-[hsl(var(--success))]">{t('devices.online')}</span>
            </button>
          ))}
        </div>

        <div className="mt-6 flex justify-end">
          <Button disabled={sendingTo !== null} onClick={() => setFiles([])} variant="secondary">
            {t('common.cancel')}
          </Button>
        </div>
      </div>
    </div>
  )
}
