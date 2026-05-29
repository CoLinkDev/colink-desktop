import { RouterProvider } from 'react-router-dom'

import { AppStateProvider } from './hooks/use-app-state'
import { LanPairingDialog } from './components/lan-pairing-dialog'
import { router } from './router'

export default function App() {
  return (
    <AppStateProvider>
      <RouterProvider router={router} />
      <LanPairingDialog />
    </AppStateProvider>
  )
}
