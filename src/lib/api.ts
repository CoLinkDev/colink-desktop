import { invoke } from '@tauri-apps/api/core'

import type {
  AppSettings,
  AppUpdateRelease,
  FileOfferRequest,
  FileTransferRecord,
  BootstrapPayload,
  DeviceInfo,
  LanPairingCandidate,
  LogPageResult,
  LoginPayload,
  MusicProviderConfig,
  MusicProviderMeta,
  CastBoardMonitor,
  CastBoardStatus,
  RegisterPayload,
  RemoteFilesystemDownload,
  RemoteFilesystemListResult,
  RemoteFilesystemRootsResult,
  RemoteTerminalSupport,
  RemoteCameraSupport,
  CameraEntry,
  SavedLoginCredentials,
  SendFilePayload,
  SendTextPayload,
  TextMessageRecord,
} from './types'

export function bootstrapApp() {
  return invoke<BootstrapPayload>('bootstrap_app')
}

export function login(payload: LoginPayload) {
  return invoke<BootstrapPayload>('login', { payload })
}

export function registerAccount(payload: RegisterPayload) {
  return invoke<BootstrapPayload>('register_account', { payload })
}

export function logout() {
  return invoke<void>('logout')
}

export function getSavedLogin() {
  return invoke<SavedLoginCredentials | null>('get_saved_login')
}

export function saveSavedLogin(payload: SavedLoginCredentials) {
  return invoke<void>('save_saved_login', { payload })
}

export function clearSavedLogin() {
  return invoke<void>('clear_saved_login')
}

export function listLogs(page: number, pageSize: number) {
  return invoke<LogPageResult>('list_logs', {
    payload: {
      page,
      pageSize,
    },
  })
}

export function listDevices() {
  return invoke<DeviceInfo[]>('list_devices')
}

export function refreshDevices() {
  return invoke<DeviceInfo[]>('refresh_devices')
}

export function updateDeviceName(deviceId: string, name: string) {
  return invoke<DeviceInfo[]>('update_device_name', {
    payload: {
      deviceId,
      name,
    },
  })
}

export function deleteDevice(deviceId: string) {
  return invoke<DeviceInfo[]>('delete_device', {
    payload: {
      deviceId,
    },
  })
}

export function rotateDeviceKey(deviceId: string) {
  return invoke<DeviceInfo[]>('rotate_device_key', {
    payload: {
      deviceId,
    },
  })
}

export function listLanPairingCandidates() {
  return invoke<LanPairingCandidate[]>('list_lan_pairing_candidates')
}

export function createPairString(legacy = false) {
  return invoke<string>('create_pair_string', { legacy })
}

export function startLanPairing(deviceId: string) {
  return invoke<void>('start_lan_pairing', {
    payload: { deviceId },
  })
}

export function respondLanPairing(requestId: string, accepted: boolean) {
  return invoke<void>('respond_lan_pairing', {
    payload: { requestId, accepted },
  })
}

export function forgetLanTrust(deviceId: string) {
  return invoke<DeviceInfo[]>('forget_lan_trust', {
    payload: { deviceId },
  })
}

export function getSettings() {
  return invoke<AppSettings>('get_settings')
}

export function updateSettings(settings: AppSettings) {
  return invoke<AppSettings>('update_settings', { settings })
}

export function getMusicProviders() {
  return invoke<MusicProviderConfig[]>('get_music_providers')
}

export function updateMusicProviders(providers: MusicProviderConfig[]) {
  return invoke<void>('update_music_providers', { providers })
}

export function listAvailableMusicProviders() {
  return invoke<MusicProviderMeta[]>('list_available_music_providers')
}

export function listCastBoardMonitors() {
  return invoke<CastBoardMonitor[]>('list_castboard_monitors')
}

export function getCastBoardStatus() {
  return invoke<CastBoardStatus>('get_castboard_status')
}

