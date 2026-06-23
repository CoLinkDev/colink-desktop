import { createHashRouter, Navigate, Outlet } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { AppLayout } from './components/app-layout'
import { LoadingScreen } from './components/loading-screen'
import { useAppState } from './hooks/use-app-state'
import { DevicesPage } from './pages/devices-page'
import { MessagesPage } from './pages/messages-page'
import { TransfersPage } from './pages/transfers-page'
import { SettingsPage } from './pages/settings-page'
import { ClipboardPage } from './pages/clipboard-page'
import { LogsPage } from './pages/logs-page'
import { CastBoardPage } from './pages/castboard-page'

function RootRedirect() {
  const { status } = useAppState()
  const { t } = useTranslation()

  if (status === 'booting') {
    return <LoadingScreen label={t('logs.loadingState')} />
  }

  return <Navigate replace to="/devices" />
}

function ProtectedShell() {
  const { status } = useAppState()
  const { t } = useTranslation()

  if (status === 'booting') {
    return <LoadingScreen label={t('logs.preparingState')} />
  }

  return (
    <AppLayout>
      <Outlet />
    </AppLayout>
  )
}

export const router = createHashRouter([
  {
    path: '/',
    element: <RootRedirect />,
  },
  {
    path: '/',
    element: <ProtectedShell />,
    children: [
      {
        path: '/devices',
        element: <DevicesPage />,
      },
      {
        path: '/messages',
        element: <MessagesPage />,
      },
      {
        path: '/transfers',
        element: <TransfersPage />,
      },
      {
        path: '/logs',
        element: <LogsPage />,
      },
      {
        path: '/settings',
        element: <SettingsPage />,
      },
      {
        path: '/clipboard',
        element: <ClipboardPage />,
      },
      {
        path: '/castboard',
        element: <CastBoardPage />,
      },
    ],
  },
])
