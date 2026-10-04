import type { InputHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

type SliderProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>

export function Slider({ className, min = 0, max = 100, value = 0, style, ...props }: SliderProps) {
  const minimum = Number(min)
  const maximum = Number(max)
  const current = Number(value)
  const progress = maximum > minimum
    ? Math.max(0, Math.min(100, ((current - minimum) / (maximum - minimum)) * 100))
    : 0

  return (
    <input
      className={cn(
        'h-1.5 w-full cursor-pointer appearance-none rounded-full outline-none disabled:pointer-events-none disabled:opacity-40',
        '[&::-webkit-slider-thumb]:h-4 [&::-webkit-slider-thumb]:w-4 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-[hsl(var(--text))] [&::-webkit-slider-thumb]:shadow-sm',
        '[&::-moz-range-thumb]:h-4 [&::-moz-range-thumb]:w-4 [&::-moz-range-thumb]:rounded-full [&::-moz-range-thumb]:border-0 [&::-moz-range-thumb]:bg-[hsl(var(--text))] [&::-moz-range-thumb]:shadow-sm',
        className,
      )}
      max={max}
      min={min}
      style={{
        background: `linear-gradient(to right, hsl(var(--text)) 0%, hsl(var(--text)) ${progress}%, hsl(var(--border)) ${progress}%, hsl(var(--border)) 100%)`,
        ...style,
      }}
      type="range"
      value={value}
      {...props}
    />
  )
}
