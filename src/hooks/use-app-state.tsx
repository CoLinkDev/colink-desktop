import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type PropsWithChildren,
} from 'react'

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
  defaultSettings,
  type AppSettings,
  type BootstrapPayload,
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

  const applyBootstrap = useCallback((payload: BootstrapPayload) => {
    setSession(payload.session)
    setSettings(payload.settings)
    setDevice(payload.device)
    setDevices(payload.devices)
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
