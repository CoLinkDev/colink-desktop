import type { FormEvent } from 'react'
import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { DndContext, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from '@dnd-kit/core'
import { SortableContext, arrayMove, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { GripVertical, Music2, X } from 'lucide-react'
import { useBlocker } from 'react-router-dom'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { Switch } from '../components/ui/switch'
import { readErrorMessage } from '../hooks/use-app-state'
import { getMusicProviders, listAvailableMusicProviders, updateMusicProviders } from '../lib/api'
import type { MusicProviderConfig, MusicProviderMeta } from '../lib/types'
import { cn } from '../lib/utils'

interface ProviderItem extends MusicProviderConfig {
  name: string
  implemented: boolean
}

export function NowPlayingPage() {
  const { t } = useTranslation()
  const [items, setItems] = useState<ProviderItem[]>([])
  const [initialItems, setInitialItems] = useState<ProviderItem[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [showNcmHelp, setShowNcmHelp] = useState(false)

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }))
  const dirty = useMemo(() => serializeItems(items) !== serializeItems(initialItems), [initialItems, items])
  const blocker = useBlocker(({ nextLocation, currentLocation }) => dirty && nextLocation.pathname !== currentLocation.pathname)

  useEffect(() => {
    let cancelled = false

    async function load() {
      setLoading(true)
      setLoadError(null)
      try {
        const [available, configured] = await Promise.all([
          listAvailableMusicProviders(),
          getMusicProviders(),
        ])
        if (cancelled) return
        const merged = mergeProviders(available, configured)
        setItems(merged)
        setInitialItems(merged)
      } catch (error) {
        if (!cancelled) {
          setLoadError(readErrorMessage(error))
        }
      } finally {
        if (!cancelled) {
          setLoading(false)
        }
      }
    }

    void load()

    return () => {
      cancelled = true
    }
  }, [])

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return

    setItems((current) => {
      const oldIndex = current.findIndex((item) => item.id === active.id)
      const newIndex = current.findIndex((item) => item.id === over.id)
      if (oldIndex < 0 || newIndex < 0) return current
      return normalizePriorities(arrayMove(current, oldIndex, newIndex))
    })
  }

  function handleToggle(id: string, enabled: boolean) {
    setItems((current) =>
      normalizePriorities(
        current.map((item) =>
          item.id === id ? { ...item, enabled: item.implemented && enabled } : item,
        ),
      ),
    )
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    try {
      const providers = normalizePriorities(items).map(({ id, enabled, priority }) => ({
        id,
        enabled,
        priority,
      }))
      await updateMusicProviders(providers)
      const nextItems = normalizePriorities(items)
      setItems(nextItems)
      setInitialItems(nextItems)
      toast.success(t('nowPlaying.saveSuccess'))
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="max-w-2xl animate-fade-in space-y-6">
      <form id="now-playing-form" className="space-y-6" onSubmit={handleSubmit}>
        <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
          <div className="flex items-start gap-3.5">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
              <Music2 className="h-5 w-5 text-[hsl(var(--text-secondary))]" />
            </div>
            <div className="min-w-0">
              <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
                {t('nowPlaying.title')}
              </div>
              <p className="mt-1 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
                {t('nowPlaying.description')}
              </p>
              <div className="mt-2 text-[12px] text-[hsl(var(--muted))]">
                <button
                  className="underline underline-offset-2 hover:text-[hsl(var(--text))]"
                  onClick={() => setShowNcmHelp(true)}
                  type="button"
                >
                  {t('nowPlaying.ncmHelpTitle')}
                </button>
              </div>
            </div>
          </div>

          <div className="mt-5">
            {loading && (
              <div className="rounded-lg border border-dashed px-4 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
                {t('common.loading')}
              </div>
            )}

            {!loading && loadError && (
              <div className="rounded-lg border border-[hsl(var(--danger)/0.2)] bg-[hsl(var(--danger)/0.08)] px-4 py-3 text-[13px] text-[hsl(var(--danger))]">
                {loadError}
              </div>
            )}

            {!loading && !loadError && items.length === 0 && (
              <div className="rounded-lg border border-dashed px-4 py-8 text-center text-[13px] text-[hsl(var(--muted))]">
                {t('nowPlaying.empty')}
              </div>
            )}

            {!loading && !loadError && items.length > 0 && (
              <DndContext collisionDetection={closestCenter} onDragEnd={handleDragEnd} sensors={sensors}>
                <SortableContext items={items.map((item) => item.id)} strategy={verticalListSortingStrategy}>
                  <div className="grid gap-2">
                    {items.map((item) => (
                      <ProviderRow
                        key={item.id}
                        item={item}
                        onToggle={handleToggle}
                      />
                    ))}
                  </div>
                </SortableContext>
              </DndContext>
            )}
          </div>
        </section>
      </form>

      {blocker.state === 'blocked' && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('settings.unsavedChangesTitle')}</div>
            <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
              {t('settings.unsavedChangesDesc')}
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button onClick={() => blocker.reset()} variant="secondary">
                {t('common.cancel')}
              </Button>
              <Button onClick={() => blocker.proceed()} variant="danger">
                {t('settings.leave')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {showNcmHelp && <NcmHelpDialog onClose={() => setShowNcmHelp(false)} />}
    </div>
  )
}

function NcmHelpDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
      <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
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
        <p className="mt-4 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          {t('nowPlaying.ncmHelpMessage')}
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

function ProviderRow({ item, onToggle }: { item: ProviderItem; onToggle: (id: string, enabled: boolean) => void }) {
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

      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
          {!item.implemented && (
            <span className="shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-[hsl(var(--muted))]">
              {t('nowPlaying.comingSoon')}
            </span>
          )}
        </div>
        <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{item.id}</div>
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

  return normalizePriorities(ordered)
}

function normalizePriorities<T extends MusicProviderConfig>(items: T[]) {
  return items.map((item, index) => ({ ...item, priority: index }))
}

function serializeItems(items: ProviderItem[]) {
  return JSON.stringify(items.map(({ id, enabled, priority }) => ({ id, enabled, priority })))
}
