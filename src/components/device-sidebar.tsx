import { useCallback, useMemo, type ReactNode } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { useAppState } from '../hooks/use-app-state'
import type { DeviceInfo, DevicePlatform } from '../lib/types'
import { cn, formatPlatformName } from '../lib/utils'

export type DeviceSortPreset = 'none' | 'online-first' | 'name-asc' | 'name-desc'
export type DeviceSortComparator = (a: DeviceInfo, b: DeviceInfo) => number
export type DeviceSortOption = DeviceSortPreset | DeviceSortComparator

export interface UseTargetDevicesOptions {
  /** Optional custom source device list; defaults to useAppState().devices */
  inputDevices?: DeviceInfo[]
  /** Whether to exclude current local device; defaults to true */
  excludeSelf?: boolean
  /** Whether to only include online devices; defaults to false */
  onlineOnly?: boolean
  /** Filter to specific device platform types (e.g. ['windows', 'macos', 'linux']) */
  allowedTypes?: DevicePlatform[]
  /** Additional custom filter predicate */
  filter?: (device: DeviceInfo) => boolean
  /** Sorting criteria: preset string, comparator function, or array of presets/comparators */
  sort?: DeviceSortOption | DeviceSortOption[]
  /** Whether to automatically select the first device if none is selected; defaults to true */
  autoSelectFirst?: boolean
  /** Externally controlled selected device ID */
  selectedDeviceId?: string
  /** Whether to sync selected device ID with URL search params; defaults to true */
  syncSearchParams?: boolean
  /** Search parameter key in URL query; defaults to 'deviceId' */
  searchParamKey?: string
  /** Callback fired when a device is selected */
  onSelectDevice?: (deviceId: string, device: DeviceInfo) => void
}

export interface UseTargetDevicesResult {
  devices: DeviceInfo[]
  selectedDeviceId: string
  selectedDevice: DeviceInfo | null
  selectDevice: (deviceId: string) => void
}

function compareByPreset(preset: DeviceSortPreset, a: DeviceInfo, b: DeviceInfo): number {
  switch (preset) {
    case 'online-first':
      return a.online === b.online ? 0 : a.online ? -1 : 1
    case 'name-asc':
      return a.name.localeCompare(b.name)
    case 'name-desc':
      return b.name.localeCompare(a.name)
    case 'none':
    default:
      return 0
  }
}

export function sortDevices(devices: DeviceInfo[], sortOption?: DeviceSortOption | DeviceSortOption[]): DeviceInfo[] {
  if (!sortOption) return devices
  const options = Array.isArray(sortOption) ? sortOption : [sortOption]
  if (options.length === 0 || (options.length === 1 && options[0] === 'none')) {
    return devices
  }

  return [...devices].sort((a, b) => {
    for (const option of options) {
      const result = typeof option === 'function'
        ? option(a, b)
        : compareByPreset(option, a, b)
      if (result !== 0) return result
    }
    return 0
  })
}

export function useTargetDevices(options: UseTargetDevicesOptions = {}): UseTargetDevicesResult {
  const {
    inputDevices,
    excludeSelf = true,
    onlineOnly = false,
    allowedTypes,
    filter: customFilter,
    sort,
    autoSelectFirst = true,
    selectedDeviceId: controlledSelectedDeviceId,
    syncSearchParams = true,
    searchParamKey = 'deviceId',
    onSelectDevice,
  } = options

  const { device: currentDevice, devices: stateDevices } = useAppState()
  const [searchParams, setSearchParams] = useSearchParams()

  const rawDevices = inputDevices ?? stateDevices

  const filteredAndSortedDevices = useMemo(() => {
    const filtered = rawDevices.filter((item) => {
      if (excludeSelf && currentDevice?.deviceId && item.deviceId === currentDevice.deviceId) {
        return false
      }
      if (onlineOnly && !item.online) {
        return false
      }
      if (allowedTypes && !allowedTypes.includes(item.type)) {
        return false
      }
      if (customFilter && !customFilter(item)) {
        return false
      }
      return true
    })

    return sortDevices(filtered, sort)
  }, [allowedTypes, currentDevice?.deviceId, customFilter, excludeSelf, onlineOnly, rawDevices, sort])

  const requestedDeviceId = useMemo(() => {
    if (controlledSelectedDeviceId !== undefined) {
      return controlledSelectedDeviceId
    }
    if (syncSearchParams) {
      return searchParams.get(searchParamKey) ?? ''
    }
    return ''
  }, [controlledSelectedDeviceId, searchParamKey, searchParams, syncSearchParams])

  const selectedDeviceId = useMemo(() => {
    if (requestedDeviceId && filteredAndSortedDevices.some((item) => item.deviceId === requestedDeviceId)) {
      return requestedDeviceId
    }
    return autoSelectFirst ? (filteredAndSortedDevices[0]?.deviceId ?? '') : ''
  }, [autoSelectFirst, filteredAndSortedDevices, requestedDeviceId])

  const selectedDevice = useMemo(
    () => filteredAndSortedDevices.find((item) => item.deviceId === selectedDeviceId) ?? null,
    [filteredAndSortedDevices, selectedDeviceId],
  )

  const selectDevice = useCallback(
    (deviceId: string) => {
      const target = filteredAndSortedDevices.find((item) => item.deviceId === deviceId)
      if (syncSearchParams) {
        setSearchParams(
          (prev) => {
            const next = new URLSearchParams(prev)
            if (deviceId) {
              next.set(searchParamKey, deviceId)
            } else {
              next.delete(searchParamKey)
            }
            return next
          },
          { replace: true },
        )
      }
      if (target) {
        onSelectDevice?.(deviceId, target)
      }
    },
    [filteredAndSortedDevices, onSelectDevice, searchParamKey, setSearchParams, syncSearchParams],
  )

  return {
    devices: filteredAndSortedDevices,
    selectedDeviceId,
    selectedDevice,
    selectDevice,
  }
}

