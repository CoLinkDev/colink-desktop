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
import { CastBoardPage } from './pages/castboard-page'
import { FilesPage } from './pages/files-page'
import { TerminalPage } from './pages/terminal-page'
import { CameraPage } from './pages/camera-page'

function RootRedirect() {
  const { status } = useAppState()
  const { t } = useTranslation()

  if (status === 'booting') {
    return <LoadingScreen label={t('common.loading')} />
  }

  return <Navigate replace to="/devices" />
}

function ProtectedShell() {
  const { status } = useAppState()
  const { t } = useTranslation()

  if (status === 'booting') {
    return <LoadingScreen label={t('common.loading')} />
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
        path: '/files',
        element: <FilesPage />,
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
      {
        path: '/terminal',
        element: <TerminalPage />,
      },
      { path: '/camera', element: <CameraPage /> },
    ],
  },
])
