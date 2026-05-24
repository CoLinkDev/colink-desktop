import { cva, type VariantProps } from 'class-variance-authority'
import type { ButtonHTMLAttributes } from 'react'

import { cn } from '../../lib/utils'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 rounded-lg text-[13px] font-medium transition-all duration-150 disabled:pointer-events-none disabled:opacity-40 active:scale-[0.98]',
  {
    variants: {
      variant: {
        primary:
          'bg-[hsl(var(--text))] text-[hsl(var(--panel))] hover:opacity-85',
        secondary:
          'border border-[hsl(var(--border))] bg-transparent text-[hsl(var(--text-secondary))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
        ghost:
          'bg-transparent text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
        danger:
          'bg-[hsl(var(--danger))] text-white hover:opacity-85',
      },
      size: {
        default: 'h-9 px-4',
        sm: 'h-8 px-3 text-xs',
        lg: 'h-10 px-5',
      },
    },
    defaultVariants: {
      variant: 'primary',
      size: 'default',
    },
  },
)

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof buttonVariants>

export function Button({
  className,
  variant,
  size,
  type = 'button',
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(buttonVariants({ variant, size }), className)}
      type={type}
      {...props}
    />
  )
}
