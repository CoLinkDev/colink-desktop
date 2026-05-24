import { useAppState } from '../hooks/use-app-state'
import { cn } from '../lib/utils'
import { formatTimestamp } from '../lib/utils'

export function LogsPage() {
  const { logs } = useAppState()

  return (
    <div className="animate-fade-in">
      <div className="rounded-xl border bg-[hsl(var(--panel))]">
        {logs.length === 0 ? (
          <div className="py-16 text-center text-[13px] text-[hsl(var(--muted))]">暂无日志</div>
        ) : (
          <div className="divide-y divide-[hsl(var(--border))]">
            {logs.slice(0, 50).map((item) => (
              <div className="px-5 py-3 transition-colors hover:bg-[hsl(var(--panel-2)/0.3)]" key={item.id}>
                <div className="flex items-center gap-2 text-[11px] text-[hsl(var(--muted))]">
                  <span className={cn(
                    "font-mono uppercase",
                    item.level === 'error' && "text-[hsl(var(--danger))]",
                    item.level === 'warn' && "text-[hsl(var(--text-secondary))]"
                  )}>{item.level}</span>
                  <span className="text-[hsl(var(--border))]">·</span>
                  <span>{item.source}</span>
                  <span className="text-[hsl(var(--border))]">·</span>
                  <span>{formatTimestamp(item.createdAt)}</span>
                </div>
                <div className="mt-1 text-[13px] text-[hsl(var(--text))]">{item.message}</div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
