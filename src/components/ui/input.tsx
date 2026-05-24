import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        'h-9 w-full rounded-lg border border-[hsl(var(--border))] bg-transparent px-3 text-[13px] text-[hsl(var(--text))] outline-none transition-colors duration-150 placeholder:text-[hsl(var(--muted))] focus:border-[hsl(var(--ring))] focus:ring-1 focus:ring-[hsl(var(--ring))]',
        className,
      )}
      {...props}
    />
  )
}