export function openCastBoardOnMonitor(monitorId: string) {
  return invoke<void>('open_castboard_on_monitor', { monitorId })
}

export function stopCastBoard() {
  return invoke<void>('stop_castboard')
}

export function checkUpdate() {
  return invoke<AppUpdateRelease | null>('check_update')
}

export function openUpdateDownload(url: string) {
  return invoke<void>('open_update_download', { url })
}

export function installTauriUpdate() {
  return invoke<void>('install_tauri_update')
}

export function pickDownloadDirectory() {
  return invoke<string | null>('pick_download_directory')
}

export function sendText(payload: SendTextPayload) {
  return invoke<TextMessageRecord>('send_text', { payload })
}

export function pickFiles(multiple = true) {
  return invoke<string[]>('pick_files', { multiple })
}

export function sendFiles(payload: SendFilePayload) {
  return invoke<FileTransferRecord[]>('send_files', { payload })
}

export function listRemoteFilesystemRoots(deviceId: string) {
  return invoke<RemoteFilesystemRootsResult>('list_remote_filesystem_roots', { deviceId })
}

export function listRemoteFilesystem(deviceId: string, path: string, offset?: number) {
  return invoke<RemoteFilesystemListResult>('list_remote_filesystem', {
    payload: { deviceId, path, offset },
  })
}

export function downloadRemoteFilesystemFile(deviceId: string, path: string) {
  return invoke<RemoteFilesystemDownload>('download_remote_filesystem_file', {
    payload: { deviceId, path },
  })
}

export function listRemoteFilesystemDownloads() {
  return invoke<RemoteFilesystemDownload[]>('list_remote_filesystem_downloads')
}

export function cancelTransfer(fileId: string) {
  return invoke<void>('cancel_transfer', { fileId })
}

export function openReceivedFile(fileId: string) {
  return invoke<void>('open_received_file', { fileId })
}

export function revealReceivedFile(fileId: string) {
  return invoke<void>('reveal_received_file', { fileId })
}

export function clearTransfers() {
  return invoke<void>('clear_transfers')
}

export function pendingFileOffers() {
  return invoke<FileOfferRequest[]>('pending_file_offers')
}

export function respondFileOffer(sessionId: string, accepted: boolean, destinationPath?: string) {
  return invoke<void>('respond_file_offer', {
    payload: { sessionId, accepted, destinationPath },
  })
}

export function openTerminal(deviceId: string, cols: number, rows: number) {
  return invoke<string>('open_terminal', { deviceId, cols, rows })
}

export function getRemoteTerminalSupport(deviceId: string) {
  return invoke<RemoteTerminalSupport>('get_remote_terminal_support', { deviceId })
}

export function writeTerminal(deviceId: string, sessionId: string, data: string) {
  return invoke<void>('write_terminal', { deviceId, sessionId, data })
}

export function resizeTerminal(deviceId: string, sessionId: string, cols: number, rows: number) {
  return invoke<void>('resize_terminal', { deviceId, sessionId, cols, rows })
}

export function closeTerminal(deviceId: string, sessionId: string) {
  return invoke<void>('close_terminal', { deviceId, sessionId })
}

export function getRemoteCameraSupport(deviceId: string) { return invoke<RemoteCameraSupport>('get_remote_camera_support', { deviceId }) }
export function listRemoteCameras(deviceId: string) { return invoke<CameraEntry[]>('list_remote_cameras', { deviceId }) }
export function openRemoteCamera(deviceId: string, cameraId: string, preferredCodecs: string[]) { return invoke<string>('open_remote_camera', { deviceId, cameraId, preferredCodecs }) }

export function sendCameraAlive(deviceId: string, sessionId: string) { return invoke<void>('send_camera_alive', { deviceId, sessionId }) }
export function closeRemoteCamera(deviceId: string, sessionId: string) { return invoke<void>('close_remote_camera', { deviceId, sessionId }) }
