import type { FormEvent, ReactNode } from 'react'
import { useEffect, useState, useMemo } from 'react'
import { z } from 'zod'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { Switch } from '../components/ui/switch'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { buildTime, fallbackVersion, formatBuildTime, projectUrl, readAppVersion } from '../lib/app-meta'
import type { AppSettings } from '../lib/types'
import { cn } from '../lib/utils'
import { resolveLanguage } from '../i18n'

export function SettingsPage() {
  const { settings, saveSettings, pickDownloadDirectory } = useAppState()

  return (
    <SettingsForm
      key={JSON.stringify(settings)}
      onPickDownloadDirectory={pickDownloadDirectory}
      onSave={saveSettings}
      settings={settings}
    />
  )
}

const SUPPORTED_LANGUAGES = [
  { code: 'en', labelKey: 'settings.languages.en' },
  { code: 'zh-CN', labelKey: 'settings.languages.zhCN' },
  { code: 'zh-TW', labelKey: 'settings.languages.zhTW' },
  { code: 'ja', labelKey: 'settings.languages.ja' },
  { code: 'ko', labelKey: 'settings.languages.ko' },
  { code: 'es', labelKey: 'settings.languages.es' },
  { code: 'de', labelKey: 'settings.languages.de' },
  { code: 'ru', labelKey: 'settings.languages.ru' },
] as const

interface SettingsFormProps {
  settings: AppSettings
  onSave: (settings: AppSettings) => Promise<void>
  onPickDownloadDirectory: () => Promise<string | null>
}

function SettingsForm({ settings, onSave, onPickDownloadDirectory }: SettingsFormProps) {
  const { t, i18n } = useTranslation()
  const [form, setForm] = useState<AppSettings>(settings)
  const [version, setVersion] = useState(fallbackVersion)
  const { setSettingsDirty } = useAppState()

  const settingsSchema = useMemo(() => z.object({
    serverUrl: z.string().url(t('settings.validation.serverUrl')),
    autoStart: z.boolean(),
    startMinimized: z.boolean(),
    lanDiscovery: z.boolean(),
    downloadPath: z.string().min(1, t('settings.validation.downloadPath')),
    notifications: z.boolean(),
    clipboardSync: z.boolean(),
    language: z.string().min(1),
  }), [t])

  useEffect(() => {
    const isDirty = JSON.stringify(form) !== JSON.stringify(settings)
    setSettingsDirty(isDirty)

    return () => {
      setSettingsDirty(false)
    }
  }, [form, settings, setSettingsDirty])

  useEffect(() => {
    let cancelled = false

    void readAppVersion().then((nextVersion) => {
      if (!cancelled) {
        setVersion(nextVersion)
      }
    })

    return () => {
      cancelled = true
    }
  }, [])

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const parsed = settingsSchema.safeParse(form)
    if (!parsed.success) {
      toast.error(parsed.error.issues[0]?.message ?? t('settings.validation.incomplete'))
      return
    }
    try {
      await onSave(parsed.data)
      toast.success(t('settings.saveSuccess'))
    } catch (e) {
      toast.error(readErrorMessage(e))
    }
  }

  function handleLanguageChange(language: string) {
    const resolved = resolveLanguage(language)
    setForm((current) => ({ ...current, language: resolved }))
  }

  return (
    <div className="max-w-2xl animate-fade-in space-y-6">
      <form id="settings-form" className="space-y-6" onSubmit={handleSubmit}>
        <Section title={t('settings.general')}>
          <Field label="Server URL" tip={t('settings.serverTip')}>
            <Input onChange={(e) => setForm((c) => ({ ...c, serverUrl: e.target.value }))} value={form.serverUrl} />
          </Field>
          <Field label={t('settings.downloadPath')} tip={t('settings.downloadPathTip')}>
            <div className="flex gap-2">
              <Input onChange={(e) => setForm((c) => ({ ...c, downloadPath: e.target.value }))} value={form.downloadPath} />
              <Button
                onClick={async () => { const p = await onPickDownloadDirectory(); if (p) setForm((c) => ({ ...c, downloadPath: p })) }}
                type="button" variant="secondary" className="shrink-0"
              >{t('settings.select')}</Button>
            </div>
          </Field>
          <Field label={t('settings.language')} tip={t('settings.languageTip')}>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 md:grid-cols-4 max-w-xl">
              {SUPPORTED_LANGUAGES.map((lang) => (
                <button
                  key={lang.code}
                  type="button"
                  onClick={() => handleLanguageChange(lang.code)}
                  className={cn(
                    "flex items-center justify-center gap-1.5 rounded-lg border px-3 py-2 text-[12px] font-medium transition-all w-full",
                    resolveLanguage(form.language || i18n.language) === lang.code
                      ? "border-[hsl(var(--text))] bg-[hsl(var(--text)/0.06)] text-[hsl(var(--text))]"
                      : "border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
                  )}
                >
                  {t(lang.labelKey)}
                </button>
              ))}
            </div>
          </Field>
        </Section>

        <Section title={t('settings.behavior')}>
          <SwitchRow label={t('settings.autoStart')} checked={form.autoStart} onChange={(v) => setForm((c) => ({ ...c, autoStart: v }))} />
          <SwitchRow label={t('settings.startMinimized')} checked={form.startMinimized} onChange={(v) => setForm((c) => ({ ...c, startMinimized: v }))} />
          <SwitchRow label={t('settings.lanDiscovery')} checked={form.lanDiscovery} onChange={(v) => setForm((c) => ({ ...c, lanDiscovery: v }))} />
          <SwitchRow label={t('settings.notifications')} checked={form.notifications} onChange={(v) => setForm((c) => ({ ...c, notifications: v }))} />
        </Section>
      </form>

      <Section title={t('settings.about')}>
        <InfoRow label={t('settings.projectUrl')} value={projectUrl} />
        <InfoRow label={t('settings.version')} value={version} />
        <InfoRow label={t('settings.buildTime')} value={formatBuildTime(buildTime)} />
      </Section>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
      <div className="text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">{title}</div>
      <div className="mt-5 grid gap-5">{children}</div>
    </section>
  )
}

function Field({ label, tip, children }: { label: string; tip: string; children: ReactNode }) {
  return (
    <div className="block">
      <div className="text-[13px] font-medium">{label}</div>
      <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{tip}</div>
      <div className="mt-2">{children}</div>
    </div>
  )
}

function SwitchRow({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between py-1">
      <span className="text-[13px] font-medium">{label}</span>
      <Switch checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </div>
  )
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid gap-1">
      <div className="text-[11px] text-[hsl(var(--muted))]">{label}</div>
      <div className="break-all text-[13px] font-medium text-[hsl(var(--text))]">{value}</div>
    </div>
  )
}
