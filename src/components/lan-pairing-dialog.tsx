import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { ShieldCheck, ShieldAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { respondLanPairing } from '../lib/api'
import type { LanPairingRequest } from '../lib/types'
import { Button } from './ui/button'

export function LanPairingDialog() {
  const { t } = useTranslation()
  const [request, setRequest] = useState<LanPairingRequest | null>(null)
  const [acting, setActing] = useState(false)

  useEffect(() => {
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        unlisten = await listen<LanPairingRequest>('lan-pairing-requested', (event) => {
          setRequest(event.payload)
          setActing(false)
        })
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      unlisten?.()
    }
  }, [])

  if (!request) {
    return null
  }

  async function respond(accepted: boolean) {
    if (!request) return
    setActing(true)
    try {
      await respondLanPairing(request.requestId, accepted)
      setRequest(null)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
      setActing(false)
    }
  }

  const Icon = request.reason === 'key_changed' ? ShieldAlert : ShieldCheck

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45 backdrop-blur-sm">
      <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
            <Icon className="h-4 w-4 text-[hsl(var(--accent))]" />
          </div>
          <div>
            <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
              {request.reason === 'key_changed'
                ? t('lanPairing.keyChangedTitle')
                : t('lanPairing.title')}
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
          {t('lanPairing.description')}
        </p>

        <div className="mt-6 flex justify-end gap-2">
          <Button disabled={acting} onClick={() => respond(false)} variant="secondary">
            {t('common.cancel')}
          </Button>
          <Button disabled={acting} onClick={() => respond(true)} variant="primary">
            {t('lanPairing.accept')}
          </Button>
        </div>
      </div>
    </div>
  )
}