export interface DeviceSidebarProps extends Partial<UseTargetDevicesOptions> {
  title?: ReactNode
  emptyText?: ReactNode
  devices?: DeviceInfo[]
  selectedDeviceId?: string
  onSelectDevice?: (deviceId: string, device: DeviceInfo) => void
  className?: string
  itemClassName?: string | ((device: DeviceInfo, isSelected: boolean) => string)
  renderItem?: (device: DeviceInfo, isSelected: boolean, onSelect: () => void) => ReactNode
  headerAction?: ReactNode
}

export function DeviceSidebar(props: DeviceSidebarProps) {
  const {
    title,
    emptyText,
    devices: controlledDevices,
    selectedDeviceId: controlledSelectedDeviceId,
    onSelectDevice: controlledOnSelectDevice,
    className,
    itemClassName,
    renderItem,
    headerAction,
    ...hookOptions
  } = props

  const { t } = useTranslation()

  // Use hook for auto-management when controlledDevices is not explicitly provided
  const hookResult = useTargetDevices({
    ...hookOptions,
    selectedDeviceId: controlledSelectedDeviceId,
    onSelectDevice: controlledOnSelectDevice,
  })

  const devices = controlledDevices ?? hookResult.devices
  const selectedDeviceId = controlledSelectedDeviceId !== undefined ? controlledSelectedDeviceId : hookResult.selectedDeviceId
  const handleSelect = controlledOnSelectDevice ?? hookResult.selectDevice

  return (
    <aside className={cn('h-full overflow-y-auto border-r py-6 pl-8 pr-4 scrollbar-thin', className)}>
      <div className="flex items-center justify-between pb-2 px-1">
        {title && (
          <div className="text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">
            {title}
          </div>
        )}
        {headerAction}
      </div>

      {devices.length === 0 ? (
        <div className="px-1 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
          {emptyText ?? t('common.empty', { defaultValue: 'No devices' })}
        </div>
      ) : (
        <div className="space-y-1">
          {devices.map((item) => {
            const isSelected = item.deviceId === selectedDeviceId
            const onSelect = () => handleSelect(item.deviceId, item)

            if (renderItem) {
              return renderItem(item, isSelected, onSelect)
            }

            const customClass = typeof itemClassName === 'function' ? itemClassName(item, isSelected) : itemClassName

            return (
              <button
                className={cn(
                  'w-full rounded-lg border px-3 py-2.5 text-left transition-all',
                  isSelected
                    ? 'border-[hsl(var(--text)/0.25)] bg-[hsl(var(--panel))] shadow-sm'
                    : 'border-transparent hover:bg-[hsl(var(--panel-2)/0.5)]',
                  customClass,
                )}
                key={item.deviceId}
                onClick={onSelect}
                type="button"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
                  <span
                    className={cn(
                      'h-1.5 w-1.5 shrink-0 rounded-full',
                      item.online ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--muted))]',
                    )}
                  />
                </div>
                <div className="mt-1 truncate text-[11px] text-[hsl(var(--muted))]">
                  {formatPlatformName(item.type, t)}
                </div>
              </button>
            )
          })}
        </div>
      )}
    </aside>
  )
}
