interface LoadingScreenProps {
  label: string
}

export function LoadingScreen({ label }: LoadingScreenProps) {
  return (
    <div className="flex min-h-screen items-center justify-center bg-[hsl(var(--background))] px-6">
      <div className="flex w-full max-w-sm flex-col gap-5 rounded-lg border border-[hsl(var(--border))] bg-[hsl(var(--panel))] p-8">
        <div className="flex items-center gap-3">
          <div className="h-3 w-3 animate-pulse rounded-full bg-[hsl(var(--accent))]" />
          <span className="text-sm text-[hsl(var(--muted))]">{label}</span>
        </div>
        <div>
          <div className="text-xl font-semibold">CoLink Desktop</div>
          <div className="mt-2 text-sm text-[hsl(var(--muted))]">
            本地状态和远端会话正在同步。
          </div>
        </div>
      </div>
    </div>
  )
}
