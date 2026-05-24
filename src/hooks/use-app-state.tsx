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
  cancelTransfer as cancelTransferRequest,
  deleteDevice as deleteDeviceRequest,
  getSettings,
  listDevices,
  login as loginRequest,
  logout as logoutRequest,
  pickDownloadDirectory as pickDownloadDirectoryRequest,
  pickFiles as pickFilesRequest,
  registerAccount,
  rotateDeviceKey as rotateDeviceKeyRequest,
  sendFiles as sendFilesRequest,
  sendText as sendTextRequest,
  updateDeviceName as updateDeviceNameRequest,
  updateSettings as updateSettingsRequest,
} from '../lib/api'
import {
  type AppLogEntry,
  defaultCloudStatus,
  defaultSettings,
  type AppSettings,
  type BootstrapPayload,
  type CloudStatus,
  type DeviceInfo,
  type FileTransferRecord,
  type LocalDeviceSummary,
  type LoginPayload,
  type RegisterPayload,
  type SendFilePayload,
  type SendTextPayload,
  type SessionSummary,
  type TextMessageRecord,
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
  messages: TextMessageRecord[]
  transfers: FileTransferRecord[]
  logs: AppLogEntry[]
  refreshBootstrap: () => Promise<void>
  login: (payload: LoginPayload) => Promise<void>
  register: (payload: RegisterPayload) => Promise<void>
  logout: () => Promise<void>
  refreshDevices: () => Promise<void>
  updateDeviceName: (deviceId: string, name: string) => Promise<void>
  deleteDevice: (deviceId: string) => Promise<void>
  rotateDeviceKey: (deviceId: string) => Promise<void>
  saveSettings: (settings: AppSettings) => Promise<void>
  pickDownloadDirectory: () => Promise<string | null>
  sendText: (payload: SendTextPayload) => Promise<void>
  pickFiles: (multiple?: boolean) => Promise<string[]>
  sendFiles: (payload: SendFilePayload) => Promise<void>
  cancelTransfer: (fileId: string) => Promise<void>
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
  const [messages, setMessages] = useState<TextMessageRecord[]>([])
  const [transfers, setTransfers] = useState<FileTransferRecord[]>([])
  const [logs, setLogs] = useState<AppLogEntry[]>([])

  const applyBootstrap = useCallback((payload: BootstrapPayload) => {
    setSession(payload.session)
    setSettings(payload.settings)
    setDevice(payload.device)
    setDevices(payload.devices)
    setCloud(payload.cloud)
    setMessages(payload.messages)
    setTransfers(payload.transfers)
    setLogs(payload.logs)
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
      setMessages([])
      setTransfers([])
      setLogs([])
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
    let unlistenMessages: (() => void) | null = null
    let unlistenTransfers: (() => void) | null = null
    let unlistenLogs: (() => void) | null = null

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

        unlistenMessages = await listen<TextMessageRecord[]>('messages-updated', (event) => {
          if (!disposed) {
            setMessages(event.payload)
          }
        })

        unlistenTransfers = await listen<FileTransferRecord[]>('transfers-updated', (event) => {
          if (!disposed) {
            setTransfers(event.payload)
          }
        })

        unlistenLogs = await listen<AppLogEntry[]>('logs-updated', (event) => {
          if (!disposed) {
            setLogs(event.payload)
          }
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
      unlistenMessages?.()
      unlistenTransfers?.()
      unlistenLogs?.()
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
    setMessages([])
    setTransfers([])
    setLogs([])
    setBootstrapError(null)
  }, [])

  const refreshDevices = useCallback(async () => {
    const nextDevices = await listDevices()
    setDevices(nextDevices)
  }, [])

  const updateDeviceName = useCallback(async (deviceId: string, name: string) => {
    const nextDevices = await updateDeviceNameRequest(deviceId, name)
    setDevices(nextDevices)
  }, [])

  const deleteDevice = useCallback(async (deviceId: string) => {
    const nextDevices = await deleteDeviceRequest(deviceId)
    setDevices(nextDevices)
  }, [])

  const rotateDeviceKey = useCallback(async (deviceId: string) => {
    const nextDevices = await rotateDeviceKeyRequest(deviceId)
    setDevices(nextDevices)
  }, [])

  const saveSettings = useCallback(async (nextSettings: AppSettings) => {
    const saved = await updateSettingsRequest(nextSettings)
    setSettings(saved)
    setBootstrapError(null)
  }, [])

  const pickDownloadDirectory = useCallback(async () => {
    return pickDownloadDirectoryRequest()
  }, [])

  const sendText = useCallback(async (payload: SendTextPayload) => {
    await sendTextRequest(payload)
  }, [])

  const pickFiles = useCallback(async (multiple = true) => {
    return pickFilesRequest(multiple)
  }, [])

  const sendFiles = useCallback(async (payload: SendFilePayload) => {
    await sendFilesRequest(payload)
  }, [])

  const cancelTransfer = useCallback(async (fileId: string) => {
    await cancelTransferRequest(fileId)
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
      messages,
      transfers,
      logs,
      refreshBootstrap,
      login,
      register,
      logout,
      refreshDevices,
      updateDeviceName,
      deleteDevice,
      rotateDeviceKey,
      saveSettings,
      pickDownloadDirectory,
      sendText,
      pickFiles,
      sendFiles,
      cancelTransfer,
    }),
    [
      status,
      bootstrapError,
      session,
      settings,
      device,
      devices,
      cloud,
      messages,
      transfers,
      logs,
      refreshBootstrap,
      login,
      register,
      logout,
      refreshDevices,
      updateDeviceName,
      deleteDevice,
      rotateDeviceKey,
      saveSettings,
      pickDownloadDirectory,
      sendText,
      pickFiles,
      sendFiles,
      cancelTransfer,
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
