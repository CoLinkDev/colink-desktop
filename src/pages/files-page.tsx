import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import type { LucideIcon } from 'lucide-react'
import {
  AlertCircle,
  Archive,
  ChevronRight,
  ChevronUp,
  Download,
  EyeOff,
  ExternalLink,
  File,
  FileAudio,
  FileCode2,
  FileImage,
  FileText,
  FileVideo,
  Folder,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  LockKeyhole,
  RefreshCw,
  Upload,
  X,
} from 'lucide-react'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import {
  downloadRemoteFilesystemFile,
  listRemoteFilesystem,
  listRemoteFilesystemDownloads,
  listRemoteFilesystemUploads,
  listRemoteFilesystemRoots,
  openReceivedFile,
  revealReceivedFile,
  uploadRemoteFilesystemFile,
} from '../lib/api'
import type {
  RemoteFilesystemDownload,
  RemoteFilesystemEntry,
  RemoteFilesystemRoot,
  RemoteFilesystemUpload,
} from '../lib/types'
import { cn, formatBytes, formatPlatformName, formatTimestamp } from '../lib/utils'

const REMOTE_FILESYSTEM_DOWNLOADS_UPDATED_EVENT = 'remote-filesystem-downloads-updated'
const REMOTE_FILESYSTEM_UPLOADS_UPDATED_EVENT = 'remote-filesystem-uploads-updated'
const REMOTE_FILESYSTEM_UNSUPPORTED_ERROR = 'colink:filesystem.unsupported.v1'

