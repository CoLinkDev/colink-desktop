import { useEffect, useRef } from 'react'
import { RouterProvider } from 'react-router-dom'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { AppStateProvider } from './hooks/use-app-state'
import { useAppState } from './hooks/use-app-state'
import { FileOfferDialog } from './components/file-offer-dialog'
import { LanPairingDialog } from './components/lan-pairing-dialog'
import { checkUpdate, openUpdateDownload } from './lib/api'
import { router } from './router'

interface LanKeyChangedPayload {
  deviceId: string
  name: string
}

export default function App() {
  const { t } = useTranslation()

  useEffect(() => {
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        unlisten = await listen<LanKeyChangedPayload>('lan-key-changed', (event) => {
          const name = event.payload.name || event.payload.deviceId
          toast.warning(
            t('lanPairing.keyChangedToast', {
              name,
              defaultValue: 'Device {{name}} key changed. Pair again to use LAN.',
            }),
          )
        })
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      unlisten?.()
    }
  }, [t])

  return (
    <AppStateProvider>
      <UpdateNotification />
      <RouterProvider router={router} />
      <FileOfferDialog />
      <LanPairingDialog />
    </AppStateProvider>
  )
}

function UpdateNotification() {
  const { status } = useAppState()
  const { t } = useTranslation()
  const checkedRef = useRef(false)

  useEffect(() => {
    if (status !== 'ready' || checkedRef.current) {
      return
    }
    checkedRef.current = true
    let disposed = false

    void (async () => {
      try {
        const update = await checkUpdate()
        if (disposed || !update) {
          return
        }
        const asset = update.assets[0]
        const notes = update.releaseNotes.trim()
        const description =
          notes.length > 240 ? `${notes.slice(0, 240)}...` : notes || t('updates.description')

        toast.info(t('updates.available', { version: update.version }), {
          description,
          duration: Infinity,
          action: asset
            ? {
                label: t('updates.download'),
                onClick: () => {
                  void openUpdateDownload(asset.downloadUrl).catch(() => {
                    toast.error(t('common.requestFailed'))
                  })
                },
              }
            : undefined,
        })
      } catch {
        // Update check is optional.
      }
    })()

    return () => {
      disposed = true
    }
  }, [status, t])

  return null
}
