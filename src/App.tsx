import { useEffect, useRef, useState } from 'react'
import { RouterProvider } from 'react-router-dom'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { AppStateProvider } from './hooks/use-app-state'
import { useAppState } from './hooks/use-app-state'
import { FileOfferDialog } from './components/file-offer-dialog'
import { LanPairingDialog } from './components/lan-pairing-dialog'
import { UpdateDialog } from './components/update-dialog'
import { checkUpdate } from './lib/api'
import { fallbackVersion, isReleaseBuild, readAppVersion } from './lib/app-meta'
import type { AppUpdateRelease } from './lib/types'
import { isBreakingVersionUpdate } from './lib/update-policy'
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
  const checkedRef = useRef(false)
  const [update, setUpdate] = useState<AppUpdateRelease | null>(null)
  const [version, setVersion] = useState(fallbackVersion)
  const required = update ? isReleaseBuild && update.assets.length > 0 && isBreakingVersionUpdate(update.version, version) : false

  useEffect(() => {
    if (!isReleaseBuild || status !== 'ready' || checkedRef.current) {
      return
    }
    checkedRef.current = true
    let disposed = false

    void (async () => {
      try {
        const version = await readAppVersion()
        const update = await checkUpdate()
        if (disposed || !update) {
          return
        }
        setVersion(version)
        setUpdate(update)
      } catch {
        // Update check is optional.
      }
    })()

    return () => {
      disposed = true
    }
  }, [status])

  return (
    <UpdateDialog
      update={update}
      required={required}
      onClose={() => {
        if (!required) {
          setUpdate(null)
        }
      }}
    />
  )
}
