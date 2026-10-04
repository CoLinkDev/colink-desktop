import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Check, ChevronDown } from 'lucide-react'

import { cn } from '../../lib/utils'

export interface SelectOption {
  value: string
  label: string
}

interface SelectProps {
  value: string
  options: SelectOption[]
  onValueChange: (value: string) => void
  ariaLabel?: string
  className?: string
  disabled?: boolean
}

export function Select({
  value,
  options,
  onValueChange,
  ariaLabel,
  className,
  disabled = false,
}: SelectProps) {
  const [open, setOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState({ left: 0, top: 0, width: 0 })
  const containerRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const selected = options.find((option) => option.value === value)

  useLayoutEffect(() => {
    if (!open) return

    const updatePosition = () => {
      const rect = triggerRef.current?.getBoundingClientRect()
      if (!rect) return
      const menuHeight = Math.min(options.length * 36 + 8, 240)
      const below = rect.bottom + 4
      const top = below + menuHeight <= window.innerHeight - 8
        ? below
        : Math.max(8, rect.top - menuHeight - 4)
      setMenuPosition({ left: rect.left, top, width: rect.width })
    }

    updatePosition()
    window.addEventListener('resize', updatePosition)
    window.addEventListener('scroll', updatePosition, true)
    return () => {
      window.removeEventListener('resize', updatePosition)
      window.removeEventListener('scroll', updatePosition, true)
    }
  }, [open, options.length])

  useEffect(() => {
    if (!open) return

    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node
      if (!containerRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false)
      }
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false)
        triggerRef.current?.focus()
      }
    }
    document.addEventListener('pointerdown', closeOnOutsidePointer)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsidePointer)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [open])

  return (
    <div className={cn('relative', className)} ref={containerRef}>
      <button
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        className="flex h-9 w-full items-center justify-between gap-3 rounded-lg border border-[hsl(var(--border))] bg-transparent px-3 text-left text-[13px] text-[hsl(var(--text))] outline-none transition-colors duration-150 hover:bg-[hsl(var(--panel-2))] focus:border-[hsl(var(--ring))] focus:ring-1 focus:ring-[hsl(var(--ring))] disabled:pointer-events-none disabled:opacity-40"
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        type="button"
      >
        <span className="truncate">{selected?.label ?? value}</span>
        <ChevronDown className={cn('h-3.5 w-3.5 shrink-0 text-[hsl(var(--muted))] transition-transform', open && 'rotate-180')} />
      </button>
      {open && createPortal(
        <div
          className="fixed z-[80] max-h-60 overflow-y-auto rounded-lg border bg-[hsl(var(--panel))] p-1 shadow-xl animate-fade-in"
          ref={menuRef}
          role="listbox"
          style={menuPosition}
        >
          {options.map((option) => {
            const isSelected = option.value === value
            return (
              <button
                aria-selected={isSelected}
                className={cn(
                  'flex h-9 w-full items-center justify-between gap-3 rounded-md px-2.5 text-left text-[13px] transition-colors',
                  isSelected
                    ? 'bg-[hsl(var(--panel-2))] font-medium text-[hsl(var(--text))]'
                    : 'text-[hsl(var(--text-secondary))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
                )}
                key={option.value}
                onClick={() => {
                  onValueChange(option.value)
                  setOpen(false)
                  triggerRef.current?.focus()
                }}
                role="option"
                type="button"
              >
                <span className="truncate">{option.label}</span>
                {isSelected && <Check className="h-3.5 w-3.5 shrink-0" />}
              </button>
            )
          })}
        </div>,
        document.body,
      )}
    </div>
  )
}
