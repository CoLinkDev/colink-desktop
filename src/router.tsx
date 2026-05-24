import { createHashRouter, Navigate, Outlet } from 'react-router-dom'

import { AppLayout } from './components/app-layout'
import { LoadingScreen } from './components/loading-screen'
import { useAppState } from './hooks/use-app-state'
import { AuthPage } from './pages/auth-page'
import { DevicesPage } from './pages/devices-page'
import { SettingsPage } from './pages/settings-page'

function RootRedirect() {
  const { session, status } = useAppState()

  if (status === 'booting') {
    return <LoadingScreen label="正在加载本地状态" />
  }

  return <Navigate replace to={session ? '/devices' : '/login'} />
}

function ProtectedShell() {
  const { session, status } = useAppState()

  if (status === 'booting') {
    return <LoadingScreen label="正在准备桌面端" />
  }

  if (!session) {
    return <Navigate replace to="/login" />
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
    path: '/login',
    element: <AuthPage />,
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
        path: '/settings',
        element: <SettingsPage />,
      },
    ],
  },
])
