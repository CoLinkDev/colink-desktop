import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        'surface h-11 w-full rounded-lg border border-[hsl(var(--border))] px-3 text-sm text-[hsl(var(--text))] outline-none transition focus:border-[hsl(var(--accent))] focus:ring-1 focus:ring-[hsl(var(--accent))] placeholder:text-[hsl(var(--muted))]',
        className,
      )}
      {...props}
    />
  )
}
