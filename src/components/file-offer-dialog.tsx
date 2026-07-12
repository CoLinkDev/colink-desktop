import { useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Download, FileArchive } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { pendingFileOffers, respondFileOffer } from '../lib/api'
import { useAppState } from '../hooks/use-app-state'
import type { FileOfferRequest } from '../lib/types'
import { Button } from './ui/button'

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let size = value
  let unitIndex = 0
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024
    unitIndex += 1
  }
  return `${size >= 10 || unitIndex === 0 ? size.toFixed(0) : size.toFixed(1)} ${units[unitIndex]}`
}

export function FileOfferDialog() {
  const { t } = useTranslation()
  const { settings, pickDownloadDirectory } = useAppState()
  const [requests, setRequests] = useState<FileOfferRequest[]>([])
  const [acting, setActing] = useState(false)
  const [destinationPath, setDestinationPath] = useState('')
  const request = requests[0] ?? null
  const currentSessionIdRef = useRef<string | null>(null)
  currentSessionIdRef.current = request?.sessionId ?? null

  useEffect(() => {
    setDestinationPath(settings.downloadPath)
  }, [request?.sessionId, settings.downloadPath])

  useEffect(() => {
    const unlisteners: Array<() => void> = []

    void (async () => {
      try {
        unlisteners.push(await listen<FileOfferRequest>('file-offer-requested', (event) => {
          setRequests((current) => {
            if (current.some((item) => item.sessionId === event.payload.sessionId)) {
              return current
            }
            return [...current, event.payload]
          })
        }))
        unlisteners.push(await listen<string>('file-offer-ended', (event) => {
          setRequests((current) => current.filter((item) => item.sessionId !== event.payload))
          if (currentSessionIdRef.current === event.payload) {
            setActing(false)
          }
        }))
        const pending = await pendingFileOffers()
        setRequests((current) => {
          const existing = new Set(current.map((item) => item.sessionId))
          return [...current, ...pending.filter((item) => !existing.has(item.sessionId))]
        })
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      for (const unlisten of unlisteners) {
        unlisten()
      }
    }
  }, [])

  if (!request) {
    return null
  }

  async function respond(accepted: boolean) {
    if (!request) return
    setActing(true)
    try {
      await respondFileOffer(request.sessionId, accepted, accepted ? destinationPath : undefined)
      setRequests((current) => current.filter((item) => item.sessionId !== request.sessionId))
      setActing(false)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
      setActing(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45 backdrop-blur-sm">
      <div className="w-full max-w-md rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
            <Download className="h-4 w-4 text-[hsl(var(--accent))]" />
          </div>
          <div className="min-w-0">
            <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
              {t('fileOffers.title')}
            </div>
            <div className="mt-0.5 truncate text-[12px] text-[hsl(var(--muted))]">
              {request.deviceName || request.deviceId}
            </div>
          </div>
        </div>

        <div className="mt-5 rounded-lg border bg-[hsl(var(--panel-2))] px-4 py-4">
          <div className="flex items-start gap-3">
            <FileArchive className="mt-0.5 h-5 w-5 shrink-0 text-[hsl(var(--muted))]" />
            <div className="min-w-0">
              <div className="break-words text-[14px] font-medium text-[hsl(var(--text))]">
                {request.fileName}
              </div>
              <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">
                {formatBytes(request.fileSize)}
              </div>
            </div>
          </div>
        </div>

        <p className="mt-4 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          {t('fileOffers.description', { name: request.deviceName || request.deviceId })}
        </p>

        <div className="mt-4">
          <div className="text-[12px] font-medium text-[hsl(var(--text-secondary))]">{t('fileOffers.destination')}</div>
          <div className="mt-1.5 flex gap-2">
            <div className="min-w-0 flex-1 truncate rounded-md border bg-[hsl(var(--panel-2))] px-3 py-2 text-[12px] text-[hsl(var(--text))]" title={destinationPath}>
              {destinationPath}
            </div>
            <Button
              disabled={acting}
              onClick={async () => {
                const path = await pickDownloadDirectory()
                if (path) setDestinationPath(path)
              }}
              type="button"
              variant="secondary"
            >
              {t('fileOffers.changeDestination')}
            </Button>
          </div>
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <Button disabled={acting} onClick={() => respond(false)} variant="secondary">
            {t('common.cancel')}
          </Button>
          <Button disabled={acting || !destinationPath.trim()} onClick={() => respond(true)} variant="primary">
            {acting ? t('common.loading') : t('fileOffers.accept')}
          </Button>
        </div>
      </div>
    </div>
  )
}
