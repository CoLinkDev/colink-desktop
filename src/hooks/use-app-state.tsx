import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type PropsWithChildren,
} from 'react'
import { listen } from '@tauri-apps/api/event'

import {
  bootstrapApp,
  getSettings,
  listDevices,
  login as loginRequest,
  logout as logoutRequest,
  registerAccount,
  updateSettings as updateSettingsRequest,
} from '../lib/api'
import {
  defaultCloudStatus,
  defaultSettings,
  type AppSettings,
  type BootstrapPayload,
  type CloudStatus,
  type DeviceInfo,
  type LocalDeviceSummary,
  type LoginPayload,
  type RegisterPayload,
  type SessionSummary,
} from '../lib/types'

type AppStatus = 'booting' | 'ready'

interface AppStateValue {
  status: AppStatus
  bootstrapError: string | null
  session: SessionSummary | null
  settings: AppSettings
  device: LocalDeviceSummary | null
  devices: DeviceInfo[]
  cloud: CloudStatus
  refreshBootstrap: () => Promise<void>
  login: (payload: LoginPayload) => Promise<void>
  register: (payload: RegisterPayload) => Promise<void>
  logout: () => Promise<void>
  refreshDevices: () => Promise<void>
  saveSettings: (settings: AppSettings) => Promise<void>
}

const AppStateContext = createContext<AppStateValue | null>(null)

export function readErrorMessage(error: unknown) {
  if (typeof error === 'string') {
    return error
  }

  if (error instanceof Error) {
    return error.message
  }

  return '请求失败'
}

export function AppStateProvider({ children }: PropsWithChildren) {
  const [status, setStatus] = useState<AppStatus>('booting')
  const [bootstrapError, setBootstrapError] = useState<string | null>(null)
  const [session, setSession] = useState<SessionSummary | null>(null)
  const [settings, setSettings] = useState<AppSettings>(defaultSettings)
  const [device, setDevice] = useState<LocalDeviceSummary | null>(null)
  const [devices, setDevices] = useState<DeviceInfo[]>([])
  const [cloud, setCloud] = useState<CloudStatus>(defaultCloudStatus)

  const applyBootstrap = useCallback((payload: BootstrapPayload) => {
    setSession(payload.session)
    setSettings(payload.settings)
    setDevice(payload.device)
    setDevices(payload.devices)
    setCloud(payload.cloud)
  }, [])

  const refreshBootstrap = useCallback(async () => {
    setStatus('booting')
    setBootstrapError(null)

    try {
      const payload = await bootstrapApp()
      applyBootstrap(payload)
    } catch (error) {
      setBootstrapError(readErrorMessage(error))

      try {
        const nextSettings = await getSettings()
        setSettings(nextSettings)
      } catch {
        setSettings(defaultSettings)
      }

      setSession(null)
      setDevice(null)
      setDevices([])
      setCloud(defaultCloudStatus)
    } finally {
      setStatus('ready')
    }
  }, [applyBootstrap])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void refreshBootstrap()
    }, 0)

    return () => window.clearTimeout(timer)
  }, [refreshBootstrap])

  useEffect(() => {
    let disposed = false
    let unlistenCloud: (() => void) | null = null
    let unlistenDevices: (() => void) | null = null
    let unlistenAuth: (() => void) | null = null

    void (async () => {
      try {
        unlistenCloud = await listen<CloudStatus>('cloud-status', (event) => {
          if (!disposed) {
            setCloud(event.payload)
          }
        })

        unlistenDevices = await listen<DeviceInfo[]>('devices-updated', (event) => {
          if (!disposed) {
            setDevices(event.payload)
          }
        })

        unlistenAuth = await listen('auth-invalidated', () => {
          if (disposed) {
            return
          }

          setBootstrapError('登录状态失效，请重新登录')
          void refreshBootstrap()
        })
      } catch {
        // Ignore browser-mode event failures. The desktop runtime provides these events.
      }
    })()

    return () => {
      disposed = true
      unlistenCloud?.()
      unlistenDevices?.()
      unlistenAuth?.()
    }
  }, [refreshBootstrap])

  const login = useCallback(
    async (payload: LoginPayload) => {
      const result = await loginRequest(payload)
      applyBootstrap(result)
      setBootstrapError(null)
    },
    [applyBootstrap],
  )

  const register = useCallback(
    async (payload: RegisterPayload) => {
      const result = await registerAccount(payload)
      applyBootstrap(result)
      setBootstrapError(null)
    },
    [applyBootstrap],
  )

  const logout = useCallback(async () => {
    await logoutRequest()
    setSession(null)
    setDevices([])
    setCloud(defaultCloudStatus)
    setBootstrapError(null)
  }, [])

  const refreshDevices = useCallback(async () => {
    const nextDevices = await listDevices()
    setDevices(nextDevices)
  }, [])

  const saveSettings = useCallback(async (nextSettings: AppSettings) => {
    const saved = await updateSettingsRequest(nextSettings)
    setSettings(saved)
    setBootstrapError(null)
  }, [])

  const value = useMemo<AppStateValue>(
    () => ({
      status,
      bootstrapError,
      session,
      settings,
      device,
      devices,
      cloud,
      refreshBootstrap,
      login,
      register,
      logout,
      refreshDevices,
      saveSettings,
    }),
    [
      status,
      bootstrapError,
      session,
      settings,
      device,
      devices,
      cloud,
      refreshBootstrap,
      login,
      register,
      logout,
      refreshDevices,
      saveSettings,
    ],
  )

  return (
    <AppStateContext.Provider value={value}>{children}</AppStateContext.Provider>
  )
}

export function useAppState() {
  const context = useContext(AppStateContext)

  if (!context) {
    throw new Error('useAppState must be used inside AppStateProvider')
  }

  return context
}
