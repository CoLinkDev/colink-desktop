import { useMemo } from 'react'
import type { LucideIcon } from 'lucide-react'
import { Clipboard, Cloud, Computer, WifiOff } from 'lucide-react'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { Switch } from '../components/ui/switch'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { cn } from '../lib/utils'

export function ClipboardPage() {
  return <ClipboardForm />
}

function ClipboardForm() {
  const { t } = useTranslation()
  const { cloud, device, devices, saveSettings, settings } = useAppState()

  const relayDeviceCount = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId && item.cloudAvailable).length,
    [device?.deviceId, devices],
  )

  const availability = getAvailability(settings.clipboardSync, cloud.connected, relayDeviceCount)

  async function handleToggle(checked: boolean) {
    try {
      await saveSettings({ ...settings, clipboardSync: checked })
    } catch (error) {
      toast.error(readErrorMessage(error))
    }
  }

  return (
    <div className="max-w-2xl animate-fade-in space-y-6">
      <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 gap-3.5">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
              <Clipboard className="h-5 w-5 text-[hsl(var(--text-secondary))]" />
            </div>

            <div className="min-w-0">
              <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
                {t('nav.clipboard')}
              </div>
              <p className="mt-1 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
                {t('clipboard.description')}
              </p>
            </div>
          </div>

          <Switch
            aria-label={t('clipboard.enableSync')}
            checked={settings.clipboardSync}
            onChange={(event) => handleToggle(event.target.checked)}
          />
        </div>
      </section>

      <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
        <div className="text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">
          {t('clipboard.availability')}
        </div>

        <div className="mt-3 flex flex-col">
          <StatusRow
            icon={availability.icon}
            label={t('clipboard.currentStatus')}
            tone={availability.tone}
            value={t(`clipboard.status.${availability.key}`)}
          />
          <StatusRow
            icon={Cloud}
            label={t('clipboard.cloudRelay')}
            tone={cloud.connected ? 'success' : 'muted'}
            value={cloud.connected ? t('cloud.connected') : t('cloud.disconnected')}
          />
          <StatusRow
            icon={Computer}
            label={t('clipboard.relayDevices')}
            value={t('clipboard.relayDeviceCount', { count: relayDeviceCount })}
          />
        </div>
      </section>
    </div>
  )
}

function getAvailability(enabled: boolean, cloudConnected: boolean, relayDeviceCount: number) {
  if (!enabled) {
    return { icon: WifiOff, key: 'disabled', tone: 'muted' } as const
  }

  if (!cloudConnected) {
    return { icon: WifiOff, key: 'cloudDisconnected', tone: 'danger' } as const
  }

  if (relayDeviceCount === 0) {
    return { icon: Cloud, key: 'noRelayDevices', tone: 'muted' } as const
  }

  return { icon: Cloud, key: 'available', tone: 'success' } as const
}

function StatusRow({
  icon: Icon,
  label,
  tone = 'default',
  value,
}: {
  icon: LucideIcon
  label: string
  tone?: 'default' | 'success' | 'danger' | 'muted'
  value: string
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-3 border-b border-[hsl(var(--border)/0.3)] last:border-0">
      <div className="flex min-w-0 items-center gap-2.5">
        <Icon
          className={cn(
            'h-4 w-4 shrink-0',
            tone === 'success' && 'text-[hsl(var(--success))]',
            tone === 'danger' && 'text-[hsl(var(--danger))]',
            tone === 'muted' && 'text-[hsl(var(--muted))]',
            tone === 'default' && 'text-[hsl(var(--text-secondary))]',
          )}
        />
        <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{label}</span>
      </div>

      <span
        className={cn(
          'shrink-0 text-[13px] font-medium',
          tone === 'success' && 'text-[hsl(var(--success))]',
          tone === 'danger' && 'text-[hsl(var(--danger))]',
          tone === 'muted' && 'text-[hsl(var(--muted))]',
          tone === 'default' && 'text-[hsl(var(--text-secondary))]',
        )}
      >
        {value}
      </span>
    </div>
  )
}