export function FilesPage() {
  const { t } = useTranslation()
  const { devices, device, transfers, setHeaderActions, pickFiles, cancelTransfer } = useAppState()
  const [searchParams, setSearchParams] = useSearchParams()
  const [roots, setRoots] = useState<RemoteFilesystemRoot[]>([])
  const [entries, setEntries] = useState<RemoteFilesystemEntry[]>([])
  const [currentPath, setCurrentPath] = useState<string | null>(null)
  const [total, setTotal] = useState(0)
  const [hasMore, setHasMore] = useState(false)
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [unsupported, setUnsupported] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [downloads, setDownloads] = useState<RemoteFilesystemDownload[]>([])
  const [uploads, setUploads] = useState<RemoteFilesystemUpload[]>([])
  const generationRef = useRef(0)
  const refreshedUploadSessionIds = useRef(new Set<string>())

  const targetDevices = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId && item.online),
    [device?.deviceId, devices],
  )
  const selectedDeviceId = useMemo(() => {
    const requestedDeviceId = searchParams.get('deviceId')
    if (requestedDeviceId && targetDevices.some((item) => item.deviceId === requestedDeviceId)) {
      return requestedDeviceId
    }
    return targetDevices[0]?.deviceId ?? ''
  }, [searchParams, targetDevices])
  const selectedDevice = targetDevices.find((item) => item.deviceId === selectedDeviceId) ?? null
  const selectedDeviceKey = selectedDevice?.deviceId ?? null

  const loadRoots = useCallback(async () => {
    if (!selectedDeviceId) {
      return
    }

    const generation = generationRef.current + 1
    generationRef.current = generation
    setLoading(true)
    setLoadingMore(false)
    setCurrentPath(null)
    setRoots([])
    setEntries([])
    setTotal(0)
    setHasMore(false)
    setUnsupported(false)
    setError(null)

    try {
      const result = await listRemoteFilesystemRoots(selectedDeviceId)
      if (generationRef.current !== generation) {
        return
      }
      setRoots(result.roots)
    } catch (requestError) {
      if (generationRef.current === generation) {
        if (isRemoteFilesystemUnsupportedError(requestError)) {
          setUnsupported(true)
          setError(null)
        } else {
          setError(remoteFilesystemErrorMessage(requestError, t))
        }
      }
    } finally {
      if (generationRef.current === generation) {
        setLoading(false)
      }
    }
  }, [selectedDeviceId])

  const loadDirectory = useCallback(async (path: string, append = false) => {
    if (!selectedDeviceId) {
      return
    }

    const offset = append ? entries.length : 0
    const generation = append ? generationRef.current : generationRef.current + 1
    if (!append) {
      generationRef.current = generation
      setLoading(true)
      setLoadingMore(false)
      setCurrentPath(path)
      setRoots([])
      setEntries([])
      setTotal(0)
      setHasMore(false)
      setUnsupported(false)
      setError(null)
    } else {
      setLoadingMore(true)
      setError(null)
    }

    try {
      const result = await listRemoteFilesystem(selectedDeviceId, path, offset)
      if (generationRef.current !== generation) {
        return
      }
      setCurrentPath(result.path)
      setEntries((current) => append ? [...current, ...result.entries] : result.entries)
      setTotal(result.total)
      setHasMore(result.hasMore)
    } catch (requestError) {
      if (generationRef.current === generation) {
        if (isRemoteFilesystemUnsupportedError(requestError)) {
          setUnsupported(true)
          setError(null)
        } else {
          setError(remoteFilesystemErrorMessage(requestError, t))
        }
      }
    } finally {
      if (generationRef.current === generation) {
        if (append) {
          setLoadingMore(false)
        } else {
          setLoading(false)
        }
      }
    }
  }, [entries.length, selectedDeviceId])

  const refresh = useCallback(() => {
    if (currentPath) {
      void loadDirectory(currentPath)
    } else {
      void loadRoots()
    }
  }, [currentPath, loadDirectory, loadRoots])

  useEffect(() => {
    generationRef.current += 1
    if (!selectedDeviceKey) {
      setRoots([])
      setEntries([])
      setCurrentPath(null)
      setTotal(0)
      setHasMore(false)
      setUnsupported(false)
      setError(null)
      setLoading(false)
      setLoadingMore(false)
      return
    }
    void loadRoots()
  }, [loadRoots, selectedDeviceKey])

  useEffect(() => {
    let disposed = false
    let unlistenDownloads: (() => void) | null = null
    let unlistenUploads: (() => void) | null = null

    void (async () => {
      try {
        const [initialDownloads, initialUploads] = await Promise.all([
          listRemoteFilesystemDownloads(),
          listRemoteFilesystemUploads(),
        ])
        if (!disposed) {
          setDownloads(initialDownloads)
          setUploads(initialUploads)
        }
        unlistenDownloads = await listen<RemoteFilesystemDownload[]>(
          REMOTE_FILESYSTEM_DOWNLOADS_UPDATED_EVENT,
          (event) => {
            if (!disposed) {
              setDownloads(event.payload)
            }
          },
        )
        unlistenUploads = await listen<RemoteFilesystemUpload[]>(
          REMOTE_FILESYSTEM_UPLOADS_UPDATED_EVENT,
          (event) => {
            if (!disposed) {
              setUploads(event.payload)
            }
          },
        )
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      disposed = true
      unlistenDownloads?.()
      unlistenUploads?.()
    }
  }, [])

  const downloadsByPath = useMemo(() => {
    const result = new Map<string, RemoteFilesystemDownload>()
    for (const download of downloads) {
      if (download.deviceId !== selectedDeviceId) {
        continue
      }
      const previous = result.get(download.remotePath)
      if (!previous || previous.requestedAt < download.requestedAt) {
        result.set(download.remotePath, download)
      }
    }
    return result
  }, [downloads, selectedDeviceId])

  const uploadsByPath = useMemo(() => {
    const result = new Map<string, RemoteFilesystemUpload>()
    for (const upload of uploads) {
      if (upload.deviceId !== selectedDeviceId) {
        continue
      }
      const previous = result.get(upload.remotePath)
      if (!previous || previous.requestedAt < upload.requestedAt) {
        result.set(upload.remotePath, upload)
      }
    }
    return result
  }, [selectedDeviceId, uploads])

  useEffect(() => {
    if (!currentPath) {
      return
    }
    for (const upload of uploadsByPath.values()) {
      if (remoteParent(upload.remotePath) !== currentPath || !upload.sessionId) {
        continue
      }
      const transfer = transfers.find((item) => item.fileId === upload.sessionId)
      if (transfer?.status === 'completed' && !refreshedUploadSessionIds.current.has(upload.sessionId)) {
        refreshedUploadSessionIds.current.add(upload.sessionId)
        refresh()
      }
    }
  }, [currentPath, refresh, transfers, uploadsByPath])

  async function requestDownload(entry: RemoteFilesystemEntry) {
    if (!selectedDeviceId || !currentPath) {
      return
    }
    const path = remoteChild(currentPath, entry.name)
    try {
      const download = await downloadRemoteFilesystemFile(selectedDeviceId, path)
      setDownloads((current) => mergeDownload(current, download))
    } catch (requestError) {
      if (isRemoteFilesystemUnsupportedError(requestError)) {
        setUnsupported(true)
        setError(null)
      } else {
        setError(remoteFilesystemErrorMessage(requestError, t))
      }
    }
  }

  const requestUpload = useCallback(async () => {
    if (!selectedDeviceId || !currentPath) return
    try {
      const paths = await pickFiles(true)
      for (const sourcePath of paths) {
        const name = sourcePath.split(/[\\/]/).filter(Boolean).at(-1)
        if (name) {
          const upload = await uploadRemoteFilesystemFile(selectedDeviceId, remoteChild(currentPath, name), sourcePath)
          setUploads((current) => mergeUpload(current, upload))
        }
      }
    } catch (requestError) {
      setError(remoteFilesystemErrorMessage(requestError, t))
    }
  }, [currentPath, pickFiles, selectedDeviceId, t])

  const requestUploadCancel = useCallback((fileId: string) => {
    void cancelTransfer(fileId).catch((requestError) => {
      setError(remoteFilesystemErrorMessage(requestError, t))
    })
  }, [cancelTransfer, t])

  useEffect(() => {
    setHeaderActions(
      <div className="flex items-center gap-1">
        <button
          aria-label={t('common.send')}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
          disabled={!selectedDevice || !currentPath || loading || loadingMore || unsupported}
          onClick={() => void requestUpload()}
          title={t('common.send')}
          type="button"
        >
          <Upload className="h-4 w-4" />
        </button>
        <button
          aria-label={t('common.refresh')}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
          disabled={!selectedDevice || loading || loadingMore || unsupported}
          onClick={refresh}
          title={t('common.refresh')}
          type="button"
        >
          <RefreshCw className={cn('h-4 w-4', (loading || loadingMore) && 'animate-spin')} />
        </button>
      </div>,
    )
    return () => setHeaderActions(null)
  }, [currentPath, loading, loadingMore, refresh, requestUpload, selectedDevice, setHeaderActions, t, unsupported])

  function selectDevice(deviceId: string) {
    setSearchParams({ deviceId })
  }

  function navigateUp() {
    const parent = currentPath ? remoteParent(currentPath) : null
    if (parent) {
      void loadDirectory(parent)
    } else {
      void loadRoots()
    }
  }

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] overflow-hidden animate-fade-in">
      <aside className="h-full overflow-y-auto border-r py-6 pl-8 pr-4 scrollbar-thin">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">
          {t('files.sidebarTitle')}
        </div>
        {targetDevices.length === 0 ? (
          <div className="px-1 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
            {t('files.emptyDevices')}
          </div>
        ) : (
          <div className="space-y-1">
            {targetDevices.map((item) => (
              <button
                className={cn(
                  'w-full rounded-lg border px-3 py-2.5 text-left transition-all',
                  item.deviceId === selectedDeviceId
                    ? 'border-[hsl(var(--text)/0.25)] bg-[hsl(var(--panel))] shadow-sm'
                    : 'border-transparent hover:bg-[hsl(var(--panel-2)/0.5)]',
                )}
                key={item.deviceId}
                onClick={() => selectDevice(item.deviceId)}
                type="button"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
                  <span className={cn('h-1.5 w-1.5 shrink-0 rounded-full', item.online ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--muted))]')} />
                </div>
                <div className="mt-1 truncate text-[11px] text-[hsl(var(--muted))]">
                  {formatPlatformName(item.type, t)}
                </div>
              </button>
            ))}
          </div>
        )}
      </aside>

      <div className="min-w-0 h-full overflow-y-auto px-8 py-6 scrollbar-thin">
        {!selectedDevice ? (
          <EmptySelection />
        ) : (
          <div className="mx-auto flex min-h-full max-w-6xl flex-col">
            <div className="mb-5 flex items-center gap-3">
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]">
                <HardDrive className="h-5 w-5" />
              </div>
              <div className="min-w-0">
                <div className="truncate text-[15px] font-semibold text-[hsl(var(--text))]">{selectedDevice.name}</div>
                <div className="mt-0.5 text-[12px] text-[hsl(var(--muted))]">{formatPlatformName(selectedDevice.type, t)}</div>
              </div>
            </div>

            {error && (
              <div className="mb-5 flex items-start gap-3 rounded-xl border border-[hsl(var(--danger)/0.22)] bg-[hsl(var(--danger)/0.08)] px-4 py-3 text-[13px] text-[hsl(var(--danger))]">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
                <div className="min-w-0 flex-1">
                  <div className="font-medium">{t('files.errorTitle')}</div>
                  <div className="mt-0.5 break-words text-[12px] opacity-90">{error}</div>
                </div>
                <Button onClick={refresh} size="sm" variant="secondary">
                  {t('files.retry')}
                </Button>
              </div>
            )}

            {loading ? (
              <LoadingState />
            ) : unsupported ? (
              <UnsupportedState />
            ) : currentPath === null ? (
              <RootsView roots={roots} onOpen={(path) => void loadDirectory(path)} />
            ) : (
                <DirectoryView
                  currentPath={currentPath}
                  downloadsByPath={downloadsByPath}
                  entries={entries}
                hasMore={hasMore}
                loadingMore={loadingMore}
                onDownload={requestDownload}
                onLoadMore={() => void loadDirectory(currentPath, true)}
                onNavigate={(
                  path,
                ) => void loadDirectory(path)}
                onNavigateUp={navigateUp}
                  onOpenDirectory={(entry) => void loadDirectory(remoteChild(currentPath, entry.name))}
                  onCancelUpload={requestUploadCancel}
                  total={total}
                  transfers={transfers}
                  uploadsByPath={uploadsByPath}
              />
            )}
          </div>
        )}
      </div>
    </div>
  )
}

