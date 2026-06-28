import type { FormEvent, ReactNode } from 'react'
import { useEffect, useState, useMemo } from 'react'
import { createPortal } from 'react-dom'
import { DndContext, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from '@dnd-kit/core'
import { SortableContext, arrayMove, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { z } from 'zod'
import { toast } from 'sonner'
import { Trans, useTranslation } from 'react-i18next'
import { GripVertical, RefreshCw, X } from 'lucide-react'

import { UpdateDialog } from '../components/update-dialog'
import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { Switch } from '../components/ui/switch'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { buildTime, fallbackVersion, formatBuildTime, isReleaseBuild, projectUrl, readAppVersion } from '../lib/app-meta'
import { checkUpdate, getMusicProviders, listAvailableMusicProviders, updateMusicProviders } from '../lib/api'
import type { AppSettings, AppUpdateRelease, MusicProviderConfig, MusicProviderMeta } from '../lib/types'
import { isBreakingVersionUpdate } from '../lib/update-policy'
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

interface ProviderItem extends MusicProviderConfig {
  name: string
  implemented: boolean
}

function SettingsForm({ settings, onSave, onPickDownloadDirectory }: SettingsFormProps) {
  const { t, i18n } = useTranslation()
  const [form, setForm] = useState<AppSettings>(settings)
  const [providerItems, setProviderItems] = useState<ProviderItem[]>([])
  const [initialProviderItems, setInitialProviderItems] = useState<ProviderItem[]>([])
  const [providersLoading, setProvidersLoading] = useState(true)
  const [providersLoadError, setProvidersLoadError] = useState<string | null>(null)
  const [showNcmHelp, setShowNcmHelp] = useState(false)
  const [version, setVersion] = useState(fallbackVersion)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdateRelease | null>(null)
  const { setSettingsDirty } = useAppState()
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }))
  const formDirty = useMemo(() => JSON.stringify(form) !== JSON.stringify(settings), [form, settings])
  const providersDirty = useMemo(
    () => serializeProviderItems(providerItems) !== serializeProviderItems(initialProviderItems),
    [initialProviderItems, providerItems],
  )
  const requiredUpdate =
    availableUpdate != null &&
    isReleaseBuild &&
    availableUpdate.assets.length > 0 &&
    isBreakingVersionUpdate(availableUpdate.version, version)

  const settingsSchema = useMemo(() => z.object({
    serverUrl: z.string().url(t('settings.validation.serverUrl')),
    autoStart: z.boolean(),
    startMinimized: z.boolean(),
    downloadPath: z.string().min(1, t('settings.validation.downloadPath')),
    clipboardSync: z.boolean(),
    language: z.string().min(1),
  }), [t])

  useEffect(() => {
    setSettingsDirty(formDirty || providersDirty)

    return () => {
      setSettingsDirty(false)
    }
  }, [formDirty, providersDirty, setSettingsDirty])

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

  useEffect(() => {
    let cancelled = false

    async function loadProviders() {
      setProvidersLoading(true)
      setProvidersLoadError(null)
      try {
        const [available, configured] = await Promise.all([
          listAvailableMusicProviders(),
          getMusicProviders(),
        ])
        if (cancelled) return
        const merged = mergeProviders(available, configured)
        setProviderItems(merged)
        setInitialProviderItems(merged)
      } catch (error) {
        if (!cancelled) {
          setProvidersLoadError(readErrorMessage(error))
        }
      } finally {
        if (!cancelled) {
          setProvidersLoading(false)
        }
      }
    }

    void loadProviders()

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
      if (formDirty) {
        await onSave(parsed.data)
      }
      if (providersDirty) {
        const providers = normalizeProviderPriorities(providerItems).map(({ id, enabled, priority }) => ({
          id,
          enabled,
          priority,
        }))
        await updateMusicProviders(providers)
        const nextItems = normalizeProviderPriorities(providerItems)
        setProviderItems(nextItems)
        setInitialProviderItems(nextItems)
      }
      toast.success(i18n.t('settings.saveSuccess'))
    } catch (e) {
      toast.error(readErrorMessage(e))
    }
  }

  function handleLanguageChange(language: string) {
    const resolved = resolveLanguage(language)
    setForm((current) => ({ ...current, language: resolved }))
  }

  async function handleCheckUpdate() {
    setCheckingUpdate(true)
    try {
      const update = await checkUpdate()
      if (!update) {
        toast.success(t('updates.upToDate'))
        return
      }

      setAvailableUpdate(update)
    } catch (e) {
      toast.error(readErrorMessage(e))
    } finally {
      setCheckingUpdate(false)
    }
  }

  function handleProviderDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return

    setProviderItems((current) => {
      const oldIndex = current.findIndex((item) => item.id === active.id)
      const newIndex = current.findIndex((item) => item.id === over.id)
      if (oldIndex < 0 || newIndex < 0) return current
      return normalizeProviderPriorities(arrayMove(current, oldIndex, newIndex))
    })
  }

  function handleProviderToggle(id: string, enabled: boolean) {
    setProviderItems((current) =>
      normalizeProviderPriorities(
        current.map((item) =>
          item.id === id ? { ...item, enabled: item.implemented && enabled } : item,
        ),
      ),
    )
  }

  return (
    <div className="max-w-2xl animate-fade-in space-y-6">
      <UpdateDialog
        update={availableUpdate}
        required={requiredUpdate}
        onClose={() => {
          if (!requiredUpdate) {
            setAvailableUpdate(null)
          }
        }}
      />
      <form id="settings-form" className="space-y-6" onSubmit={handleSubmit}>
        <Section title={t('settings.general')}>
          <Field label={t('settings.serverUrl')} tip={t('settings.serverTip')}>
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
        </Section>

        <Section title={t('nowPlaying.title')}>
          <div className="text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
            {t('nowPlaying.description')}
          </div>

          {providersLoading && (
            <div className="rounded-lg border border-dashed px-4 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
              {t('common.loading')}
            </div>
          )}

          {!providersLoading && providersLoadError && (
            <div className="rounded-lg border border-[hsl(var(--danger)/0.2)] bg-[hsl(var(--danger)/0.08)] px-4 py-3 text-[13px] text-[hsl(var(--danger))]">
              {providersLoadError}
            </div>
          )}

          {!providersLoading && !providersLoadError && providerItems.length === 0 && (
            <div className="rounded-lg border border-dashed px-4 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
              {t('nowPlaying.empty')}
            </div>
          )}

          {!providersLoading && !providersLoadError && providerItems.length > 0 && (
            <DndContext collisionDetection={closestCenter} onDragEnd={handleProviderDragEnd} sensors={sensors}>
              <SortableContext items={providerItems.map((item) => item.id)} strategy={verticalListSortingStrategy}>
                <div className="grid gap-2">
                  {providerItems.map((item) => (
                    <ProviderRow
                      key={item.id}
                      item={item}
                      onShowNcmHelp={() => setShowNcmHelp(true)}
                      onToggle={handleProviderToggle}
                    />
                  ))}
                </div>
              </SortableContext>
            </DndContext>
          )}
        </Section>
      </form>

      {showNcmHelp && <NcmHelpDialog onClose={() => setShowNcmHelp(false)} />}

      <Section title={t('settings.about')}>
        <InfoRow label={t('settings.projectUrl')} value={projectUrl} />
        <InfoRow label={t('settings.version')} value={version} />
        <InfoRow label={t('settings.buildTime')} value={formatBuildTime(buildTime)} />
        <Button className="w-fit" disabled={checkingUpdate} onClick={handleCheckUpdate} variant="secondary">
          <RefreshCw className={cn("size-4", checkingUpdate && "animate-spin")} />
          {checkingUpdate ? t('updates.checking') : t('updates.check')}
        </Button>
      </Section>
    </div>
  )
}

function NcmHelpDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
      <div className="w-full max-w-lg rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
        <div className="flex items-center justify-between gap-3">
          <div className="text-[16px] font-semibold text-[hsl(var(--text))]">
            {t('nowPlaying.ncmHelpTitle')}
          </div>
          <button
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            onClick={onClose}
            title={t('common.close')}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        <p className="mt-4 whitespace-pre-line break-words text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          <Trans
            components={{
              code: <code className="rounded bg-[hsl(var(--panel-2))] px-1.5 py-0.5 font-mono text-[12px] text-[hsl(var(--text))]" />,
            }}
            i18nKey="nowPlaying.ncmHelpMessage"
          />
        </p>
        <div className="mt-6 flex justify-end">
          <Button onClick={onClose} variant="primary">
            {t('common.confirm')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}

function ProviderRow({
  item,
  onShowNcmHelp,
  onToggle,
}: {
  item: ProviderItem
  onShowNcmHelp: () => void
  onToggle: (id: string, enabled: boolean) => void
}) {
  const { t } = useTranslation()
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: item.id })

  return (
    <div
      ref={setNodeRef}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
      }}
      className={cn(
        'flex min-h-[54px] items-center gap-3 rounded-lg border bg-[hsl(var(--panel))] px-3 py-2',
        isDragging && 'z-10 shadow-lg',
        !item.implemented && 'opacity-60',
      )}
    >
      <button
        {...attributes}
        {...listeners}
        aria-label={item.name}
        className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
        type="button"
      >
        <GripVertical className="h-4 w-4" />
      </button>

      <div className="flex min-w-0 flex-1 items-center gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">
            {t(providerNameKey(item.id), { defaultValue: item.name })}
          </span>
          {item.id === 'ncm' && (
            <button
              className="shrink-0 text-[11px] text-[hsl(var(--muted))] underline underline-offset-2 hover:text-[hsl(var(--text))]"
              onClick={onShowNcmHelp}
              type="button"
            >
              {t('nowPlaying.ncmHelpTitle')}
            </button>
          )}
          {!item.implemented && (
            <span className="shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-[hsl(var(--muted))]">
              {t('nowPlaying.comingSoon')}
            </span>
          )}
        </div>
      </div>

      <Switch
        aria-label={item.name}
        checked={item.enabled}
        disabled={!item.implemented}
        onChange={(event) => onToggle(item.id, event.target.checked)}
      />
    </div>
  )
}

