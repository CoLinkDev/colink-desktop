import { useTranslation } from 'react-i18next'
import { useAppState } from '../hooks/use-app-state'
import { cn } from '../lib/utils'
import { formatTimestamp } from '../lib/utils'

export function LogsPage() {
  const { logs } = useAppState()
  const { t } = useTranslation()

  return (
    <div className="animate-fade-in">
      <div className="rounded-xl border bg-[hsl(var(--panel))]">
        {logs.length === 0 ? (
          <div className="py-16 text-center text-[13px] text-[hsl(var(--muted))]">{t('logs.empty')}</div>
        ) : (
          <div className="divide-y divide-[hsl(var(--border))]">
            {logs.slice(0, 50).map((item) => (
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
                    {formatTimestamp(item.createdAt)}
                  </span>
                </div>

                {/* Line 2: Full message text */}
                <div className="mt-1.5 text-[13px] text-[hsl(var(--text))] break-all whitespace-pre-wrap leading-relaxed">
                  {item.message}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
