import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { Copy, X } from 'lucide-react'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import type { DeviceInfo } from '../lib/types'
import { formatPlatformName } from '../lib/utils'
import { Button } from './ui/button'

interface DeviceDetailsDialogProps {
  device: DeviceInfo
  isLocalDevice: boolean
  onClose: () => void
}

export function DeviceDetailsDialog({ device, isLocalDevice, onClose }: DeviceDetailsDialogProps) {
  const { t } = useTranslation()
  const [fingerprint, setFingerprint] = useState('')

  useEffect(() => {
    let disposed = false

    void publicKeyFingerprint(device.publicKey).then((value) => {
      if (!disposed) {
        setFingerprint(value)
      }
    })

    return () => {
      disposed = true
    }
  }, [device.publicKey])

  const rows: DetailRowData[] = [
    { label: t('devices.detailsFields.name'), value: device.name },
    { label: t('devices.deviceId'), value: device.deviceId, mono: true },
    { label: t('devices.detailsFields.platform'), value: formatPlatformName(device.type, t) },
    { label: t('devices.detailsFields.fetchSource'), value: describeSources(device, isLocalDevice, t) },
    { label: t('devices.detailsFields.localReachable'), value: formatBoolean(isLocalDevice, t) },
    { label: t('devices.detailsFields.cloudAvailable'), value: formatBoolean(device.cloudAvailable, t) },
    { label: t('devices.detailsFields.lanAvailable'), value: formatBoolean(device.lanAvailable, t) },
    { label: t('devices.detailsFields.activeRoute'), value: formatRoute(device.activeRoute, t) },
    { label: t('devices.detailsFields.trustedByLan'), value: formatBoolean(device.trustedByLan, t) },
    { label: t('devices.detailsFields.trustedByCloud'), value: formatBoolean(device.trustedByCloud, t) },
    { label: t('devices.detailsFields.lastAlive'), value: device.lastSeen || t('devices.neverConnected') },
    {
      label: t('devices.detailsFields.publicKeyFingerprint'),
      value: device.publicKey ? fingerprint || t('common.calculating') : t('common.none'),
      mono: true,
    },
    { label: t('devices.detailsFields.publicKey'), value: device.publicKey || t('common.none'), mono: true },
  ]

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
      <div className="flex max-h-[82vh] w-full max-w-2xl flex-col rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
        <div className="flex shrink-0 items-center justify-between">
          <div className="text-[16px] font-semibold text-[hsl(var(--text))]">
            {t('devices.detailsTitle')}
          </div>
          <button
            className="flex h-8 w-8 items-center justify-center rounded-lg text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            onClick={onClose}
            title={t('common.close')}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mt-5 min-h-0 flex-1 overflow-y-auto pr-1">
          <div className="divide-y">
            {rows.map((row) => (
              <DetailRow key={row.label} row={row} />
            ))}
          </div>
        </div>

        <div className="mt-6 flex shrink-0 justify-end gap-2">
          <Button
            onClick={() => {
              void navigator.clipboard.writeText(
                rows.map((row) => `${row.label}: ${row.value}`).join('\n'),
              )
              toast.success(t('devices.detailsCopied'))
            }}
            variant="secondary"
          >
            <Copy className="h-3.5 w-3.5" />
            {t('common.copy')}
          </Button>
          <Button onClick={onClose} variant="primary">
            {t('common.close')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}

interface DetailRowData {
  label: string
  value: string
  mono?: boolean
}

function DetailRow({ row }: { row: DetailRowData }) {
  return (
    <div className="grid gap-1 py-2.5 md:grid-cols-[160px_minmax(0,1fr)] md:gap-4">
      <div className="text-[12px] text-[hsl(var(--muted))]">{row.label}</div>
      <div
        className={
          row.mono
            ? 'break-all font-mono text-[12px] leading-relaxed text-[hsl(var(--text-secondary))]'
            : 'break-words text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]'
        }
      >
        {row.value}
      </div>
    </div>
  )
}

function describeSources(device: DeviceInfo, isLocalDevice: boolean, t: (key: string) => string) {
  const sources = new Set<string>()
  const deviceSources = device.deviceSources ?? []

  if (isLocalDevice || deviceSources.includes('local')) {
    sources.add(t('devices.detailsSources.localIdentity'))
  }
  if (deviceSources.includes('cloud')) {
    sources.add(t('devices.detailsSources.serverDeviceList'))
  }
  if (deviceSources.includes('trusted_peer_key')) {
    sources.add(t('devices.detailsSources.trustedPeerKey'))
  }

  if (sources.size === 0) {
    sources.add(t('devices.detailsSources.serverDeviceList'))
  }

  return Array.from(sources).join(', ')
}

function formatBoolean(value: boolean, t: (key: string) => string) {
  return value ? t('common.yes') : t('common.no')
}

function formatRoute(value: string | null, t: (key: string) => string) {
  if (!value) {
    return t('common.none')
  }

  const labels: Record<string, string> = {
    lan: t('devices.routes.lan'),
    cloud: t('devices.routes.cloud'),
  }

  return labels[value] ?? value
}

async function publicKeyFingerprint(publicKey: string) {
  if (!publicKey) {
    return ''
  }

  try {
    const bytes = base64ToBytes(publicKey)
    const digest = await window.crypto.subtle.digest('SHA-256', bytes)
    return Array.from(new Uint8Array(digest))
      .map((byte) => byte.toString(16).padStart(2, '0'))
      .join(':')
  } catch {
    const fallback = await window.crypto.subtle.digest('SHA-256', new TextEncoder().encode(publicKey))
    return Array.from(new Uint8Array(fallback))
      .map((byte) => byte.toString(16).padStart(2, '0'))
      .join(':')
  }
}

function base64ToBytes(value: string) {
  const binary = window.atob(value)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index)
  }
  return bytes
}
