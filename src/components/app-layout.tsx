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
        return '设备管理'
      case '/messages':
        return '消息中心'
      case '/logs':
        return '运行日志'
      case '/settings':
        return '系统设置'
      default:
        return '本地设备中心'
    }
  }

  return (
    <div className="grid h-screen w-screen grid-cols-[248px_minmax(0,1fr)] overflow-hidden bg-[hsl(var(--background))]">
      <aside className="flex h-full border-r border-[hsl(var(--border))] bg-[hsl(var(--panel))]">
        <div className="flex w-full flex-col px-5 py-6">
          <div className="mb-8 px-3">
            <div className="text-2xl font-semibold tracking-wider text-[hsl(var(--text))]">
              CoLink
            </div>
          </div>

          <nav className="flex flex-1 flex-col gap-2">
            <SidebarLink icon={Computer} label="设备" to="/devices" />
            <SidebarLink icon={MessagesSquare} label="消息" to="/messages" />
            <SidebarLink icon={ScrollText} label="日志" to="/logs" />
            <SidebarLink icon={Settings2} label="设置" to="/settings" />
          </nav>

          {/* Bottom connection status, appearance and logout */}
          <div className="mt-auto border-t border-[hsl(var(--border))] pt-4 space-y-2">
            <div className="flex items-center gap-2 px-3 text-xs text-[hsl(var(--muted))] mb-2">
              <span className={cn(
                "h-2 w-2 rounded-full transition-colors duration-300",
                cloud.connected ? "bg-[hsl(var(--accent))]" : "bg-[hsl(var(--muted))]"
              )} />
              <span>云端：{getCloudLabel(cloud.state, cloud.attempt)}</span>
            </div>

            <Button
              onClick={() => setShowThemeModal(true)}
              size="sm"
              variant="ghost"
              className="w-full justify-start text-[hsl(var(--muted))] hover:text-[hsl(var(--text))] hover:bg-[hsl(var(--panel-2))] px-3"
            >
              {theme === 'dark' && <Moon className="h-4 w-4 mr-2" />}
              {theme === 'light' && <Sun className="h-4 w-4 mr-2" />}
              {theme === 'auto' && <Laptop className="h-4 w-4 mr-2" />}
              外观设置
            </Button>

            <Button
              onClick={async () => {
                await logout()
                navigate('/login')
              }}
              size="sm"
              variant="ghost"
              className="w-full justify-start text-[hsl(var(--muted))] hover:text-[hsl(var(--text))] hover:bg-[hsl(var(--panel-2))] px-3"
            >
              <LogOut className="h-4 w-4 mr-2" />
              退出登录
            </Button>
          </div>
        </div>
      </aside>

      <div className="flex h-full flex-col overflow-hidden">
        <header className="flex shrink-0 items-center justify-between border-b border-[hsl(var(--border))] px-8 py-5">
          <div>
            <div className="text-lg font-semibold">{getTitle()}</div>
          </div>

          <div className="flex items-center gap-3">
            {location.pathname === '/devices' && (
              <div className="flex items-center gap-3">
                {refreshError && <span className="text-xs text-[hsl(var(--danger))]">{refreshError}</span>}
                <Button
                  disabled={refreshing}
                  onClick={handleRefreshDevices}
                  size="sm"
                  variant="secondary"
                  className="flex items-center gap-1.5"
                >
                  <RefreshCw className={refreshing ? 'h-3.5 w-3.5 animate-spin' : 'h-3.5 w-3.5'} />
                  刷新
                </Button>
              </div>
            )}
          </div>
        </header>

        <main className="flex-1 overflow-y-auto px-8 py-8">{children}</main>
      </div>

      {/* Modern High-End Theme Toggler Modal */}
      {showThemeModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm animate-in fade-in duration-150">
          <div className="w-full max-w-sm rounded-xl border border-[hsl(var(--border))] bg-[hsl(var(--panel))] p-6 shadow-2xl animate-in zoom-in-95 duration-150">
            <h3 className="text-lg font-semibold text-[hsl(var(--text))]">外观设置</h3>
            <p className="mt-1 text-sm text-[hsl(var(--muted))]">
              选择适合您的界面显示主题。
            </p>

            <div className="mt-6 grid grid-cols-3 gap-2">
              <button
                onClick={() => setTheme('light')}
                className={cn(
                  "flex flex-col items-center justify-center gap-2 rounded-lg border py-3.5 transition-all hover:bg-[hsl(var(--panel-2)/0.5)]",
                  theme === 'light'
                    ? "border-[hsl(var(--accent))] bg-[hsl(var(--accent)/0.08)] text-[hsl(var(--accent))]"
                    : "border-[hsl(var(--border))] text-[hsl(var(--muted))]"
                )}
                type="button"
              >
                <Sun className="h-5 w-5" />
                <span className="text-xs font-medium">浅色模式</span>
              </button>

              <button
                onClick={() => setTheme('dark')}
                className={cn(
                  "flex flex-col items-center justify-center gap-2 rounded-lg border py-3.5 transition-all hover:bg-[hsl(var(--panel-2)/0.5)]",
                  theme === 'dark'
                    ? "border-[hsl(var(--accent))] bg-[hsl(var(--accent)/0.08)] text-[hsl(var(--accent))]"
                    : "border-[hsl(var(--border))] text-[hsl(var(--muted))]"
                )}
                type="button"
              >
                <Moon className="h-5 w-5" />
                <span className="text-xs font-medium">深色模式</span>
              </button>

              <button
                onClick={() => setTheme('auto')}
                className={cn(
                  "flex flex-col items-center justify-center gap-2 rounded-lg border py-3.5 transition-all hover:bg-[hsl(var(--panel-2)/0.5)]",
                  theme === 'auto'
                    ? "border-[hsl(var(--accent))] bg-[hsl(var(--accent)/0.08)] text-[hsl(var(--accent))]"
                    : "border-[hsl(var(--border))] text-[hsl(var(--muted))]"
                )}
                type="button"
              >
                <Laptop className="h-5 w-5" />
                <span className="text-xs font-medium">跟随系统</span>
              </button>
            </div>

            <div className="mt-6 flex justify-end">
              <Button
                onClick={() => setShowThemeModal(false)}
                size="sm"
                variant="secondary"
                className="px-4"
              >
                确定
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
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
