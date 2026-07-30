import { useCallback, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'
import { ArrowDown, ArrowUp, Grid2X2, Info, Key, List, QrCode, RefreshCw, Search, Trash2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'

import { DeviceCard } from '../components/device-card'
import { DeviceDetailsDialog } from '../components/device-details-dialog'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { createPairString, forgetLanTrust, listLanPairingCandidates, startLanPairing } from '../lib/api'
import type { DeviceInfo, LanPairingCandidate } from '../lib/types'
import { cn, formatLastSeen, formatPlatformName } from '../lib/utils'

type DeviceViewMode = 'cards' | 'list'
type DeviceSortKey = 'name' | 'platform' | 'status' | 'route' | 'lastSeen' | 'security'
type SortDirection = 'asc' | 'desc'
const DEVICE_VIEW_MODE_KEY = 'colink-device-view-mode'

interface DeviceSort {
  key: DeviceSortKey
  direction: SortDirection
}

export function DevicesPage() {
  const { t } = useTranslation()
  const {
    devices,
    device,
    cloud,
    rotateDeviceKey,
    refreshDevices,
    setHeaderActions,
  } = useAppState()
  const [actingId, setActingId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [rotateConfirmId, setRotateConfirmId] = useState<string | null>(null)
  const [forgetConfirmId, setForgetConfirmId] = useState<string | null>(null)
  const [detailsDevice, setDetailsDevice] = useState<DeviceInfo | null>(null)
  const [candidates, setCandidates] = useState<LanPairingCandidate[]>([])
  const [viewMode, setViewMode] = useState<DeviceViewMode>(() => readDeviceViewMode())
  const [searchQuery, setSearchQuery] = useState('')
  const [sort, setSort] = useState<DeviceSort>({ key: 'name', direction: 'asc' })
  const [refreshing, setRefreshing] = useState(false)
  const [pairString, setPairString] = useState<string | null>(null)
  const [legacyPairQr, setLegacyPairQr] = useState(false)

  const handleRefreshDevices = useCallback(async () => {
    setRefreshing(true)
    try {
      await refreshDevices()
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    } finally {
      setRefreshing(false)
    }
  }, [refreshDevices])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        const initial = await listLanPairingCandidates()
        if (!disposed) {
          setCandidates(initial)
        }
        unlisten = await listen<LanPairingCandidate[]>('lan-pairing-candidates-updated', (event) => {
          if (!disposed) {
            setCandidates(event.payload)
          }
        })
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    localStorage.setItem(DEVICE_VIEW_MODE_KEY, viewMode)
    setHeaderActions(
      <>
        <button
          aria-label={t('common.refresh')}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
          disabled={!cloud.connected || refreshing}
          onClick={handleRefreshDevices}
          title={t('common.refresh')}
          type="button"
        >
          <RefreshCw className={cn('h-4 w-4', refreshing && 'animate-spin')} />
        </button>
        <div className="inline-flex rounded-lg border bg-[hsl(var(--panel))] p-0.5">
          <button
            aria-label={t('devices.cardMode')}
            className={cn(
              'inline-flex h-7 w-8 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:text-[hsl(var(--text))]',
              viewMode === 'cards' && 'bg-[hsl(var(--panel-2))] text-[hsl(var(--text))]',
            )}
            onClick={() => setViewMode('cards')}
            title={t('devices.cardMode')}
            type="button"
          >
            <Grid2X2 className="h-4 w-4" />
          </button>
          <button
            aria-label={t('devices.listMode')}
            className={cn(
              'inline-flex h-7 w-8 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:text-[hsl(var(--text))]',
              viewMode === 'list' && 'bg-[hsl(var(--panel-2))] text-[hsl(var(--text))]',
            )}
            onClick={() => setViewMode('list')}
            title={t('devices.listMode')}
            type="button"
          >
            <List className="h-4 w-4" />
          </button>
        </div>
      </>,
    )

    return () => setHeaderActions(null)
  }, [cloud.connected, handleRefreshDevices, refreshing, setHeaderActions, t, viewMode])

  function handleInitiateRotate(deviceId: string) {
    setRotateConfirmId(deviceId)
    setError(null)
  }

  async function handleConfirmRotate() {
    if (!rotateConfirmId) return

    setActingId(rotateConfirmId)
    setError(null)
    try {
      await rotateDeviceKey(rotateConfirmId)
      setRotateConfirmId(null)
      toast.success(t('devices.rotateSuccess'))
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  async function handleStartPairing(deviceId: string) {
    setActingId(deviceId)
    try {
      await startLanPairing(deviceId)
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  async function handleCreatePairString() {
    try {
      setLegacyPairQr(false)
      setPairString(await createPairString())
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    }
  }

  async function handleSwitchPairQr() {
    const legacy = !legacyPairQr
    try {
      setPairString(await createPairString(legacy))
      setLegacyPairQr(legacy)
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    }
  }

  async function handleForgetTrust(deviceId: string) {
    setForgetConfirmId(deviceId)
    setError(null)
  }

  async function handleConfirmForgetTrust() {
    if (!forgetConfirmId) return

    setActingId(forgetConfirmId)
    try {
      await forgetLanTrust(forgetConfirmId)
      await refreshDevices()
      setForgetConfirmId(null)
    } catch (requestError) {
      toast.error(readErrorMessage(requestError))
    } finally {
      setActingId(null)
    }
  }

  const rotatingDevice = devices.find((d) => d.deviceId === rotateConfirmId)
  const forgetDevice = devices.find((d) => d.deviceId === forgetConfirmId)
  const visibleDevices = useMemo(() => {
    const query = searchQuery.trim().toLowerCase()
    const filtered = viewMode === 'list' && query
      ? devices.filter((item) => deviceSearchText(item, item.deviceId === device?.deviceId, t).includes(query))
      : devices

    return [...filtered].sort((left, right) => {
      const result = compareDevices(left, right, sort.key, device?.deviceId ?? null, t)
      return sort.direction === 'asc' ? result : -result
    })
  }, [device?.deviceId, devices, searchQuery, sort.direction, sort.key, t, viewMode])

  function handleSort(key: DeviceSortKey) {
    setSort((current) => ({
      key,
      direction: current.key === key && current.direction === 'asc' ? 'desc' : 'asc',
    }))
  }

  return (
    <div className="space-y-5 animate-fade-in">
      {viewMode === 'list' && (
        <div className="flex items-center gap-3">
          <div className="relative min-w-0 max-w-sm flex-1">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[hsl(var(--muted))]" />
            <Input
              className="pl-9"
              onChange={(event) => setSearchQuery(event.target.value)}
              placeholder={t('devices.searchPlaceholder')}
              value={searchQuery}
            />
          </div>
          <Button
            className="ml-auto shrink-0"
            onClick={handleCreatePairString}
            title={t('devices.showPairQr')}
            variant="secondary"
          >
            <QrCode className="h-4 w-4" />
            {t('devices.showPairQr')}
          </Button>
        </div>
      )}

      {error && !rotateConfirmId && (
        <div className="rounded-lg bg-[hsl(var(--danger)/0.08)] px-4 py-2.5 text-[13px] text-[hsl(var(--danger))]">
          {error}
        </div>
      )}

      {devices.length === 0 ? (
        <div className="py-16 text-center text-[13px] text-[hsl(var(--muted))]">
          {t('devices.empty')}
        </div>
      ) : viewMode === 'list' ? (
        <DeviceList
          actingId={actingId}
          devices={visibleDevices}
          isLocalDevice={(deviceId) => deviceId === device?.deviceId}
          onForgetTrust={handleForgetTrust}
          onRotateKey={handleInitiateRotate}
          onSort={handleSort}
          onViewDetails={setDetailsDevice}
          sort={sort}
        />
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          {devices.map((item) => (
            <DeviceCard
              key={item.deviceId}
              device={item}
              isLocalDevice={item.deviceId === device?.deviceId}
              onViewDetails={setDetailsDevice}
              onRotateKey={handleInitiateRotate}
              onForgetTrust={handleForgetTrust}
              actingId={actingId}
            />
          ))}
        </div>
      )}

      {candidates.length > 0 && (
        <section className="space-y-3">
          <div className="text-[12px] font-semibold uppercase tracking-wider text-[hsl(var(--muted))]">
            {t('devices.lanCandidates')}
          </div>
          <div className="grid gap-2 lg:grid-cols-2">
            {candidates.map((candidate) => (
              <div
                key={candidate.deviceId}
                className="flex items-center justify-between rounded-lg border bg-[hsl(var(--panel))] px-4 py-3"
              >
                <div className="min-w-0">
                  <div className="truncate text-[13px] font-medium text-[hsl(var(--text))]">
                    {candidate.name || candidate.deviceId}
                  </div>
                  <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">
                    {candidate.ip}:{candidate.port} · {candidate.state} · {candidate.deviceId}
                  </div>
                </div>
                <Button
                  disabled={actingId === candidate.deviceId}
                  onClick={() => handleStartPairing(candidate.deviceId)}
                  size="sm"
                  variant="secondary"
                >
                  {t('devices.pair')}
                </Button>
              </div>
            ))}
          </div>
        </section>
      )}

      {detailsDevice && (
        <DeviceDetailsDialog
          device={detailsDevice}
          isLocalDevice={detailsDevice.deviceId === device?.deviceId}
          onClose={() => setDetailsDevice(null)}
        />
      )}

      {pairString && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('devices.pairQrTitle')}</div>
            <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
              {t('devices.pairQrDescription')}
            </p>
            <div className="mt-5 flex justify-center rounded-xl bg-white p-4">
              <QRCodeSVG bgColor="#ffffff" fgColor="#111827" includeMargin size={256} value={pairString} />
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <Button onClick={handleSwitchPairQr} variant="secondary">
                {legacyPairQr ? t('devices.switchToNewPairQr') : t('devices.switchToLegacyPairQr')}
              </Button>
              <Button onClick={() => setPairString(null)} variant="secondary">{t('common.close')}</Button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      {/* Rotate Key Confirmation Modal */}
      {rotateConfirmId && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('devices.rotateConfirmTitle')}</div>
            <p className="mt-2 text-[13px] text-[hsl(var(--text-secondary))] leading-relaxed">
              {t('devices.rotateConfirmDesc', { name: rotatingDevice?.name || t('messages.notSelected') })}
            </p>

            {error && (
              <div className="mt-4 rounded-lg bg-[hsl(var(--danger)/0.08)] px-3.5 py-2.5 text-[12px] text-[hsl(var(--danger))] border border-[hsl(var(--danger)/0.15)]">
                {error}
              </div>
            )}

            <div className="mt-6 flex justify-end gap-2">
              <Button
                variant="secondary"
                onClick={() => {
                  setRotateConfirmId(null)
                  setError(null)
                }}
                disabled={!!actingId}
              >
                {t('common.cancel')}
              </Button>
              <Button
                variant="primary"
                onClick={handleConfirmRotate}
                disabled={!!actingId}
              >
                {actingId ? t('devices.rotating') : t('devices.rotateConfirmBtn')}
              </Button>
            </div>
          </div>
        </div>,
        document.body
      )}

      {forgetConfirmId && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('devices.forgetConfirmTitle')}</div>
            <p className="mt-2 text-[13px] text-[hsl(var(--text-secondary))] leading-relaxed">
              {t('devices.forgetConfirmDesc', { name: forgetDevice?.name || t('messages.notSelected') })}
            </p>

            <div className="mt-6 flex justify-end gap-2">
              <Button
                variant="secondary"
                onClick={() => {
                  setForgetConfirmId(null)
                  setError(null)
                }}
                disabled={!!actingId}
              >
                {t('common.cancel')}
              </Button>
              <Button
                variant="danger"
                onClick={handleConfirmForgetTrust}
                disabled={!!actingId}
              >
                {actingId ? t('common.loading') : t('devices.forgetConfirmBtn')}
              </Button>
            </div>
          </div>
        </div>,
        document.body
      )}
    </div>
  )
}

function readDeviceViewMode(): DeviceViewMode {
  const saved = localStorage.getItem(DEVICE_VIEW_MODE_KEY)
  return saved === 'list' || saved === 'cards' ? saved : 'cards'
}

interface DeviceListProps {
  devices: DeviceInfo[]
  isLocalDevice: (deviceId: string) => boolean
  onViewDetails: (device: DeviceInfo) => void
  onRotateKey: (deviceId: string) => void
  onForgetTrust: (deviceId: string) => void
  onSort: (key: DeviceSortKey) => void
  sort: DeviceSort
  actingId: string | null
}

function DeviceList({
  devices,
  isLocalDevice,
  onViewDetails,
  onRotateKey,
  onForgetTrust,
  onSort,
  sort,
  actingId,
}: DeviceListProps) {
  const { t } = useTranslation()

  if (devices.length === 0) {
    return (
      <div className="rounded-lg border bg-[hsl(var(--panel))] py-12 text-center text-[13px] text-[hsl(var(--muted))]">
        {t('devices.listEmpty')}
      </div>
    )
  }

  return (
    <div className="overflow-hidden rounded-lg border bg-[hsl(var(--panel))]">
      <div className="overflow-x-auto">
        <table className="w-full min-w-[860px] border-collapse text-left text-[13px]">
          <thead className="bg-[hsl(var(--panel-2))] text-[11px] font-semibold uppercase text-[hsl(var(--muted))]">
            <tr>
              <SortableHeader activeSort={sort} label={t('devices.columns.name')} onSort={onSort} sortKey="name" />
              <SortableHeader activeSort={sort} label={t('devices.columns.platform')} onSort={onSort} sortKey="platform" />
              <SortableHeader activeSort={sort} label={t('devices.columns.status')} onSort={onSort} sortKey="status" />
              <SortableHeader activeSort={sort} label={t('devices.columns.route')} onSort={onSort} sortKey="route" />
              <SortableHeader activeSort={sort} label={t('devices.columns.lastSeen')} onSort={onSort} sortKey="lastSeen" />
              <SortableHeader activeSort={sort} label={t('devices.columns.security')} onSort={onSort} sortKey="security" />
              <th className="w-28 px-4 py-3 text-right">{t('devices.columns.actions')}</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {devices.map((item) => {
              const local = isLocalDevice(item.deviceId)
              const canForgetTrust = item.deviceSources.includes('trusted_peer_key')
              return (
                <tr className="transition-colors hover:bg-[hsl(var(--panel-2)/0.65)]" key={item.deviceId}>
                  <td className="px-4 py-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="truncate font-medium text-[hsl(var(--text))]">{item.name}</span>
                        {local && (
                          <span className="rounded-md border px-1.5 py-0.5 text-[11px] text-[hsl(var(--muted))]">
                            {t('devices.localDevice')}
                          </span>
                        )}
                      </div>
                      <div className="mt-1 max-w-[260px] truncate font-mono text-[11px] text-[hsl(var(--muted))]">
                        {item.deviceId}
                      </div>
                    </div>
                  </td>
                  <td className="px-4 py-3 text-[hsl(var(--text-secondary))]">{formatPlatformName(item.type, t)}</td>
                  <td className="px-4 py-3">
                    <DeviceStatusText device={item} isLocalDevice={local} />
                  </td>
                  <td className="px-4 py-3 text-[hsl(var(--text-secondary))]">{deviceRouteLabel(item, t)}</td>
                  <td className="px-4 py-3 text-[hsl(var(--text-secondary))]">
                    {formatLastSeen(item.lastSeen, t('devices.neverConnected'))}
                  </td>
                  <td className="px-4 py-3 text-[hsl(var(--text-secondary))]">{deviceSecurityLabel(item, t)}</td>
                  <td className="px-4 py-3">
                    <div className="flex justify-end gap-1.5">
                      <button
                        aria-label={t('devices.details')}
                        className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
                        onClick={() => onViewDetails(item)}
                        title={t('devices.details')}
                        type="button"
                      >
                        <Info className="h-3.5 w-3.5" />
                      </button>
                      {local && (
                        <button
                          aria-label={t('devices.rotateKey')}
                          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
                          disabled={actingId === item.deviceId}
                          onClick={() => onRotateKey(item.deviceId)}
                          title={t('devices.rotateKey')}
                          type="button"
                        >
                          <Key className="h-3.5 w-3.5" />
                        </button>
                      )}
                      {canForgetTrust && (
                        <button
                          aria-label={t('devices.forgetConfirmBtn')}
                          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border text-[hsl(var(--danger))] transition-colors hover:bg-[hsl(var(--danger)/0.08)] disabled:opacity-40"
                          disabled={actingId === item.deviceId}
                          onClick={() => onForgetTrust(item.deviceId)}
                          title={t('devices.forgetConfirmBtn')}
                          type="button"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </div>
  )
}

function SortableHeader({
  activeSort,
  label,
  onSort,
  sortKey,
}: {
  activeSort: DeviceSort
  label: string
  onSort: (key: DeviceSortKey) => void
  sortKey: DeviceSortKey
}) {
  const active = activeSort.key === sortKey
  const Icon = active && activeSort.direction === 'desc' ? ArrowDown : ArrowUp

  return (
    <th className="px-4 py-3">
      <button
        className="inline-flex items-center gap-1.5 text-left transition-colors hover:text-[hsl(var(--text))]"
        onClick={() => onSort(sortKey)}
        type="button"
      >
        <span>{label}</span>
        <Icon className={cn('h-3 w-3', active ? 'opacity-100' : 'opacity-25')} />
      </button>
    </th>
  )
}

function DeviceStatusText({ device, isLocalDevice }: { device: DeviceInfo; isLocalDevice: boolean }) {
  const { t } = useTranslation()
  const label = isLocalDevice
    ? t('devices.localDevice')
    : device.lanAvailable
      ? device.lanState === 'suspect' ? t('devices.lanSuspect') : t('devices.lan')
      : device.cloudAvailable
        ? t('devices.cloud')
        : t('devices.offline')
  const tone = device.lanState === 'suspect'
    ? 'text-[hsl(var(--warning))]'
    : device.online || isLocalDevice
      ? 'text-[hsl(var(--success))]'
      : 'text-[hsl(var(--muted))]'

  return <span className={cn('font-medium', tone)}>{label}</span>
}

function compareDevices(
  left: DeviceInfo,
  right: DeviceInfo,
  key: DeviceSortKey,
  localDeviceId: string | null,
  t: (key: string) => string,
) {
  if (key === 'lastSeen') {
    return lastSeenValue(left) - lastSeenValue(right)
  }
  return sortValue(left, key, left.deviceId === localDeviceId, t).localeCompare(
    sortValue(right, key, right.deviceId === localDeviceId, t),
    undefined,
    { numeric: true, sensitivity: 'base' },
  )
}

function sortValue(device: DeviceInfo, key: DeviceSortKey, isLocalDevice: boolean, t: (key: string) => string) {
  switch (key) {
    case 'platform':
      return formatPlatformName(device.type, t)
    case 'status':
      return isLocalDevice ? t('devices.localDevice') : deviceStatusLabel(device, t)
    case 'route':
      return deviceRouteLabel(device, t)
    case 'security':
      return deviceSecurityLabel(device, t)
    case 'name':
    default:
      return device.name
  }
}

function deviceSearchText(device: DeviceInfo, isLocalDevice: boolean, t: (key: string) => string) {
  return [
    device.name,
    device.deviceId,
    formatPlatformName(device.type, t),
    isLocalDevice ? t('devices.localDevice') : '',
    deviceStatusLabel(device, t),
    deviceRouteLabel(device, t),
    deviceSecurityLabel(device, t),
  ].join(' ').toLowerCase()
}

function deviceStatusLabel(device: DeviceInfo, t: (key: string) => string) {
  if (device.lanAvailable) {
    return device.lanState === 'suspect' ? t('devices.lanSuspect') : t('devices.lan')
  }
  if (device.cloudAvailable) {
    return t('devices.cloud')
  }
  return t('devices.offline')
}

function deviceRouteLabel(device: DeviceInfo, t: (key: string) => string) {
  if (device.activeRoute === 'lan') {
    return t('devices.routes.lan')
  }
  if (device.activeRoute === 'cloud') {
    return t('devices.routes.cloud')
  }
  return '-'
}

function deviceSecurityLabel(device: DeviceInfo, t: (key: string) => string) {
  switch (device.securityState) {
    case 'verified':
    case 'unverified':
    case 'trusted':
    case 'unknown':
    case 'keyChanged':
      return t(`devices.securityStates.${device.securityState}`)
    default:
      return device.securityState || '-'
  }
}

function lastSeenValue(device: DeviceInfo) {
  if (!device.lastSeen) {
    return 0
  }
  const timestamp = Date.parse(device.lastSeen)
  return Number.isNaN(timestamp) ? 0 : timestamp
}