function mergeProviders(available: MusicProviderMeta[], configured: MusicProviderConfig[]) {
  const metaById = new Map(available.map((item) => [item.id, item]))
  const configuredById = new Map(configured.map((item) => [item.id, item]))
  const ordered: ProviderItem[] = []

  for (const config of [...configured].sort((a, b) => a.priority - b.priority)) {
    const meta = metaById.get(config.id)
    if (!meta) continue
    ordered.push({
      id: meta.id,
      name: meta.name,
      implemented: meta.implemented,
      enabled: meta.implemented && config.enabled,
      priority: ordered.length,
    })
  }

  for (const meta of available) {
    if (configuredById.has(meta.id)) continue
    ordered.push({
      id: meta.id,
      name: meta.name,
      implemented: meta.implemented,
      enabled: false,
      priority: ordered.length,
    })
  }

  return normalizeProviderPriorities(ordered)
}

function normalizeProviderPriorities<T extends MusicProviderConfig>(items: T[]) {
  return items.map((item, index) => ({ ...item, priority: index }))
}

function serializeProviderItems(items: ProviderItem[]) {
  return JSON.stringify(items.map(({ id, enabled, priority }) => ({ id, enabled, priority })))
}

function providerNameKey(id: string) {
  return `nowPlaying.providers.${id}`
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
