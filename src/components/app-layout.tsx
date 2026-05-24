import type { LucideIcon } from 'lucide-react'
import { Computer, LogOut, MessagesSquare, RefreshCw, Settings2, ScrollText, Sun, Moon, Laptop, ArrowUpDown, Save } from 'lucide-react'
import type { PropsWithChildren } from 'react'
import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { NavLink, useNavigate, useLocation } from 'react-router-dom'

import { Toaster } from 'sonner'

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
      case '/transfers':
        return '文件传输'
      case '/logs':
        return '日志'
      case '/settings':
        return '设置'
      default:
        return 'CoLink Desktop'
    }
  }

  return (
    <>
      <div className="grid h-screen w-screen grid-cols-[220px_minmax(0,1fr)] overflow-hidden">
        {/* Sidebar */}
      <aside className="flex h-full flex-col border-r bg-[hsl(var(--sidebar))]">
        {/* Logo/Brand Area */}
        <div className="flex flex-col px-4 pt-7 pb-5 select-none">
          <div className="px-1 font-google-sans text-[18px] font-bold tracking-tight text-[hsl(var(--text))]">
            CoLink Desktop
          </div>
        </div>

        {/* Navigation Items */}
        <nav className="flex flex-1 flex-col gap-1 px-3">
          <SidebarLink icon={Computer} label="设备" to="/devices" />
          <SidebarLink icon={MessagesSquare} label="消息" to="/messages" />
          <SidebarLink icon={ArrowUpDown} label="文件传输" to="/transfers" />
          <SidebarLink icon={ScrollText} label="日志" to="/logs" />
          <SidebarLink icon={Settings2} label="设置" to="/settings" />
        </nav>

        {/* Bottom area */}
        <div className="mt-auto border-t p-3 space-y-2">
          {/* Connection Status Widget */}
          <div className="rounded-lg bg-[hsl(var(--panel-2))] border p-2.5 select-none">
            <div className="flex items-center justify-between">
              <span className="text-[10px] font-semibold text-[hsl(var(--muted))] uppercase tracking-wider">
                连接状态
              </span>
              <div className="flex items-center gap-1.5">
                <span className="relative flex h-1.5 w-1.5">
                  {cloud.connected && (
                    <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[hsl(var(--success))] opacity-75" />
                  )}
                  <span className={cn(
                    "relative inline-flex h-1.5 w-1.5 rounded-full transition-colors duration-300",
                    cloud.connected ? "bg-[hsl(var(--success))]" : "bg-[hsl(var(--muted))]"
                  )} />
                </span>
                <span className="text-[11px] font-medium text-[hsl(var(--text))]">
                  {getCloudLabel(cloud.state, cloud.attempt)}
                </span>
              </div>
            </div>
          </div>

          {/* Action Buttons Stack */}
          <div className="flex flex-col gap-1">
            <button
              onClick={() => setShowThemeModal(true)}
              className="group flex h-[38px] w-full items-center gap-2.5 rounded-lg px-3 text-[13px] font-medium text-[hsl(var(--muted))] transition-all duration-200 hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
              type="button"
            >
              <div className="flex h-5 w-5 shrink-0 items-center justify-center">
                {theme === 'dark' && <Moon className="h-4 w-4 text-[hsl(var(--accent))]" />}
                {theme === 'light' && <Sun className="h-4 w-4 text-[hsl(var(--accent))]" />}
                {theme === 'auto' && <Laptop className="h-4 w-4 text-[hsl(var(--accent))]" />}
              </div>
              <span className="flex h-5 items-center translate-x-0 transition-transform duration-200 group-hover:translate-x-[2px] leading-none">
                外观
              </span>
            </button>

            <button
              onClick={async () => {
                await logout()
                navigate('/login')
              }}
              className="group flex h-[38px] w-full items-center gap-2.5 rounded-lg px-3 text-[13px] font-medium text-[hsl(var(--muted))] transition-all duration-200 hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--danger))]"
              type="button"
            >
              <div className="flex h-5 w-5 shrink-0 items-center justify-center">
                <LogOut className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--danger))]" />
              </div>
              <span className="flex h-5 items-center translate-x-0 transition-transform duration-200 group-hover:translate-x-[2px] leading-none">
                退出
              </span>
            </button>
          </div>
        </div>
      </aside>

      {/* Main content */}
      <div className="flex h-full flex-col overflow-hidden bg-[hsl(var(--background))]">
        <header className="flex h-16 shrink-0 items-center justify-between border-b px-8">
          <h1 className="text-[20px] font-semibold tracking-tight text-[hsl(var(--text))]">{getTitle()}</h1>

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
            {location.pathname === '/settings' && (
              <Button
                form="settings-form"
                type="submit"
                size="sm"
                className="gap-1.5"
              >
                <Save className="h-3.5 w-3.5" />
                保存
              </Button>
            )}
          </div>
        </header>

        <main className={cn(
          "flex-1 min-h-0",
          (location.pathname === '/messages' || location.pathname === '/transfers')
            ? "overflow-hidden"
            : "overflow-y-auto px-8 py-6"
        )}>
          {children}
        </main>
      </div>
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

      {/* Toast Notification Container */}
      <Toaster theme={theme === 'auto' ? 'system' : theme} position="top-right" closeButton richColors />
    </>
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
        "flex items-center justify-center gap-1.5 rounded-lg border px-2 py-2 text-[12px] font-medium transition-all w-full",
        active
          ? "border-[hsl(var(--text))] bg-[hsl(var(--text)/0.06)] text-[hsl(var(--text))]"
          : "border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
      )}
      type="button"
    >
      <Icon className="h-3.5 w-3.5 shrink-0" />
      <span>{label}</span>
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
          'group relative flex h-[38px] items-center gap-2.5 rounded-lg px-3 text-[13px] font-medium transition-all duration-200',
          isActive
            ? 'bg-[hsl(var(--panel-2))] text-[hsl(var(--text))]'
            : 'text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))/0.4] hover:text-[hsl(var(--text))]',
        )
      }
      to={to}
    >
      {({ isActive }) => (
        <>
          {/* Active side indicator - pushed further to the left (from left-1.5 to left-1) */}
          {isActive && (
            <div className="absolute left-1 h-3.5 w-1 rounded-full bg-[hsl(var(--accent))]" />
          )}
          {/* Fixed-width icon wrapper for grid-perfect alignment */}
          <div className="flex h-5 w-5 shrink-0 items-center justify-center">
            <Icon className={cn(
              "h-4 w-4 transition-colors duration-200",
              isActive ? "text-[hsl(var(--accent))]" : "text-[hsl(var(--muted))] group-hover:text-[hsl(var(--text))]"
            )} />
          </div>
          {/* h-5 and items-center guarantees mathematical baseline vertical alignment with the icon wrapper */}
          <span className="flex h-5 items-center translate-x-0 transition-transform duration-200 group-hover:translate-x-[2px] leading-none">
            {label}
          </span>
        </>
      )}
    </NavLink>
  )
}
