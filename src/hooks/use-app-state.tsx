import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type PropsWithChildren,
  type ReactNode,
} from 'react'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import i18n, { resolveLanguage } from '../i18n'

import {
  bootstrapApp,
  cancelTransfer as cancelTransferRequest,
  clearTransfers as clearTransfersRequest,
  deleteDevice as deleteDeviceRequest,
  getSettings,
  refreshDevices as refreshDevicesRequest,
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
  type TransferProgressPayload,
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
  transferSpeeds: Record<string, number>
  theme: 'light' | 'dark' | 'auto'
  setTheme: (theme: 'light' | 'dark' | 'auto') => void
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
  clearTransfers: () => Promise<void>
  settingsDirty: boolean
  setSettingsDirty: (dirty: boolean) => void
  terminalSessionActive: boolean
  setTerminalSessionActive: (active: boolean) => void
  headerActions: ReactNode
  setHeaderActions: (actions: ReactNode) => void
}

const AppStateContext = createContext<AppStateValue | null>(null)

function isTransferInFlight(record: FileTransferRecord) {
  return record.status === 'sending' || record.status === 'receiving'
}

function mergeTransferRecord(current: FileTransferRecord[], nextRecord: FileTransferRecord) {
  const next = [...current]
  const index = next.findIndex((item) => item.fileId === nextRecord.fileId)

  if (index >= 0) {
    next[index] = nextRecord
  } else {
    next.push(nextRecord)
  }

  next.sort((left, right) => right.updatedAt - left.updatedAt)
  return next
}

function pruneTransferSpeeds(current: Record<string, number>, transfers: FileTransferRecord[]) {
  const activeIds = new Set(
    transfers.filter((record) => isTransferInFlight(record)).map((record) => record.fileId),
  )
  const next: Record<string, number> = {}

  for (const [fileId, speed] of Object.entries(current)) {
    if (activeIds.has(fileId)) {
      next[fileId] = speed
    }
  }

  return next
}

export function readErrorMessage(error: unknown, fallback = i18n.t('common.requestFailed')) {
  if (typeof error === 'string') {
    return error
  }

  if (error instanceof Error) {
    return error.message
  }

  return fallback
}

export function AppStateProvider({ children }: PropsWithChildren) {
  const [status, setStatus] = useState<AppStatus>('booting')
  const [bootstrapError, setBootstrapError] = useState<string | null>(null)
  const [session, setSession] = useState<SessionSummary | null>(null)
  const [settings, setSettings] = useState<AppSettings>(defaultSettings)
  const [settingsDirty, setSettingsDirty] = useState(false)
  const [terminalSessionActive, setTerminalSessionActive] = useState(false)
  const [headerActions, setHeaderActions] = useState<ReactNode>(null)
  const [device, setDevice] = useState<LocalDeviceSummary | null>(null)
  const [devices, setDevices] = useState<DeviceInfo[]>([])
  const [cloud, setCloud] = useState<CloudStatus>(defaultCloudStatus)
  const [messages, setMessages] = useState<TextMessageRecord[]>([])
  const [transfers, setTransfers] = useState<FileTransferRecord[]>([])
  const [transferSpeeds, setTransferSpeeds] = useState<Record<string, number>>({})

  const [theme, setThemeState] = useState<'light' | 'dark' | 'auto'>(() => {
    const saved = localStorage.getItem('colink-theme')
    if (saved === 'light' || saved === 'dark' || saved === 'auto') {
      return saved
    }
    return 'dark'
  })

  useEffect(() => {
    const root = window.document.documentElement
    
    function applyTheme() {
      if (theme === 'dark') {
        root.classList.add('dark')
      } else if (theme === 'light') {
        root.classList.remove('dark')
      } else {
        // Auto mode
        const systemIsDark = window.matchMedia('(prefers-color-scheme: dark)').matches
        if (systemIsDark) {
          root.classList.add('dark')
        } else {
          root.classList.remove('dark')
        }
      }
    }

    applyTheme()
    localStorage.setItem('colink-theme', theme)

    if (theme === 'auto') {
      const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
      const listener = () => applyTheme()
      
      if (mediaQuery.addEventListener) {
        mediaQuery.addEventListener('change', listener)
      } else {
        mediaQuery.addListener(listener)
      }
      
      return () => {
        if (mediaQuery.removeEventListener) {
          mediaQuery.removeEventListener('change', listener)
        } else {
          mediaQuery.removeListener(listener)
        }
      }
    }
  }, [theme])

  const setTheme = useCallback((nextTheme: 'light' | 'dark' | 'auto') => {
    setThemeState(nextTheme)
  }, [])

  const applyBootstrap = useCallback((payload: BootstrapPayload) => {
    const language = resolveLanguage(payload.settings.language)
    if (i18n.language !== language) {
      void i18n.changeLanguage(language)
    }
    setSession(payload.session)
    setSettings({ ...payload.settings, language })
    setDevice(payload.device)
    setDevices(payload.devices)
    setCloud(payload.cloud)
    setMessages(payload.messages)
    setTransfers(payload.transfers)
    setTransferSpeeds({})
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
      setTransferSpeeds({})
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
    let unlistenTransferProgress: (() => void) | null = null

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

        unlistenAuth = await listen<string>('auth-invalidated', (event) => {
          if (disposed) {
          return
        }

          toast.info(event.payload || i18n.t('auth.sessionInvalidated'))
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
            setTransferSpeeds((current) => pruneTransferSpeeds(current, event.payload))
          }
        })

        unlistenTransferProgress = await listen<TransferProgressPayload>('transfer-progress', (event) => {
          if (!disposed) {
            setTransfers((current) => mergeTransferRecord(current, event.payload.record))
            setTransferSpeeds((current) => ({
              ...current,
              [event.payload.record.fileId]: event.payload.bytesPerSecond,
            }))
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
      unlistenTransferProgress?.()
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
    setBootstrapError(null)
    setCloud(defaultCloudStatus)
    await refreshBootstrap()
  }, [refreshBootstrap])

  const refreshDevices = useCallback(async () => {
    const nextDevices = await refreshDevicesRequest()
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
    const language = resolveLanguage(saved.language)
    if (i18n.language !== language) {
      await i18n.changeLanguage(language)
    }
    setSettings({ ...saved, language })
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

  const clearTransfers = useCallback(async () => {
    await clearTransfersRequest()
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
      transferSpeeds,
      theme,
      setTheme,
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
      clearTransfers,
      settingsDirty,
      setSettingsDirty,
      terminalSessionActive,
      setTerminalSessionActive,
      headerActions,
      setHeaderActions,
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
      transferSpeeds,
      theme,
      setTheme,
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
      clearTransfers,
      settingsDirty,
      setSettingsDirty,
      terminalSessionActive,
      headerActions,
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
