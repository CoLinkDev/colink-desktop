import { createHashRouter, Navigate, Outlet, useLocation } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { AppLayout } from './components/app-layout'
import { LoadingScreen } from './components/loading-screen'
import { useAppState } from './hooks/use-app-state'
import { DevicesPage } from './pages/devices-page'
import { TransfersPage } from './pages/transfers-page'
import { SettingsPage } from './pages/settings-page'
import { CastBoardPage } from './pages/castboard-page'
import { FilesPage } from './pages/files-page'
import { TerminalPage } from './pages/terminal-page'
import { CameraPage } from './pages/camera-page'
import { NotesPage } from './pages/notes-page'

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

function MessagesRedirect() {
  const location = useLocation()
  return <Navigate replace to={{ pathname: '/transfers', search: location.search }} />
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
        element: <MessagesRedirect />,
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
        path: '/notes',
        element: <NotesPage />,
      },
      {
        path: '/settings',
        element: <SettingsPage />,
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
