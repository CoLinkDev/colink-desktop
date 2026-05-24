import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

type SwitchProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>

export function Switch({ className, checked, ...props }: SwitchProps) {
  return (
    <label className={cn('relative inline-flex h-[22px] w-[40px] shrink-0 cursor-pointer items-center select-none', className)}>
      <input
        checked={checked}
        className="peer sr-only"
        type="checkbox"
        {...props}
      />
      <span className="absolute inset-0 rounded-full bg-[hsl(var(--muted)/0.15)] dark:bg-[hsl(var(--muted)/0.25)] border border-[hsl(var(--border))] transition-colors duration-200 peer-checked:bg-[hsl(var(--text))] peer-checked:border-[hsl(var(--text))]" />
      <span className="absolute left-[3px] h-4 w-4 rounded-full bg-white shadow-sm transition-all duration-200 peer-checked:translate-x-[18px] peer-checked:bg-white dark:peer-checked:bg-[hsl(var(--panel))]" />
    </label>
  )
}
