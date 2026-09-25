import { resolveLanguage } from '../i18n'
import { isReleaseBuild } from './app-meta'

export type DevicePlatform = 'windows' | 'macos' | 'linux' | 'android' | 'ios' | 'unknown'

export interface AppSettings {
  serverUrl: string
  autoStart: boolean
  startMinimized: boolean
  downloadPath: string
  clipboardSync: boolean
  autoAcceptFileOffers: boolean
  language: string
}

export interface SessionSummary {
  userId: string
  username: string
}

export interface LocalDeviceSummary {
  deviceId: string
  name: string
  deviceType: DevicePlatform
}

export interface DeviceInfo {
  deviceId: string
  name: string
  type: DevicePlatform
  online: boolean
  cloudAvailable: boolean
  lastSeen: string | null
  publicKey: string
  publicKeyUpdatedAt: number | null
  localIp: string | null
  localPort: number | null
  lanAvailable: boolean
  lanState: 'alive' | 'suspect' | 'unavailable'
  activeRoute: string | null
  deviceSources: string[]
  trustedByLan: boolean
  trustedByCloud: boolean
  securityState: string
}

export interface LanPairingCandidate {
  deviceId: string
  name: string
  type: DevicePlatform
  ip: string
  port: number
  state: string
}

export interface LanPairingRequest {
  requestId: string
  deviceId: string
  name: string
  code: string
  reason: 'unknown_device' | string
  publicKey: string
  initiatedLocally: boolean
  error?: string
}

export interface LanPairingCompleted {
  requestId: string
  deviceId: string
}

export interface LanPairingFailed {
  requestId: string
  deviceId: string
  reason: string
  message: string
}

export interface CloudStatus {
  state: 'disconnected' | 'connecting' | 'connected' | 'reconnecting'
  connected: boolean
  attempt: number
  lastError: string | null
}

export interface BootstrapPayload {
  settings: AppSettings
  session: SessionSummary | null
  devices: DeviceInfo[]
  device: LocalDeviceSummary | null
  cloud: CloudStatus
  messages: TextMessageRecord[]
  transfers: FileTransferRecord[]
}

export interface AppUpdateRelease {
  version: string
  releaseNotes: string
  publishedAt: string
  assets: AppUpdateAsset[]
  automaticInstallAvailable: boolean
}

export interface AppUpdateAsset {
  name: string
  size: number
  downloadUrl: string
  sha256?: string
}

export interface LoginPayload {
  identifier: string
  password: string
}

export interface SavedLoginCredentials {
  identifier: string
  password: string
}

export interface RegisterPayload {
  email: string
  username: string
  password: string
}

export interface TextMessageRecord {
  messageId: string
  deviceId: string
  direction: 'inbound' | 'outbound'
  text: string
  route: string
  createdAt: number
}

export interface FileTransferRecord {
  fileId: string
  deviceId: string
  direction: 'inbound' | 'outbound'
  fileName: string
  fileSize: number
  transferredBytes: number
  totalChunks: number
  status: string
  checksum: string
  route: string
  tempPath: string | null
  finalPath: string | null
  error: string | null
  createdAt: number
  updatedAt: number
}

export interface FileOfferRequest {
  sessionId: string
  deviceId: string
  deviceName: string
  fileName: string
  fileSize: number
  purpose: 'transfer' | 'filesystemDownload'
}

export interface SystemShareFile {
  path: string
  name: string
  size: number
}

export interface TransferProgressPayload {
  record: FileTransferRecord
  bytesPerSecond: number
}

export interface TransferPreparingPayload {
  current: number
  total: number
}

export interface MusicProviderConfig {
  id: string
  enabled: boolean
  priority: number
}

export interface MusicProviderMeta {
  id: string
  name: string
  implemented: boolean
}

export interface CastBoardMonitor {
  id: string
  name: string
  x: number
  y: number
  width: number
  height: number
  scaleFactor: number
}

export type CastBoardState = 'closed' | 'opening' | 'open' | 'closing' | 'failed'

export interface CastBoardStatus {
  state: CastBoardState
  monitor: CastBoardMonitor | null
  message: string | null
}

export interface SendTextPayload {
  deviceId: string
  text: string
}

