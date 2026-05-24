export type DevicePlatform = 'windows' | 'macos' | 'linux' | 'android' | 'ios'

export interface AppSettings {
  serverUrl: string
  autoStart: boolean
  startMinimized: boolean
  lanDiscovery: boolean
  downloadPath: string
  notifications: boolean
}

export interface SessionSummary {
  userId: string
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
  lastSeen: string | null
  publicKey: string
}

export interface BootstrapPayload {
  settings: AppSettings
  session: SessionSummary | null
  devices: DeviceInfo[]
  device: LocalDeviceSummary | null
}

export interface LoginPayload {
  identifier: string
  password: string
}

export interface RegisterPayload {
  email: string
  username: string
  password: string
}

export const defaultSettings: AppSettings = {
  serverUrl: 'http://127.0.0.1:8080',
  autoStart: true,
  startMinimized: true,
  lanDiscovery: true,
  downloadPath: '',
  notifications: true,
}
