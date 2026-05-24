import { invoke } from '@tauri-apps/api/core'

import type {
  AppSettings,
  BootstrapPayload,
  DeviceInfo,
  LoginPayload,
  RegisterPayload,
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

export function getSettings() {
  return invoke<AppSettings>('get_settings')
}

export function updateSettings(settings: AppSettings) {
  return invoke<AppSettings>('update_settings', { settings })
}