function EmptySelection() {
  const { t } = useTranslation()
  return (
    <div className="flex h-full min-h-[360px] flex-col items-center justify-center text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]">
        <FolderOpen className="h-6 w-6" />
      </div>
      <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">{t('files.selectDevice')}</div>
      <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">{t('files.selectDeviceDescription')}</p>
    </div>
  )
}

function LoadingState() {
  return (
    <div className="flex min-h-[300px] items-center justify-center">
      <LoaderCircle className="h-5 w-5 animate-spin text-[hsl(var(--muted))]" />
    </div>
  )
}

function UnsupportedState() {
  const { t } = useTranslation()
  return (
    <div className="flex min-h-[300px] flex-col items-center justify-center text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]">
        <HardDrive className="h-6 w-6" />
      </div>
      <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">{t('files.unsupportedTitle')}</div>
      <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">{t('files.unsupportedDescription')}</p>
    </div>
  )
}

function RootsView({ roots, onOpen }: { roots: RemoteFilesystemRoot[]; onOpen: (path: string) => void }) {
  const { t } = useTranslation()
  if (roots.length === 0) {
    return <EmptyContent message={t('files.locationsEmpty')} />
  }

  return (
    <section>
      <div className="mb-3 text-[12px] font-semibold uppercase tracking-widest text-[hsl(var(--muted))]">{t('files.rootsTitle')}</div>
      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
        {roots.map((root) => (
          <button
            className="group rounded-xl border bg-[hsl(var(--panel))] p-4 text-left transition-colors hover:bg-[hsl(var(--panel-2)/0.45)]"
            key={root.path}
            onClick={() => onOpen(root.path)}
            type="button"
          >
            <div className="flex items-start gap-3">
              <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))] group-hover:text-[hsl(var(--text))]">
                <FolderOpen className="h-4 w-4" />
              </div>
              <div className="min-w-0">
                <div className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{root.label || root.path}</div>
                <div className="mt-1 truncate font-mono text-[11px] text-[hsl(var(--muted))]">{root.path}</div>
                {hasByteCount(root.totalBytes) && hasByteCount(root.freeBytes) && (
                  <div className="mt-2 text-[11px] text-[hsl(var(--text-secondary))]">
                    {t('files.storageAvailable', { free: formatBytes(root.freeBytes), total: formatBytes(root.totalBytes) })}
                  </div>
                )}
              </div>
            </div>
          </button>
        ))}
      </div>
    </section>
  )
}

