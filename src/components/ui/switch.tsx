import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

type SwitchProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>

export function Switch({ className, checked, ...props }: SwitchProps) {
  return (
    <label className={cn('relative inline-flex h-[22px] w-[40px] cursor-pointer items-center', className)}>
      <input
        checked={checked}
        className="peer sr-only"
        type="checkbox"
        {...props}
      />
      <span className="absolute inset-0 rounded-full bg-[hsl(var(--border))] transition-colors duration-200 peer-checked:bg-[hsl(var(--text))]" />
      <span className="absolute left-[3px] h-4 w-4 rounded-full bg-white shadow-sm transition-transform duration-200 peer-checked:translate-x-[18px]" />
    </label>
  )
}
