import { useCallback, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { listen } from '@tauri-apps/api/event'
import { Monitor, Play, Puzzle, RefreshCw, Settings, Square, Trash2, Upload } from 'lucide-react'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import {
  deleteCastBoardPlugin,
  getCastBoardStatus,
  listCastBoardMonitors,
  listCastBoardPlugins,
  openCastBoardOnMonitor,
  pickCastBoardPlugin,
  stopCastBoard,
  toggleCastBoardPlugin,
} from '../lib/api'
import type { CastBoardMonitor, CastBoardPlugin, CastBoardStatus } from '../lib/types'
import { Button } from '../components/ui/button'
import { Switch } from '../components/ui/switch'
import { PluginConfigDialog } from '../components/castboard/plugin-config-dialog'
import { cn } from '../lib/utils'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

const initialCastBoardStatus: CastBoardStatus = {
  state: 'closed',
  monitor: null,
  message: null,
}

const pluginErrorKeys: Record<string, string> = {
  castboard_plugin_invalid_archive: 'castboard.plugins.errors.invalidArchive',
  castboard_plugin_invalid_manifest: 'castboard.plugins.errors.invalidManifest',
  castboard_plugin_incompatible: 'castboard.plugins.errors.incompatible',
  castboard_plugin_storage_error: 'castboard.plugins.errors.storage',
  castboard_plugin_not_found: 'castboard.plugins.errors.notFound',
}

function localizedText(values: Record<string, string> | null, language: string): string {
  if (!values) return ''
  const normalizedLanguage = language.toLowerCase()
  const exact = Object.entries(values).find(([key]) => key.toLowerCase() === normalizedLanguage)?.[1]
  if (exact) return exact
  const baseLanguage = normalizedLanguage.split('-')[0]
  const base = Object.entries(values).find(([key]) => key.toLowerCase() === baseLanguage)?.[1]
  return base ?? values.en ?? Object.values(values)[0] ?? ''
}

export function CastBoardPage() {
  const { t, i18n } = useTranslation()
  const { setHeaderActions } = useAppState()
  const [monitors, setMonitors] = useState<CastBoardMonitor[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [opening, setOpening] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [status, setStatus] = useState<CastBoardStatus>(initialCastBoardStatus)
  const [plugins, setPlugins] = useState<CastBoardPlugin[]>([])
  const [pluginsLoading, setPluginsLoading] = useState(true)
  const [importing, setImporting] = useState(false)
  const [actingPluginId, setActingPluginId] = useState<string | null>(null)
  const [deletePlugin, setDeletePlugin] = useState<CastBoardPlugin | null>(null)
  const [configPlugin, setConfigPlugin] = useState<CastBoardPlugin | null>(null)
  const pluginsBusy = importing || actingPluginId !== null

  const selectedMonitor = useMemo(
    () => monitors.find((monitor) => monitor.id === selectedId) ?? null,
    [monitors, selectedId],
  )
  const language = i18n.resolvedLanguage ?? i18n.language

  const pluginErrorMessage = useCallback((error: unknown) => {
    const code = typeof error === 'string' ? error : ''
    const key = pluginErrorKeys[code]
    return key ? t(key) : t('castboard.plugins.errors.unknown')
  }, [t])

  const refreshMonitors = useCallback(async () => {
    setLoading(true)
    try {
      const next = await listCastBoardMonitors()
      setMonitors(next)
      setSelectedId((current) => {
        if (current && next.some((monitor) => monitor.id === current)) return current
        return next[0]?.id ?? null
      })
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setLoading(false)
    }
  }, [])

  const refreshPlugins = useCallback(async () => {
    setPluginsLoading(true)
    try {
      setPlugins(await listCastBoardPlugins())
    } catch (error) {
      toast.error(pluginErrorMessage(error))
    } finally {
      setPluginsLoading(false)
    }
  }, [pluginErrorMessage])

  async function handleOpen() {
    if (!selectedMonitor) return
    setOpening(true)
    try {
      await openCastBoardOnMonitor(selectedMonitor.id, language)
      toast.success(t('castboard.started'))
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setOpening(false)
    }
  }

  async function handleStop() {
    setStopping(true)
    try {
      await stopCastBoard()
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setStopping(false)
    }
  }

  async function handleImportPlugin() {
    setImporting(true)
    try {
      const plugin = await pickCastBoardPlugin()
      if (!plugin) return
      await refreshPlugins()
      toast.success(t('castboard.plugins.imported', { name: localizedText(plugin.name, language) }))
    } catch (error) {
      toast.error(pluginErrorMessage(error))
    } finally {
      setImporting(false)
    }
  }

  async function handleTogglePlugin(plugin: CastBoardPlugin, enabled: boolean) {
    setActingPluginId(plugin.id)
    try {
      await toggleCastBoardPlugin(plugin.id, enabled)
      setPlugins((current) => current.map((item) => item.id === plugin.id ? { ...item, enabled } : item))
    } catch (error) {
      toast.error(pluginErrorMessage(error))
    } finally {
      setActingPluginId(null)
    }
  }

  async function handleDeletePlugin() {
    if (!deletePlugin) return
    setActingPluginId(deletePlugin.id)
    try {
      await deleteCastBoardPlugin(deletePlugin.id)
      setPlugins((current) => current.filter((plugin) => plugin.id !== deletePlugin.id))
      toast.success(t('castboard.plugins.deleted'))
      setDeletePlugin(null)
    } catch (error) {
      toast.error(pluginErrorMessage(error))
    } finally {
      setActingPluginId(null)
    }
  }

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null
    void (async () => {
      try {
        const current = await getCastBoardStatus()
        if (!disposed) setStatus(current)
        unlisten = await listen<CastBoardStatus>('castboard-status', (event) => {
          if (!disposed) setStatus(event.payload)
        })
      } catch (error) {
        if (!disposed) toast.error(readErrorMessage(error))
      }
    })()
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    void refreshMonitors()
    void refreshPlugins()
  }, [refreshMonitors, refreshPlugins])

  useEffect(() => {
    setHeaderActions(
      <Button disabled={loading} onClick={refreshMonitors} size="sm" variant="secondary">
        <RefreshCw className={cn('h-3.5 w-3.5', loading && 'animate-spin')} />
        {t('castboard.refreshDisplays')}
      </Button>,
    )
    return () => setHeaderActions(null)
  }, [loading, refreshMonitors, setHeaderActions, t])

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <div className="rounded-xl border bg-[hsl(var(--panel))] px-4 py-3">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <div className="text-[13px] font-medium text-[hsl(var(--text))]">{t(`castboard.status.${status.state}`)}</div>
            <div className="mt-1 truncate text-[12px] text-[hsl(var(--muted))]">
              {status.monitor ? t('castboard.statusMonitor', { name: status.monitor.name }) : t('castboard.statusNoMonitor')}
            </div>
          </div>
          <div className={cn(
            'h-2.5 w-2.5 shrink-0 rounded-full',
            status.state === 'open' && 'bg-[hsl(var(--success))]',
            (status.state === 'opening' || status.state === 'closing') && 'bg-[hsl(var(--accent))]',
            status.state === 'failed' && 'bg-[hsl(var(--danger))]',
            status.state === 'closed' && 'bg-[hsl(var(--muted))]',
          )} />
        </div>
        {status.state === 'failed' && status.message && (
          <div className="mt-3 rounded-lg border border-[hsl(var(--danger)/0.2)] bg-[hsl(var(--danger)/0.08)] px-3 py-2 text-[12px] text-[hsl(var(--danger))]">
            {t('castboard.startFailed')}
          </div>
        )}
      </div>

      <div className="grid gap-3 md:grid-cols-2">
        {monitors.map((monitor) => {
          const active = monitor.id === selectedId
          return (
            <button
              className={cn(
                'flex min-h-[110px] items-start gap-3 rounded-lg border bg-[hsl(var(--panel))] p-4 text-left transition-colors hover:bg-[hsl(var(--panel-2))]',
                active && 'border-[hsl(var(--accent))] bg-[hsl(var(--panel-2))]',
              )}
              key={monitor.id}
              onClick={() => setSelectedId(monitor.id)}
              type="button"
            >
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
                <Monitor className="h-5 w-5" />
              </div>
              <div className="min-w-0">
                <div className="truncate text-[14px] font-medium text-[hsl(var(--text))]">{monitor.name}</div>
                <div className="mt-2 text-[12px] text-[hsl(var(--muted))]">{monitor.width} x {monitor.height}</div>
                <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">{t('castboard.position', { x: monitor.x, y: monitor.y })}</div>
              </div>
            </button>
          )
        })}
      </div>

      {!loading && monitors.length === 0 && (
        <div className="rounded-lg border bg-[hsl(var(--panel))] py-12 text-center text-[13px] text-[hsl(var(--muted))]">
          {t('castboard.empty')}
        </div>
      )}

      <div className="flex justify-start gap-2">
        <Button disabled={!selectedMonitor || opening || status.state === 'opening'} onClick={handleOpen}>
          <Play className="h-4 w-4" />
          {opening || status.state === 'opening' ? t('castboard.starting') : t('castboard.start')}
        </Button>
        {(status.state === 'open' || status.state === 'closing') && (
          <Button disabled={stopping || status.state === 'closing'} onClick={handleStop} variant="secondary">
            <Square className="h-4 w-4" />
            {stopping || status.state === 'closing' ? t('castboard.stopping') : t('castboard.stop')}
          </Button>
        )}
      </div>

      <section className="mt-2 rounded-xl border bg-[hsl(var(--panel))] p-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h2 className="text-[14px] font-semibold text-[hsl(var(--text))]">{t('castboard.plugins.title')}</h2>
            <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">{t('castboard.plugins.description')}</p>
          </div>
          <Button disabled={pluginsBusy} onClick={handleImportPlugin} size="sm" variant="secondary">
            <Upload className="h-3.5 w-3.5" />
            {importing ? t('castboard.plugins.importing') : t('castboard.plugins.import')}
          </Button>
        </div>

        <div className="mt-4 flex flex-col gap-2">
          {plugins.map((plugin) => {
            const name = localizedText(plugin.name, language)
            const description = localizedText(plugin.description, language)
            return (
              <div className="flex items-center gap-3 rounded-lg border bg-[hsl(var(--panel-2))] p-3" key={plugin.id}>
                <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
                  <Puzzle className="h-5 w-5" />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{name}</span>
                    <span className="rounded-full bg-[hsl(var(--accent)/0.12)] px-2 py-0.5 text-[10px] text-[hsl(var(--accent))]">
                      {t(`castboard.plugins.types.${plugin.type}`)}
                    </span>
                    <span className="text-[11px] text-[hsl(var(--muted))]">v{plugin.version}</span>
                  </div>
                  {description && <p className="mt-1 line-clamp-2 text-[12px] text-[hsl(var(--muted))]">{description}</p>}
                </div>
                {plugin.configSchema && (
                  <Button
                    aria-label={t('castboard.plugins.configure', { name })}
                    className="h-8 w-8 p-0"
                    disabled={pluginsBusy}
                    onClick={() => setConfigPlugin(plugin)}
                    title={t('castboard.plugins.configure', { name })}
                    variant="ghost"
                  >
                    <Settings className="h-4 w-4" />
                  </Button>
                )}
                <Switch
                  aria-label={t('castboard.plugins.toggle', { name })}
                  checked={plugin.enabled}
                  disabled={pluginsBusy}
                  onChange={(event) => { void handleTogglePlugin(plugin, event.target.checked) }}
                />
                <Button
                  aria-label={t('castboard.plugins.delete')}
                  className="h-8 w-8 p-0"
                  disabled={pluginsBusy}
                  onClick={() => setDeletePlugin(plugin)}
                  title={t('castboard.plugins.delete')}
                  variant="ghost"
                >
                  <Trash2 className="h-4 w-4 text-[hsl(var(--danger))]" />
                </Button>
              </div>
            )
          })}
          {!pluginsLoading && plugins.length === 0 && (
            <div className="rounded-lg border border-dashed py-8 text-center text-[12px] text-[hsl(var(--muted))]">
              {t('castboard.plugins.empty')}
            </div>
          )}
          {pluginsLoading && (
            <div className="py-8 text-center text-[12px] text-[hsl(var(--muted))]">{t('common.loading')}</div>
          )}
        </div>
      </section>

      {deletePlugin && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm animate-fade-in">
          <div aria-modal="true" className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in" role="dialog">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('castboard.plugins.deleteTitle')}</div>
            <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
              {t('castboard.plugins.deleteDescription', { name: localizedText(deletePlugin.name, language) })}
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button disabled={actingPluginId === deletePlugin.id} onClick={() => setDeletePlugin(null)} variant="secondary">
                {t('common.cancel')}
              </Button>
              <Button disabled={actingPluginId === deletePlugin.id} onClick={handleDeletePlugin} variant="danger">
                {t('castboard.plugins.delete')}
              </Button>
            </div>
          </div>
        </div>,
        document.body,
      )}
      {configPlugin && (
        <PluginConfigDialog
          language={language}
          onClose={() => setConfigPlugin(null)}
          plugin={configPlugin}
        />
      )}
    </div>
  )
}
