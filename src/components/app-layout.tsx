import type { LucideIcon } from 'lucide-react'
import { Computer, LogOut, RefreshCw, Settings2 } from 'lucide-react'
import type { PropsWithChildren } from 'react'
import { NavLink, useNavigate } from 'react-router-dom'

import { useAppState } from '../hooks/use-app-state'
import { cn } from '../lib/utils'
import { Button } from './ui/button'

export function AppLayout({ children }: PropsWithChildren) {
  const navigate = useNavigate()
  const { device, logout, refreshBootstrap } = useAppState()

  return (
    <div className="grid min-h-screen grid-cols-[248px_minmax(0,1fr)] bg-[hsl(var(--background))]">
      <aside className="flex border-r border-[hsl(var(--border))] bg-[hsl(var(--panel))]">
        <div className="flex w-full flex-col px-5 py-6">
          <div className="mb-8">
            <div className="text-xs uppercase tracking-[0.12em] text-[hsl(var(--muted))]">
              CoLink
            </div>
            <div className="mt-2 text-2xl font-semibold">Desktop</div>
          </div>

          <nav className="flex flex-1 flex-col gap-2">
            <SidebarLink icon={Computer} label="设备" to="/devices" />
            <SidebarLink icon={Settings2} label="设置" to="/settings" />
          </nav>

          <div className="surface-muted rounded-lg border border-[hsl(var(--border))] p-4">
            <div className="text-xs uppercase tracking-[0.1em] text-[hsl(var(--muted))]">
              当前设备
            </div>
            <div className="mt-3 text-sm font-medium text-[hsl(var(--text))]">
              {device?.name ?? '未注册'}
            </div>
            <div className="mt-1 text-xs text-[hsl(var(--muted))]">
              {device?.deviceType ?? 'unknown'}
            </div>
          </div>
        </div>
      </aside>

      <div className="flex min-h-screen flex-col">
        <header className="flex items-center justify-between border-b border-[hsl(var(--border))] px-8 py-5">
          <div>
            <div className="text-sm text-[hsl(var(--muted))]">账户已连接</div>
            <div className="mt-1 text-lg font-semibold">本地设备中心</div>
          </div>

          <div className="flex items-center gap-3">
            <Button onClick={() => void refreshBootstrap()} size="sm" variant="secondary">
              <RefreshCw className="h-4 w-4" />
              重载
            </Button>
            <Button
              onClick={async () => {
                await logout()
                navigate('/login')
              }}
              size="sm"
              variant="ghost"
            >
              <LogOut className="h-4 w-4" />
              退出
            </Button>
          </div>
        </header>

        <main className="flex-1 px-8 py-8">{children}</main>
      </div>
    </div>
  )
}

interface SidebarLinkProps {
  icon: LucideIcon
  label: string
  to: string
}

function SidebarLink({ icon: Icon, label, to }: SidebarLinkProps) {
  return (
    <NavLink
      className={({ isActive }) =>
        cn(
          'flex h-11 items-center gap-3 rounded-lg px-3 text-sm transition',
          isActive
            ? 'bg-[hsl(var(--accent)/0.18)] text-[hsl(var(--text))]'
            : 'text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
        )
      }
      to={to}
    >
      <Icon className="h-4 w-4" />
      {label}
    </NavLink>
  )
}
