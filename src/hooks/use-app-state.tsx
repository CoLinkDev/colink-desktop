import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
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
  notesList as notesListRequest,
  notesTagsList as notesTagsListRequest,
  notesSync as notesSyncRequest,
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
  type NoteRecord,
  type NoteTagRecord,
  type NotesSyncOutcome,
  type RegisterPayload,
  type SendFilePayload,
  type SendTextPayload,
  type SessionSummary,
  type TextMessageRecord,
  type TransferProgressPayload,
} from '../lib/types'
import { readCommandError } from '../lib/command-error'

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
  notes: NoteRecord[]
  notesTags: NoteTagRecord[]
  notesSyncing: boolean
  refreshNotes: () => Promise<void>
  syncNotes: () => Promise<NotesSyncOutcome | null>
  applyNoteRecord: (record: NoteRecord) => void
  removeNoteRecord: (noteId: string) => void
  applyNoteTagRecord: (record: NoteTagRecord) => void
  removeNoteTagRecord: (tagId: string) => void
  transferSpeeds: Record<string, number>
  theme: 'light' | 'dark' | 'auto'
  setTheme: (theme: 'light' | 'dark' | 'auto') => void
  refreshBootstrap: () => Promise<void>
  login: (payload: LoginPayload) => Promise<void>
  register: (payload: RegisterPayload) => Promise<void>
  logout: () => Promise<void>
  refreshDevices: () => Promise<void>
  updateDeviceName: (deviceId: string, name: string) => Promise<void>
  deleteDevice: (deviceId: string) => Promise<boolean>
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
  notesDraftDirty: boolean
  setNotesDraftDirty: (dirty: boolean) => void
  notesDraftBusy: boolean
  setNotesDraftBusy: (busy: boolean) => void
  registerNotesDiscardHandler: (handler: (() => Promise<boolean>) | null) => void
  discardNotesDraft: () => Promise<boolean>
  terminalSessionActive: boolean
  setTerminalSessionActive: (active: boolean) => void
  headerActions: ReactNode
  setHeaderActions: (actions: ReactNode) => void
}

const AppStateContext = createContext<AppStateValue | null>(null)

function isTransferInFlight(record: FileTransferRecord) {
  return record.status === 'sending' || record.status === 'receiving'
}

function isTransferTerminal(record: FileTransferRecord) {
  return record.status === 'completed' || record.status === 'failed' || record.status === 'cancelled' || record.status === 'rejected'
}

