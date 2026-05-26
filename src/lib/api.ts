import { invoke } from '@tauri-apps/api/core'

import type {
  AppSettings,
  FileTransferRecord,
  BootstrapPayload,
  DeviceInfo,
  LoginPayload,
  RegisterPayload,
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

export function getSettings() {
  return invoke<AppSettings>('get_settings')
}

export function updateSettings(settings: AppSettings) {
  return invoke<AppSettings>('update_settings', { settings })
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
