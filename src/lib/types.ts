import { resolveLanguage } from '../i18n'

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
}

export interface LanPairingCompleted {
  requestId: string
  deviceId: string
}

export interface LanPairingFailed {
  requestId: string
  deviceId: string
  reason: string
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
  logs: AppLogEntry[]
}

export interface AppUpdateRelease {
  version: string
  releaseNotes: string
  publishedAt: string
  assets: AppUpdateAsset[]
}

export interface AppUpdateAsset {
  name: string
  size: number
  downloadUrl: string
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
}

export interface TransferProgressPayload {
  record: FileTransferRecord
  bytesPerSecond: number
}

export interface TransferPreparingPayload {
  current: number
  total: number
}

export interface AppLogEntry {
  id: string
  level: string
  source: string
  message: string
  createdAt: number
}

export interface LogPageResult {
  logs: AppLogEntry[]
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

export const defaultSettings: AppSettings = {
  serverUrl: 'http://127.0.0.1:8080',
  autoStart: true,
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