function mergeTransferRecord(current: FileTransferRecord[], nextRecord: FileTransferRecord) {
  const next = [...current]
  const index = next.findIndex((item) => item.fileId === nextRecord.fileId)

  if (index >= 0) {
    if (isTransferTerminal(next[index]) && isTransferInFlight(nextRecord)) {
      return current
    }
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

function mergeNoteRecord(current: NoteRecord[], record: NoteRecord) {
  const next = current.filter((item) => item.id !== record.id)
  if (!record.deleted && record.syncState !== 'pendingDelete') {
    next.push(record)
  }
  next.sort((left, right) => right.updatedAt - left.updatedAt || left.id.localeCompare(right.id))
  return next
}

function mergeNoteTagRecord(current: NoteTagRecord[], record: NoteTagRecord) {
  const next = current.filter((item) => item.id !== record.id)
  if (!record.deleted && record.syncState !== 'pendingDelete') {
    next.push(record)
  }
  next.sort((left, right) => left.createdAt - right.createdAt || left.id.localeCompare(right.id))
  return next
}

export function readErrorMessage(error: unknown, fallback = i18n.t('common.requestFailed')) {
  if (typeof error === 'string') {
    return error
  }

  if (error instanceof Error) {
    return error.message
  }

  const commandError = readCommandError(error)
  if (commandError) {
    return commandError.message
  }

  return fallback
}

export function AppStateProvider({ children }: PropsWithChildren) {
  const [status, setStatus] = useState<AppStatus>('booting')
  const [bootstrapError, setBootstrapError] = useState<string | null>(null)
  const [session, setSession] = useState<SessionSummary | null>(null)
  const [settings, setSettings] = useState<AppSettings>(defaultSettings)
  const [settingsDirty, setSettingsDirty] = useState(false)
  const [notesDraftDirty, setNotesDraftDirty] = useState(false)
  const [notesDraftBusy, setNotesDraftBusy] = useState(false)
  const notesDiscardHandlerRef = useRef<(() => Promise<boolean>) | null>(null)
  const [terminalSessionActive, setTerminalSessionActive] = useState(false)
  const [headerActions, setHeaderActions] = useState<ReactNode>(null)
  const [device, setDevice] = useState<LocalDeviceSummary | null>(null)
  const [devices, setDevices] = useState<DeviceInfo[]>([])
  const [cloud, setCloud] = useState<CloudStatus>(defaultCloudStatus)
  const [messages, setMessages] = useState<TextMessageRecord[]>([])
  const [transfers, setTransfers] = useState<FileTransferRecord[]>([])
  const [transferSpeeds, setTransferSpeeds] = useState<Record<string, number>>({})
  const [notes, setNotes] = useState<NoteRecord[]>([])
  const [notesTags, setNotesTags] = useState<NoteTagRecord[]>([])
  const [notesSyncing, setNotesSyncing] = useState(false)
  const notesSyncingRef = useRef(false)
  const notesSyncIssueRef = useRef<NotesSyncOutcome['status'] | null>(null)
  const notesRefreshRef = useRef({ requested: 0, completed: 0, inFlight: null as Promise<void> | null })

  const registerNotesDiscardHandler = useCallback((handler: (() => Promise<boolean>) | null) => {
    notesDiscardHandlerRef.current = handler
  }, [])

  const discardNotesDraft = useCallback(async () => {
    return notesDiscardHandlerRef.current ? notesDiscardHandlerRef.current() : true
  }, [])

  const refreshNotes = useCallback((): Promise<void> => {
    const refresh = notesRefreshRef.current
    refresh.requested += 1
    if (!refresh.inFlight) {
      refresh.inFlight = (async () => {
        while (refresh.completed < refresh.requested) {
          const version = refresh.requested
          try {
            const [nextNotes, nextTags] = await Promise.all([notesListRequest(), notesTagsListRequest()])
            if (version === refresh.requested) {
              setNotes(nextNotes)
              setNotesTags(nextTags)
            }
          } catch (error) {
            console.error('Failed to refresh notes', error)
          } finally {
            refresh.completed = version
          }
        }
      })().finally(() => {
        refresh.inFlight = null
      })
    }
    return refresh.inFlight
  }, [])

  const applyNoteRecord = useCallback((record: NoteRecord) => {
    setNotes((current) => mergeNoteRecord(current, record))
  }, [])

  const removeNoteRecord = useCallback((noteId: string) => {
    setNotes((current) => current.filter((record) => record.id !== noteId))
  }, [])

  const applyNoteTagRecord = useCallback((record: NoteTagRecord) => {
    setNotesTags((current) => mergeNoteTagRecord(current, record))
  }, [])

  const removeNoteTagRecord = useCallback((tagId: string) => {
    setNotesTags((current) => current.filter((record) => record.id !== tagId))
    setNotes((current) => current.map((record) => (
      record.tagIds.includes(tagId)
        ? { ...record, tagIds: record.tagIds.filter((id) => id !== tagId) }
        : record
    )))
  }, [])

  const syncNotes = useCallback(async () => {
    if (notesSyncingRef.current) {
      return null
    }
    notesSyncingRef.current = true
    setNotesSyncing(true)
    try {
      const outcome = await notesSyncRequest()
      if (outcome.status === 'storage_full') {
        if (notesSyncIssueRef.current !== outcome.status) {
          toast.error(i18n.t('notes.storageFull'), { id: 'notes-storage-full' })
        }
        notesSyncIssueRef.current = outcome.status
      } else if (outcome.status === 'ok') {
        notesSyncIssueRef.current = null
      }
      await refreshNotes()
      return outcome
    } catch (error) {
      console.error('Failed to sync notes', error)
      return null
    } finally {
      notesSyncingRef.current = false
      setNotesSyncing(false)
    }
  }, [refreshNotes])

  useEffect(() => {
    void refreshNotes()
  }, [refreshNotes, session?.userId])

  // Sync after the cloud connection is restored and periodically.
  useEffect(() => {
    if (cloud.state === 'connected' && session) {
      void syncNotes()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cloud.state, session?.userId])

  useEffect(() => {
    const timer = window.setInterval(
      () => {
        if (session) {
          void syncNotes()
        }
      },
      5 * 60 * 1000,
    )
    return () => window.clearInterval(timer)
  }, [session?.userId, syncNotes])


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
    let unlistenNotes: (() => void) | null = null

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

        unlistenNotes = await listen('notes-updated', () => {
          if (!disposed) {
            void refreshNotes()
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
      unlistenNotes?.()
    }
  }, [refreshBootstrap, refreshNotes])

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
    const outcome = await deleteDeviceRequest(deviceId)
    setDevices(outcome.devices)
    return outcome.notFound
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
      notes,
      notesTags,
      notesSyncing,
      refreshNotes,
      syncNotes,
      applyNoteRecord,
      removeNoteRecord,
      applyNoteTagRecord,
      removeNoteTagRecord,
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
      notesDraftDirty,
      setNotesDraftDirty,
      notesDraftBusy,
      setNotesDraftBusy,
      registerNotesDiscardHandler,
      discardNotesDraft,
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
      notesDraftDirty,
      notesDraftBusy,
      notes,
      notesTags,
      notesSyncing,
      refreshNotes,
      syncNotes,
      applyNoteRecord,
      removeNoteRecord,
      applyNoteTagRecord,
      removeNoteTagRecord,
      registerNotesDiscardHandler,
      discardNotesDraft,
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
