import { ChevronLeft, ChevronRight } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { listLogs } from '../lib/api'
import type { AppLogEntry } from '../lib/types'
import { cn } from '../lib/utils'
import { formatTimestamp } from '../lib/utils'

const LOGS_PER_PAGE = 20

export function LogsPage() {
  const { logs: liveLogs } = useAppState()
  const { t, i18n } = useTranslation()
  const [page, setPage] = useState(1)
  const [pageLogs, setPageLogs] = useState<AppLogEntry[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const totalPages = Math.max(1, Math.ceil(total / LOGS_PER_PAGE))

  useEffect(() => {
    let disposed = false

    setLoading(true)
    void listLogs(page, LOGS_PER_PAGE)
      .then((result) => {
        if (disposed) return

        const nextTotalPages = Math.max(1, Math.ceil(result.total / LOGS_PER_PAGE))
        setTotal(result.total)
        if (page > nextTotalPages) {
          setPage(nextTotalPages)
          return
        }
        setPageLogs(result.logs)
      })
      .catch((error) => {
        if (!disposed) {
          toast.error(readErrorMessage(error))
        }
      })
      .finally(() => {
        if (!disposed) {
          setLoading(false)
        }
      })

    return () => {
      disposed = true
    }
  }, [page, liveLogs])

  return (
    <div className="animate-fade-in">
      <div className="rounded-xl border bg-[hsl(var(--panel))]">
        {!loading && total === 0 ? (
          <div className="py-16 text-center text-[13px] text-[hsl(var(--muted))]">{t('logs.empty')}</div>
        ) : (
          <>
            <div className="divide-y divide-[hsl(var(--border))]">
              {pageLogs.map((item) => (
                <div className="px-5 py-3 transition-colors hover:bg-[hsl(var(--panel-2)/0.3)] flex flex-col justify-center" key={item.id}>
                  {/* Line 1: Fluid Metadata & Far Right Timestamp */}
                  <div className="flex items-center justify-between gap-4 text-[11px] text-[hsl(var(--muted))]">
                    <div className="flex items-center gap-3 min-w-0">
                      {/* Level (natural flow) */}
                      <span className={cn(
                        "font-mono font-semibold uppercase tracking-wider shrink-0",
                        item.level === 'error' && "text-[hsl(var(--danger))]",
                        item.level === 'warn' && "text-[hsl(var(--text-secondary))]",
                        item.level === 'info' && "text-[hsl(var(--muted))]"
                      )}>{item.level}</span>

                      {/* Source (natural flow) */}
                      <span className="font-semibold truncate" title={item.source}>
                        {item.source}
                      </span>
                    </div>

                    {/* Timestamp at the far right */}
                    <span className="shrink-0 font-medium select-none">
                      {formatTimestamp(item.createdAt, i18n.language)}
                    </span>
                  </div>

                  {/* Line 2: Full message text */}
                  <div className="mt-1.5 text-[13px] text-[hsl(var(--text))] break-all whitespace-pre-wrap leading-relaxed">
                    {item.message}
                  </div>
                </div>
              ))}
            </div>
            <div className="flex items-center justify-end gap-3 border-t border-[hsl(var(--border))] px-5 py-3 text-[12px] text-[hsl(var(--muted))]">
              <span className="select-none">
                {t('logs.pageStatus', { page, total: totalPages })}
              </span>
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  aria-label={t('logs.previousPage')}
                  className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-[hsl(var(--border))] text-[hsl(var(--text))] transition-colors hover:bg-[hsl(var(--panel-2))] disabled:cursor-not-allowed disabled:opacity-40"
                  disabled={page <= 1}
                  onClick={() => setPage((current) => Math.max(1, current - 1))}
                >
                  <ChevronLeft className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  aria-label={t('logs.nextPage')}
                  className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-[hsl(var(--border))] text-[hsl(var(--text))] transition-colors hover:bg-[hsl(var(--panel-2))] disabled:cursor-not-allowed disabled:opacity-40"
                  disabled={page >= totalPages}
                  onClick={() => setPage((current) => Math.min(totalPages, current + 1))}
                >
                  <ChevronRight className="h-4 w-4" />
                </button>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
