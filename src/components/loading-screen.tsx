interface LoadingScreenProps {
  label: string
}

export function LoadingScreen({ label }: LoadingScreenProps) {
  return (
    <div className="flex min-h-screen items-center justify-center bg-[hsl(var(--background))]">
      <div className="animate-fade-in text-center">
        <div className="text-[17px] font-semibold tracking-tight text-[hsl(var(--text))]">
          CoLink
        </div>
        <div className="mt-3 flex items-center justify-center gap-2">
          <div className="h-1 w-1 rounded-full bg-[hsl(var(--muted))] animate-pulse-soft" />
          <span className="text-[13px] text-[hsl(var(--muted))]">{label}</span>
        </div>
      </div>
    </div>
  )
}
