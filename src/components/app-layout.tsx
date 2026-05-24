import type { LucideIcon } from 'lucide-react'
import { Computer, LogOut, MessagesSquare, RefreshCw, Settings2, ScrollText, Sun, Moon, Laptop } from 'lucide-react'
import type { PropsWithChildren } from 'react'
import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { NavLink, useNavigate, useLocation } from 'react-router-dom'

import { useAppState, readErrorMessage } from '../hooks/use-app-state'
import { cn } from '../lib/utils'
import { Button } from './ui/button'

export function AppLayout({ children }: PropsWithChildren) {
  const navigate = useNavigate()
  const location = useLocation()
  const { cloud, logout, refreshDevices, theme, setTheme } = useAppState()

  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [showThemeModal, setShowThemeModal] = useState(false)

  useEffect(() => {
    let unlisten: (() => void) | null = null

    void (async () => {
      try {
        unlisten = await listen<string>('shell-navigate', (event) => {
          navigate(event.payload)
        })
      } catch {
        // Desktop runtime only.
      }
    })()

    return () => {
      unlisten?.()
    }
  }, [navigate])

  async function handleRefreshDevices() {
    setRefreshing(true)
    setRefreshError(null)
    try {
      await refreshDevices()
    } catch (err) {
      setRefreshError(readErrorMessage(err))
    } finally {
      setRefreshing(false)
    }
  }

  const getTitle = () => {
    switch (location.pathname) {
      case '/devices':
        return '设备'
      case '/messages':
        return '消息'
      case '/logs':
        return '日志'
      case '/settings':
        return '设置'
      default:
        return 'CoLink'
    }
  }

  return (
    <div className="grid h-screen w-screen grid-cols-[220px_minmax(0,1fr)] overflow-hidden">
      {/* Sidebar */}
      <aside className="flex h-full flex-col border-r bg-[hsl(var(--sidebar))]">
        <div className="flex flex-col px-4 pt-7 pb-5">
          <div className="px-3 text-[15px] font-semibold tracking-tight text-[hsl(var(--text))]">
            CoLink
          </div>
        </div>

        <nav className="flex flex-1 flex-col gap-0.5 px-3">
          <SidebarLink icon={Computer} label="设备" to="/devices" />
          <SidebarLink icon={MessagesSquare} label="消息" to="/messages" />
          <SidebarLink icon={ScrollText} label="日志" to="/logs" />
          <SidebarLink icon={Settings2} label="设置" to="/settings" />
        </nav>

        {/* Bottom area */}
        <div className="mt-auto space-y-1 border-t px-3 py-3">
          <div className="flex items-center gap-2 px-2.5 py-1.5 text-[11px] text-[hsl(var(--muted))]">
            <span className={cn(
              "h-1.5 w-1.5 rounded-full transition-colors duration-300",
              cloud.connected ? "bg-[hsl(var(--success))]" : "bg-[hsl(var(--muted))]"
            )} />
            <span>{getCloudLabel(cloud.state, cloud.attempt)}</span>
          </div>

          <button
            onClick={() => setShowThemeModal(true)}
            className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            type="button"
          >
            {theme === 'dark' && <Moon className="h-3.5 w-3.5" />}
            {theme === 'light' && <Sun className="h-3.5 w-3.5" />}
            {theme === 'auto' && <Laptop className="h-3.5 w-3.5" />}
            外观
          </button>

          <button
            onClick={async () => {
              await logout()
              navigate('/login')
            }}
            className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            type="button"
          >
            <LogOut className="h-3.5 w-3.5" />
            退出
          </button>
        </div>
      </aside>

      {/* Main content */}
      <div className="flex h-full flex-col overflow-hidden bg-[hsl(var(--background))]">
        <header className="flex shrink-0 items-center justify-between px-8 pt-7 pb-0">
          <h1 className="text-[20px] font-semibold tracking-tight">{getTitle()}</h1>

          <div className="flex items-center gap-2">
            {location.pathname === '/devices' && (
              <div className="flex items-center gap-2">
                {refreshError && <span className="text-xs text-[hsl(var(--danger))]">{refreshError}</span>}
                <Button
                  disabled={refreshing}
                  onClick={handleRefreshDevices}
                  size="sm"
                  variant="secondary"
                >
                  <RefreshCw className={refreshing ? 'h-3 w-3 animate-spin' : 'h-3 w-3'} />
                  刷新
                </Button>
              </div>
            )}
          </div>
        </header>

        <main className="flex-1 overflow-y-auto px-8 py-6">{children}</main>
      </div>

      {/* Theme Modal */}
      {showThemeModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-xs rounded-xl border bg-[hsl(var(--panel))] p-5 shadow-xl animate-scale-in">
            <div className="text-[15px] font-semibold text-[hsl(var(--text))]">外观</div>
            <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">
              选择界面显示主题
            </p>

            <div className="mt-5 grid grid-cols-3 gap-1.5">
              <ThemeOption
                active={theme === 'light'}
                icon={Sun}
                label="浅色"
                onClick={() => setTheme('light')}
              />
              <ThemeOption
                active={theme === 'dark'}
                icon={Moon}
                label="深色"
                onClick={() => setTheme('dark')}
              />
              <ThemeOption
                active={theme === 'auto'}
                icon={Laptop}
                label="系统"
                onClick={() => setTheme('auto')}
              />
            </div>

            <div className="mt-5 flex justify-end">
              <Button
                onClick={() => setShowThemeModal(false)}
                size="sm"
                variant="secondary"
              >
                完成
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

function ThemeOption({
  active,
  icon: Icon,
  label,
  onClick,
}: {
  active: boolean
  icon: LucideIcon
  label: string
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex flex-col items-center justify-center gap-1.5 rounded-lg border py-3 text-[12px] font-medium transition-all",
        active
          ? "border-[hsl(var(--text))] bg-[hsl(var(--text)/0.06)] text-[hsl(var(--text))]"
          : "border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))]"
      )}
      type="button"
    >
      <Icon className="h-4 w-4" />
      {label}
    </button>
  )
}


function getCloudLabel(state: string, attempt: number) {
  if (state === 'connected') {
    return '已连接'
  }

  if (state === 'connecting') {
    return '连接中'
  }

  if (state === 'reconnecting') {
    return `重连中 #${attempt}`
  }

  return '未连接'
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
          'flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] transition-colors',
          isActive
            ? 'bg-[hsl(var(--panel-2))] font-medium text-[hsl(var(--text))]'
            : 'text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
        )
      }
      to={to}
    >
      <Icon className="h-[15px] w-[15px]" />
      {label}
    </NavLink>
  )
}
