import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
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

  const formatFailure = useCallback((reason: string, message: string) => {
    switch (reason) {
      case 'colink:pairing.cancelled.v1':
        return t('lanPairing.cancelled')
      case 'colink:pairing.user_rejected.v1':
        return t('lanPairing.rejected')
      case 'colink:pairing.timeout.v1':
        return t('lanPairing.timedOut')
      case 'colink:pairing.connection_closed.v1':
        return t('lanPairing.connectionClosed')
      default:
        return message || reason || t('lanPairing.failed')
    }
  }, [t])

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
          if (event.payload.reason === 'colink:pairing.connection_closed.v1') {
            setCurrentRequest(null)
            setActing(false)
            setWaiting(false)
            return
          }
          setCurrentRequest({
            ...requestRef.current,
            error: formatFailure(event.payload.reason, event.payload.message),
          })
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
  }, [formatFailure, setCurrentRequest])

  if (!request) {
    return null
  }

  async function respond(accepted: boolean) {
    if (!request) return
    setActing(true)
    try {
      await respondLanPairing(request.requestId, accepted)
      if (accepted && !request.initiatedLocally) {
        setWaiting(true)
      } else {
        setCurrentRequest({
          ...request,
          error: accepted
            ? t('lanPairing.failed')
            : request.initiatedLocally
              ? t('lanPairing.cancelled')
              : t('lanPairing.rejected'),
        })
      }
    } catch (error) {
      setCurrentRequest({
        ...request,
        error: error instanceof Error ? error.message : String(error),
      })
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
          {request.error || (waiting ? t('lanPairing.waiting') : t('lanPairing.description'))}
        </p>

        <div className="mt-6 flex justify-end gap-2">
          {request.error ? (
            <Button onClick={() => setCurrentRequest(null)} variant="primary">
              {t('common.close')}
            </Button>
          ) : (
            <>
              <Button disabled={acting || waiting} onClick={() => respond(false)} variant="secondary">
                {request.initiatedLocally ? t('common.cancel') : t('lanPairing.reject')}
              </Button>
              {!request.initiatedLocally && (
                <Button disabled={acting || waiting} onClick={() => respond(true)} variant="primary">
                  {waiting ? t('common.loading') : t('lanPairing.accept')}
                </Button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  )
}
