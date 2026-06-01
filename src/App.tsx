import { useEffect } from 'react'
import { RouterProvider } from 'react-router-dom'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { AppStateProvider } from './hooks/use-app-state'
import { LanPairingDialog } from './components/lan-pairing-dialog'
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
      <RouterProvider router={router} />
      <LanPairingDialog />
    </AppStateProvider>
  )
}
