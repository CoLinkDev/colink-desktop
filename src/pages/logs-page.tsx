import { useAppState } from '../hooks/use-app-state'
import { formatTimestamp } from '../lib/utils'

export function LogsPage() {
  const { logs } = useAppState()

  return (
    <div className="space-y-6">
      <div className="surface rounded-lg border border-[hsl(var(--border))]">
        {logs.length === 0 ? (
          <div className="px-6 py-10 text-sm text-[hsl(var(--muted))]">还没有日志。</div>
        ) : (
          <div className="divide-y divide-[hsl(var(--border))]">
            {logs.slice(0, 50).map((item) => (
              <div className="px-6 py-4 transition hover:bg-[hsl(var(--panel-2)/0.3)]" key={item.id}>
                <div className="flex flex-wrap items-center gap-3 text-xs text-[hsl(var(--muted))]">
                  <span className="font-semibold uppercase tracking-wider">{item.level}</span>
                  <span>•</span>
                  <span>{item.source}</span>
                  <span>•</span>
                  <span>{formatTimestamp(item.createdAt)}</span>
                </div>
                <div className="mt-2 text-sm text-[hsl(var(--text))]">{item.message}</div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
