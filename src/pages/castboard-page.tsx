import { useCallback, useEffect, useMemo, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Monitor, Play, RefreshCw, Square } from 'lucide-react'
import { toast } from 'sonner'
import { useTranslation } from 'react-i18next'

import { getCastBoardStatus, listCastBoardMonitors, openCastBoardOnMonitor, stopCastBoard } from '../lib/api'
import type { CastBoardMonitor, CastBoardStatus } from '../lib/types'
import { Button } from '../components/ui/button'
import { cn } from '../lib/utils'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

const initialCastBoardStatus: CastBoardStatus = {
  state: 'closed',
  monitor: null,
  message: null,
}

export function CastBoardPage() {
  const { t } = useTranslation()
  const { setHeaderActions } = useAppState()
  const [monitors, setMonitors] = useState<CastBoardMonitor[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [opening, setOpening] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [status, setStatus] = useState<CastBoardStatus>(initialCastBoardStatus)

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

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        const current = await getCastBoardStatus()
        if (!disposed) {
          setStatus(current)
        }
        unlisten = await listen<CastBoardStatus>('castboard-status', (event) => {
          if (!disposed) {
            setStatus(event.payload)
          }
        })
      } catch (error) {
        if (!disposed) {
          toast.error(readErrorMessage(error))
        }
      }
    })()

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

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
      <div className="rounded-xl border bg-[hsl(var(--panel))] px-4 py-3">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <div className="text-[13px] font-medium text-[hsl(var(--text))]">
              {t(`castboard.status.${status.state}`)}
            </div>
            <div className="mt-1 truncate text-[12px] text-[hsl(var(--muted))]">
              {status.monitor
                ? t('castboard.statusMonitor', { name: status.monitor.name })
                : t('castboard.statusNoMonitor')}
            </div>
          </div>
          <div
            className={cn(
              'h-2.5 w-2.5 shrink-0 rounded-full',
              status.state === 'open' && 'bg-[hsl(var(--success))]',
              status.state === 'opening' && 'bg-[hsl(var(--accent))]',
              status.state === 'closing' && 'bg-[hsl(var(--accent))]',
              status.state === 'failed' && 'bg-[hsl(var(--danger))]',
              status.state === 'closed' && 'bg-[hsl(var(--muted))]',
            )}
          />
        </div>
        {status.state === 'failed' && status.message && (
          <div className="mt-3 rounded-lg border border-[hsl(var(--danger)/0.2)] bg-[hsl(var(--danger)/0.08)] px-3 py-2 text-[12px] text-[hsl(var(--danger))]">
            {status.message}
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
    </div>
  )
}
