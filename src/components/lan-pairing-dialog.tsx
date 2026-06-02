import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { ShieldCheck } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { respondLanPairing } from '../lib/api'
import type { LanPairingCompleted, LanPairingFailed, LanPairingRequest } from '../lib/types'
import { Button } from './ui/button'

export function LanPairingDialog() {
  const { t } = useTranslation()
  const [request, setRequest] = useState<LanPairingRequest | null>(null)
  const [acting, setActing] = useState(false)
  const [waiting, setWaiting] = useState(false)
  const requestRef = useRef<LanPairingRequest | null>(null)

  const setCurrentRequest = useCallback((next: LanPairingRequest | null) => {
    requestRef.current = next
    setRequest(next)
  }, [])

  useEffect(() => {
    const unlisteners: Array<() => void> = []

    void (async () => {
      try {
        unlisteners.push(await listen<LanPairingRequest>('lan-pairing-requested', (event) => {
          setCurrentRequest(event.payload)
          setActing(false)
          setWaiting(false)
        }))
        unlisteners.push(await listen<LanPairingCompleted>('lan-pairing-completed', (event) => {
          if (requestRef.current?.requestId !== event.payload.requestId) {
            return
          }
          setCurrentRequest(null)
          setActing(false)
          setWaiting(false)
        }))
        unlisteners.push(await listen<LanPairingFailed>('lan-pairing-failed', (event) => {
          if (requestRef.current?.requestId !== event.payload.requestId) {
            return
          }
          toast.error(event.payload.reason || t('lanPairing.failed'))
          setCurrentRequest(null)
          setActing(false)
          setWaiting(false)
        }))
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      for (const unlisten of unlisteners) {
        unlisten()
      }
    }
  }, [setCurrentRequest, t])

  if (!request) {
    return null
  }

  async function respond(accepted: boolean) {
    if (!request) return
    setActing(true)
    try {
      await respondLanPairing(request.requestId, accepted)
      if (accepted) {
        setWaiting(true)
      } else {
        setCurrentRequest(null)
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
      setActing(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45 backdrop-blur-sm">
      <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
            <ShieldCheck className="h-4 w-4 text-[hsl(var(--accent))]" />
          </div>
          <div>
            <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
              {t('lanPairing.title')}
            </div>
            <div className="mt-0.5 text-[12px] text-[hsl(var(--muted))]">
              {request.name || request.deviceId}
            </div>
          </div>
        </div>

        <div className="mt-5 rounded-lg border bg-[hsl(var(--panel-2))] px-4 py-4 text-center">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">
            {t('lanPairing.code')}
          </div>
          <div className="mt-2 font-mono text-[32px] font-semibold tracking-[0.18em] text-[hsl(var(--text))]">
            {request.code}
          </div>
        </div>

        <p className="mt-4 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          {waiting ? t('lanPairing.waiting') : t('lanPairing.description')}
        </p>

        <div className="mt-6 flex justify-end gap-2">
          <Button disabled={acting || waiting} onClick={() => respond(false)} variant="secondary">
            {t('common.cancel')}
          </Button>
          <Button disabled={acting || waiting} onClick={() => respond(true)} variant="primary">
            {waiting ? t('common.loading') : t('lanPairing.accept')}
          </Button>
        </div>
      </div>
    </div>
  )
}
