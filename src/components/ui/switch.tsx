import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

type SwitchProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>

export function Switch({ className, checked, ...props }: SwitchProps) {
  return (
    <label className={cn('relative inline-flex h-6 w-11 items-center', className)}>
      <input
        checked={checked}
        className="peer sr-only"
        type="checkbox"
        {...props}
      />
      <span className="absolute inset-0 rounded-full bg-[hsl(var(--panel-2))] transition peer-checked:bg-[hsl(var(--accent))]" />
      <span className="absolute left-1 h-4 w-4 rounded-full bg-white transition peer-checked:translate-x-5" />
    </label>
  )
}
