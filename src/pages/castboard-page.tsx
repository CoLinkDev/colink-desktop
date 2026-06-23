import { useCallback, useEffect, useMemo, useState } from 'react'
import { Monitor, Play, RefreshCw } from 'lucide-react'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { listCastBoardMonitors, openCastBoardOnMonitor } from '../lib/api'
import type { CastBoardMonitor } from '../lib/types'
import { Button } from '../components/ui/button'
import { cn } from '../lib/utils'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

export function CastBoardPage() {
  const { t } = useTranslation()
  const { setHeaderActions } = useAppState()
  const [monitors, setMonitors] = useState<CastBoardMonitor[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [opening, setOpening] = useState(false)

  const selectedMonitor = useMemo(
    () => monitors.find((monitor) => monitor.id === selectedId) ?? null,
    [monitors, selectedId],
  )

  const refreshMonitors = useCallback(async () => {
    setLoading(true)
    try {
      const next = await listCastBoardMonitors()
      setMonitors(next)
      setSelectedId((current) => {
        if (current && next.some((monitor) => monitor.id === current)) {
          return current
        }
        return next[0]?.id ?? null
      })
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setLoading(false)
    }
  }, [])

  async function handleOpen() {
    if (!selectedMonitor) {
      return
    }
    setOpening(true)
    try {
      await openCastBoardOnMonitor(selectedMonitor.id)
      toast.success(t('castboard.started'))
    } catch (error) {
      toast.error(readErrorMessage(error))
    } finally {
      setOpening(false)
    }
  }

  useEffect(() => {
    void refreshMonitors()
  }, [refreshMonitors])

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
    <div className="flex max-w-2xl flex-col gap-4 animate-fade-in">
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
                <div className="mt-2 text-[12px] text-[hsl(var(--muted))]">
                  {monitor.width} x {monitor.height}
                </div>
                <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">
                  {t('castboard.position', { x: monitor.x, y: monitor.y })}
                </div>
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

      <div className="flex justify-end">
        <Button disabled={!selectedMonitor || opening} onClick={handleOpen}>
          <Play className="h-4 w-4" />
          {opening ? t('castboard.starting') : t('castboard.start')}
        </Button>
      </div>
    </div>
  )
}
