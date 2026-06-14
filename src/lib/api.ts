import { invoke } from '@tauri-apps/api/core'

import type {
  AppSettings,
  AppUpdateRelease,
  FileOfferRequest,
  FileTransferRecord,
  BootstrapPayload,
  DeviceInfo,
  LanPairingCandidate,
  LoginPayload,
  MusicProviderConfig,
  MusicProviderMeta,
  RegisterPayload,
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

export function listDevices() {
  return invoke<DeviceInfo[]>('list_devices')
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

export function checkUpdate() {
  return invoke<AppUpdateRelease | null>('check_update')
}

export function openUpdateDownload(url: string) {
  return invoke<void>('open_update_download', { url })
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

export function cancelTransfer(fileId: string) {
  return invoke<void>('cancel_transfer', { fileId })
}

export function clearTransfers() {
  return invoke<void>('clear_transfers')
}

export function pendingFileOffers() {
  return invoke<FileOfferRequest[]>('pending_file_offers')
}

export function respondFileOffer(sessionId: string, accepted: boolean) {
  return invoke<void>('respond_file_offer', {
    payload: { sessionId, accepted },
  })
}