export interface SendFilePayload {
  deviceId: string
  paths: string[]
}

export interface RemoteFilesystemRoot {
  path: string
  label?: string | null
  totalBytes?: number | null
  freeBytes?: number | null
}

export interface RemoteFilesystemEntry {
  name: string
  kind: 'directory' | 'file' | 'symlink' | 'other'
  size?: number | null
  modified?: number | null
  created?: number | null
  readonly: boolean
  hidden: boolean
}

export interface RemoteFilesystemRootsResult {
  roots: RemoteFilesystemRoot[]
}

export interface RemoteFilesystemListResult {
  path: string
  entries: RemoteFilesystemEntry[]
  total: number
  offset: number
  hasMore: boolean
}

export interface RemoteFilesystemDownload {
  requestId: string
  deviceId: string
  remotePath: string
  requestedAt: number
  sessionId: string | null
  error: string | null
}

export interface RemoteFilesystemUpload {
  requestId: string
  deviceId: string
  remotePath: string
  requestedAt: number
  sessionId: string | null
  error: string | null
}

export type RemoteTerminalSupport = 'unknown' | 'supported' | 'unsupported'
export type RemoteCameraSupport = RemoteTerminalSupport

export interface CameraEntry {
  cameraId: string
  label: string
  position?: string | null
}

export type NoteSyncState = 'synced' | 'pending' | 'pendingDelete' | 'conflict' | 'conflictDelete'

export type NoteConflictKind = 'edit' | 'cloudDeleted' | 'delete'

export interface NoteRecord {
  id: string
  title: string
  markdown: string
  tagIds: string[]
  attachmentIds: string[]
  revision: number
  baseRevision: number
  syncState: NoteSyncState
  conflictKind: NoteConflictKind | null
  conflictTitle: string | null
  conflictMarkdown: string | null
  conflictTagIds: string[] | null
  conflictAttachmentIds: string[] | null
  conflictRevision: number | null
  ancestorTitle: string
  ancestorMarkdown: string
  ancestorTagIds: string[]
  ancestorAttachmentIds: string[]
  ancestorRevision: number
  deleted: boolean
  createdAt: number
  updatedAt: number
}

export interface NoteTagRecord {
  id: string
  name: string
  revision: number
  baseRevision: number
  syncState: NoteSyncState
  deleted: boolean
  createdAt: number
  updatedAt: number
}

export interface NoteAttachmentRecord {
  id: string
  kind: 'image' | 'file' | string
  fileName: string
  mediaType: string
  size: number
  sha256: string
  syncState: NoteSyncState
  deleted: boolean
  createdAt: number
}

export interface NoteUpsertPayload {
  id?: string
  title: string
  markdown: string
  tagIds: string[]
  attachmentIds: string[]
}

export interface ConflictResolutionPayload {
  noteId: string
  resolution: 'local' | 'cloud' | 'merged' | 'confirm_delete' | 'cancel_delete'
  title?: string
  markdown?: string
  tagIds?: string[]
  attachmentIds?: string[]
}

export interface NotesSyncOutcome {
  status: 'ok' | 'offline' | 'unsupported' | 'storage_full' | 'error'
  message: string | null
  pushedNotes: number
  pushedTags: number
  pushedAttachments: number
  pulledNotes: number
  pulledTags: number
  conflicts: number
  repairedReferences: number
}

export interface AttachmentDeleteOutcome {
  unsupported: boolean
}

export interface DeviceDeleteOutcome {
  devices: DeviceInfo[]
  notFound: boolean
}

export interface NotesStorageInfo {
  usedBytes: number
  limitBytes: number
  remainingBytes: number
  attachmentBytes: number
  markdownBytes: number
  maxAttachmentBytes: number
  maxMarkdownBytes: number
}

export const defaultSettings: AppSettings = {
  serverUrl: 'http://127.0.0.1:8080',
  autoStart: isReleaseBuild,
  startMinimized: true,
  downloadPath: '',
  clipboardSync: true,
  autoAcceptFileOffers: true,
  language: resolveLanguage(),
}

export const defaultCloudStatus: CloudStatus = {
  state: 'disconnected',
  connected: false,
  attempt: 0,
  lastError: null,
}