interface DirectoryViewProps {
  currentPath: string
  downloadsByPath: Map<string, RemoteFilesystemDownload>
  uploadsByPath: Map<string, RemoteFilesystemUpload>
  entries: RemoteFilesystemEntry[]
  hasMore: boolean
  loadingMore: boolean
  onDownload: (entry: RemoteFilesystemEntry) => Promise<void>
  onLoadMore: () => void
  onNavigate: (path: string) => void
  onNavigateUp: () => void
  onOpenDirectory: (entry: RemoteFilesystemEntry) => void
  onCancelUpload: (fileId: string) => void
  total: number
  transfers: ReturnType<typeof useAppState>['transfers']
}

function DirectoryView({
  currentPath,
  downloadsByPath,
  uploadsByPath,
  entries,
  hasMore,
  loadingMore,
  onDownload,
  onLoadMore,
  onNavigate,
  onNavigateUp,
  onOpenDirectory,
  onCancelUpload,
  total,
  transfers,
}: DirectoryViewProps) {
  const { t } = useTranslation()
  const breadcrumbs = remoteBreadcrumbs(currentPath)
  const pendingUploads = [...uploadsByPath.values()].filter((upload) =>
    remoteParent(upload.remotePath) === currentPath
      && !entries.some((entry) => remoteChild(currentPath, entry.name) === upload.remotePath)
      && uploadTransferStatus(upload, transfers) !== 'completed',
  )

  return (
    <section className="overflow-hidden rounded-xl border bg-[hsl(var(--panel))]">
      <div className="border-b bg-[hsl(var(--panel-2)/0.2)] px-5 py-3.5">
        <div className="flex items-center gap-3">
          <button
            aria-label={t('files.up')}
            className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel))] hover:text-[hsl(var(--text))]"
            onClick={onNavigateUp}
            title={t('files.up')}
            type="button"
          >
            <ChevronUp className="h-4 w-4" />
          </button>
          <nav className="min-w-0 flex flex-1 items-center overflow-x-auto whitespace-nowrap scrollbar-none" aria-label={t('files.currentFolder')}>
            {breadcrumbs.map((item, index) => (
              <div className="flex min-w-0 items-center" key={item.path}>
                {index > 0 && <ChevronRight className="mx-1 h-3.5 w-3.5 shrink-0 text-[hsl(var(--muted))]" />}
                <button
                  className={cn(
                    'truncate rounded px-1.5 py-1 text-[12px] transition-colors hover:bg-[hsl(var(--panel))]',
                    index === breadcrumbs.length - 1 ? 'font-medium text-[hsl(var(--text))]' : 'text-[hsl(var(--muted))]',
                  )}
                  onClick={() => onNavigate(item.path)}
                  title={item.path}
                  type="button"
                >
                  {item.label}
                </button>
              </div>
            ))}
          </nav>
          <div className="shrink-0 text-[11px] text-[hsl(var(--muted))]">{t('files.itemCount', { count: total })}</div>
        </div>
      </div>

      {entries.length === 0 && pendingUploads.length === 0 ? (
        <EmptyContent message={t('files.directoryEmpty')} />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[720px] table-fixed text-left">
            <thead className="border-b text-[11px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">
              <tr>
                <th className="w-[48%] px-5 py-3 font-semibold">{t('files.columns.name')}</th>
                <th className="w-[19%] px-4 py-3 font-semibold">{t('files.columns.modified')}</th>
                <th className="w-[15%] px-4 py-3 text-right font-semibold">{t('files.columns.size')}</th>
                <th className="w-[18%] px-5 py-3 text-right font-semibold">{t('files.columns.actions')}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {entries.map((entry) => {
                const path = remoteChild(currentPath, entry.name)
                const upload = uploadsByPath.get(path)
                return (
                  <FileEntryRow
                    download={downloadsByPath.get(path)}
                    entry={entry}
                    key={`${entry.kind}:${entry.name}`}
                    onCancelUpload={onCancelUpload}
                    onDownload={() => void onDownload(entry)}
                    onOpenDirectory={() => onOpenDirectory(entry)}
                    transfers={transfers}
                    upload={uploadTransferStatus(upload, transfers) === 'completed' ? undefined : upload}
                  />
                )
              })}
              {pendingUploads.map((upload) => (
                <FileEntryRow
                  download={undefined}
                  entry={remoteUploadEntry(upload)}
                  key={`upload:${upload.requestId}`}
                  onCancelUpload={onCancelUpload}
                  onDownload={() => undefined}
                  onOpenDirectory={() => undefined}
                  transfers={transfers}
                  upload={upload}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      {hasMore && (
        <div className="border-t p-4 text-center">
          <Button disabled={loadingMore} onClick={onLoadMore} variant="secondary">
            {loadingMore && <LoaderCircle className="h-3.5 w-3.5 animate-spin" />}
            {t('files.loadMore')}
          </Button>
        </div>
      )}
    </section>
  )
}

function FileEntryRow({
  download,
  entry,
  onCancelUpload,
  onDownload,
  onOpenDirectory,
  transfers,
  upload,
}: {
  download: RemoteFilesystemDownload | undefined
  entry: RemoteFilesystemEntry
  onCancelUpload: (fileId: string) => void
  onDownload: () => void
  onOpenDirectory: () => void
  transfers: ReturnType<typeof useAppState>['transfers']
  upload: RemoteFilesystemUpload | undefined
}) {
  const { t } = useTranslation()
  const isDirectory = entry.kind === 'directory'
  const sessionId = download?.sessionId
  const transfer = sessionId ? transfers.find((item) => item.fileId === sessionId) : undefined
  const hasDownloadError = Boolean(download?.error)
  const failed = hasDownloadError || ['failed', 'rejected', 'cancelled'].includes(transfer?.status ?? '')
  const completed = transfer?.status === 'completed'
  const waitingForOffer = Boolean(download) && !sessionId && !hasDownloadError
  const waitingForApproval = Boolean(sessionId) && !transfer && !hasDownloadError
  const active = Boolean(transfer) && !failed && !completed
  const uploadSessionId = upload?.sessionId
  const uploadTransfer = uploadSessionId ? transfers.find((item) => item.fileId === uploadSessionId) : undefined
  const uploadStatus = uploadTransfer?.status
  const uploadFailed = Boolean(upload?.error) || ['failed', 'rejected', 'cancelled'].includes(uploadStatus ?? '')
  const uploadCompleted = uploadStatus === 'completed'
  const uploadWaitingForOffer = Boolean(upload) && !uploadSessionId && !upload?.error
  const uploadWaitingForApproval = Boolean(uploadSessionId) && !uploadTransfer && !upload?.error
  const uploadActive = Boolean(uploadTransfer) && !uploadFailed && !uploadCompleted
  const uploadCancellable = uploadStatus === 'offered' || uploadStatus === 'accepted' || uploadStatus === 'sending'
  const uploadProgress = uploadTransfer && uploadTransfer.fileSize > 0
    ? Math.min(1, Math.max(0, uploadTransfer.transferredBytes / uploadTransfer.fileSize))
    : 0
  const status = upload
    ? remoteUploadStatus(upload, uploadStatus, uploadTransfer?.transferredBytes, uploadTransfer?.fileSize, t)
    : downloadStatus(download, transfer?.status, t)
  const statusError = upload ? upload.error : download?.error
  const statusFailed = upload ? uploadFailed : failed

  function openDirectoryFromRow() {
    if (isDirectory) {
      onOpenDirectory()
    }
  }

  return (
    <tr
      className={cn(
        'transition-colors hover:bg-[hsl(var(--panel-2)/0.45)]',
        isDirectory && 'cursor-pointer focus-visible:bg-[hsl(var(--panel-2)/0.45)] focus-visible:outline-none',
      )}
      onClick={isDirectory ? openDirectoryFromRow : undefined}
      onKeyDown={(event) => {
        if (!isDirectory || (event.key !== 'Enter' && event.key !== ' ')) {
          return
        }
        event.preventDefault()
        openDirectoryFromRow()
      }}
      role={isDirectory ? 'button' : undefined}
      tabIndex={isDirectory ? 0 : undefined}
    >
      <td className="px-5 py-3.5">
        <div className="flex min-w-0 max-w-full items-center gap-3 text-left">
          <div className={cn(
            'flex h-8 w-8 shrink-0 items-center justify-center rounded-lg',
            isDirectory ? 'bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]' : 'bg-[hsl(var(--panel-2)/0.65)] text-[hsl(var(--muted))]',
          )}>
            <EntryIcon entry={entry} />
          </div>
          <div className="min-w-0">
            <div className={cn('truncate text-[13px] text-[hsl(var(--text))]', isDirectory && 'font-medium')} title={entry.name}>
              {entry.name}
            </div>
            <div className="mt-1 flex items-center gap-1.5 text-[11px] text-[hsl(var(--muted))]">
              {entry.readonly && <LockKeyhole className="h-3 w-3" aria-label={t('files.readonly')} />}
              {entry.hidden && <EyeOff className="h-3 w-3" aria-label={t('files.hidden')} />}
              {status && <span className={cn(statusFailed && 'text-[hsl(var(--danger))]', !statusFailed && 'text-[hsl(var(--text-secondary))]')} title={statusError ? remoteFilesystemErrorMessage(statusError, t) : undefined}>{status}</span>}
            </div>
          </div>
        </div>
      </td>
      <td className="px-4 py-3.5 text-[12px] text-[hsl(var(--muted))]">{entry.modified ? formatTimestamp(entry.modified) : '—'}</td>
      <td className="px-4 py-3.5 text-right text-[12px] text-[hsl(var(--muted))]">{hasByteCount(entry.size) ? formatBytes(entry.size) : '—'}</td>
      <td className="px-5 py-3.5">
        {entry.kind === 'file' && (
          <div className="flex justify-end gap-1.5">
            {upload ? (
              <>
                {(uploadActive || uploadWaitingForOffer || uploadWaitingForApproval) && (
                  <TransferProgressIndicator progress={uploadProgress} title={status || undefined} />
                )}
                {uploadCancellable && uploadTransfer && (
                  <ActionButton icon={X} label={t('transfers.cancelTitle')} onClick={() => onCancelUpload(uploadTransfer.fileId)} />
                )}
              </>
            ) : completed && transfer ? (
              <>
                <ActionButton icon={ExternalLink} label={t('files.open')} onClick={() => void openReceivedFile(transfer.fileId)} />
                <ActionButton icon={FolderOpen} label={t('files.showInFolder')} onClick={() => void revealReceivedFile(transfer.fileId)} />
              </>
            ) : active || waitingForOffer || waitingForApproval ? (
              <span className="inline-flex h-8 w-8 items-center justify-center text-[hsl(var(--muted))]" title={status || undefined}>
                <LoaderCircle className="h-4 w-4 animate-spin" />
              </span>
            ) : (
              <ActionButton icon={Download} label={t('files.download')} onClick={onDownload} />
            )}
          </div>
        )}
      </td>
    </tr>
  )
}

function ActionButton({ icon: Icon, label, onClick }: { icon: LucideIcon; label: string; onClick: () => void }) {
  return (
    <button
      aria-label={label}
      className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
      onClick={onClick}
      title={label}
      type="button"
    >
      <Icon className="h-3.5 w-3.5" />
    </button>
  )
}

function EntryIcon({ entry }: { entry: RemoteFilesystemEntry }) {
  const Icon = iconForEntry(entry)
  return <Icon className="h-4 w-4" />
}

function EmptyContent({ message }: { message: string }) {
  return <div className="py-20 text-center text-[13px] text-[hsl(var(--muted))]">{message}</div>
}

function iconForEntry(entry: RemoteFilesystemEntry) {
  if (entry.kind === 'directory') return Folder
  const extension = entry.name.split('.').pop()?.toLowerCase() ?? ''
  if (['jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp', 'svg'].includes(extension)) return FileImage
  if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv', '3gp'].includes(extension)) return FileVideo
  if (['mp3', 'wav', 'ogg', 'flac', 'm4a', 'aac', 'wma'].includes(extension)) return FileAudio
  if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(extension)) return Archive
  if (['kt', 'java', 'py', 'js', 'ts', 'tsx', 'html', 'css', 'json', 'xml', 'cpp', 'c', 'sh', 'bat'].includes(extension)) return FileCode2
  if (['pdf', 'doc', 'docx', 'txt', 'rtf', 'odt', 'xls', 'xlsx', 'csv', 'ods', 'ppt', 'pptx', 'odp'].includes(extension)) return FileText
  return File
}

function downloadStatus(
  download: RemoteFilesystemDownload | undefined,
  transferStatus: string | undefined,
  t: TFunction,
) {
  if (!download) return null
  if (download.error) return t('transfers.status.failed')
  if (!download.sessionId) return t('files.waitingForOffer')
  if (!transferStatus) return t('files.waitingForApproval')
  return t(`transfers.status.${transferStatus}`, { defaultValue: transferStatus })
}

function remoteUploadStatus(
  upload: RemoteFilesystemUpload,
  status: string | undefined,
  transferredBytes: number | undefined,
  fileSize: number | undefined,
  t: TFunction,
) {
  if (upload.error) return t('transfers.status.failed')
  if (!upload.sessionId) return t('files.waitingForOffer')
  if (!status) return t('files.waitingForApproval')
  if (status === 'sending') {
    return `${t('transfers.status.sending')} ${formatBytes(transferredBytes ?? 0)} / ${formatBytes(fileSize ?? 0)}`
  }
  if (status === 'accepted') return t('transfers.status.offered')
  return t(`transfers.status.${status}`, { defaultValue: status })
}

function uploadTransferStatus(
  upload: RemoteFilesystemUpload | undefined,
  transfers: ReturnType<typeof useAppState>['transfers'],
) {
  if (!upload || upload.error || !upload.sessionId) return null
  return transfers.find((item) => item.fileId === upload.sessionId)?.status
}

function remoteUploadEntry(upload: RemoteFilesystemUpload): RemoteFilesystemEntry {
  const name = upload.remotePath.split(/[\\/]/).filter(Boolean).at(-1) || upload.remotePath
  return {
    name,
    kind: 'file',
    readonly: false,
    hidden: false,
  }
}

function TransferProgressIndicator({ progress, title }: { progress: number; title?: string }) {
  const radius = 8
  const circumference = 2 * Math.PI * radius
  const offset = circumference * (1 - progress)
  return (
    <span className="inline-flex h-8 w-8 items-center justify-center text-[hsl(var(--primary))]" title={title}>
      <svg aria-hidden="true" className="h-4 w-4 -rotate-90" viewBox="0 0 20 20">
        <circle className="text-[hsl(var(--panel-2))]" cx="10" cy="10" fill="none" r={radius} stroke="currentColor" strokeWidth="2" />
        <circle
          cx="10"
          cy="10"
          fill="none"
          r={radius}
          stroke="currentColor"
          strokeDasharray={circumference}
          strokeDashoffset={offset}
          strokeLinecap="round"
          strokeWidth="2"
        />
      </svg>
    </span>
  )
}

function mergeDownload(current: RemoteFilesystemDownload[], next: RemoteFilesystemDownload) {
  const index = current.findIndex((item) => item.requestId === next.requestId)
  if (index < 0) return [next, ...current]
  const updated = [...current]
  updated[index] = next
  return updated
}

function mergeUpload(current: RemoteFilesystemUpload[], next: RemoteFilesystemUpload) {
  const index = current.findIndex((item) => item.requestId === next.requestId)
  if (index < 0) return [next, ...current]
  const updated = [...current]
  updated[index] = next
  return updated
}

function isRemoteFilesystemUnsupportedError(error: unknown) {
  return readErrorMessage(error) === REMOTE_FILESYSTEM_UNSUPPORTED_ERROR
}

function remoteFilesystemErrorMessage(error: unknown, t: TFunction) {
  const message = readErrorMessage(error)
  const match = /^colink:fs\.([a-z_]+)\.v1$/.exec(message)
  if (!match) {
    return message || t('files.errors.generic')
  }
  return t(`files.errors.${match[1]}`, { defaultValue: t('files.errors.generic') })
}

function hasByteCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
}

function remoteChild(parent: string, name: string) {
  const separator = parent.includes('\\') && !parent.includes('/') ? '\\' : '/'
  return `${parent.replace(/[\\/]+$/, '')}${separator}${name}`
}

function remoteParent(path: string) {
  const trimmed = path.replace(/[\\/]+$/, '')
  if (!trimmed) return null
  const index = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  if (index === 0 && trimmed.startsWith('/')) return '/'
  if (index <= 0) return null
  if (index === 2 && trimmed[1] === ':') return `${trimmed.slice(0, 2)}\\`
  return trimmed.slice(0, index)
}

function remoteBreadcrumbs(path: string) {
  const windowsPath = path.includes('\\') && !path.includes('/')
  const separator = windowsPath ? '\\' : '/'
  const trimmed = path.replace(/[\\/]+$/, '')
  if (windowsPath) {
    const root = /^[A-Za-z]:/.exec(trimmed)?.[0] ?? ''
    const parts = trimmed.slice(root.length).split('\\').filter(Boolean)
    const breadcrumbs = root ? [{ label: root, path: `${root}\\` }] : []
    let current = root ? `${root}\\` : ''
    for (const part of parts) {
      current = current ? `${current.replace(/[\\/]+$/, '')}${separator}${part}` : part
      breadcrumbs.push({ label: part, path: current })
    }
    return breadcrumbs
  }

  const parts = trimmed.split('/').filter(Boolean)
  const breadcrumbs = [{ label: '/', path: '/' }]
  let current = ''
  for (const part of parts) {
    current = `${current}/${part}`
    breadcrumbs.push({ label: part, path: current })
  }
  return breadcrumbs
}
