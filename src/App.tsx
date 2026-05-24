import { RouterProvider } from 'react-router-dom'

import { AppStateProvider } from './hooks/use-app-state'
import { router } from './router'

export default function App() {
  return (
    <AppStateProvider>
      <RouterProvider router={router} />
    </AppStateProvider>
  )
}
