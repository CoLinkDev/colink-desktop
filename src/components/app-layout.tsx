import type { LucideIcon } from 'lucide-react'
import { Computer, LogIn, LogOut, MessagesSquare, RefreshCw, Settings2, ScrollText, Sun, Moon, Laptop, ArrowUpDown, Save } from 'lucide-react'
import type { TFunction } from 'i18next'
import type { PropsWithChildren } from 'react'
import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { NavLink, useNavigate, useLocation, useBlocker } from 'react-router-dom'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'

import { Toaster } from 'sonner'

import { useAppState, readErrorMessage } from '../hooks/use-app-state'
import { cn } from '../lib/utils'
import { AuthDialog } from './auth-dialog'
import { Button } from './ui/button'

export function AppLayout({ children }: PropsWithChildren) {
  const navigate = useNavigate()
  const location = useLocation()
  const { cloud, logout, refreshDevices, session, theme, setTheme, settingsDirty } = useAppState()
  const { t } = useTranslation()

  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [showThemeModal, setShowThemeModal] = useState(false)
  const [showAuthDialog, setShowAuthDialog] = useState(false)
  const [showLogoutConfirm, setShowLogoutConfirm] = useState(false)
  const [loggingOut, setLoggingOut] = useState(false)
  const [logoutError, setLogoutError] = useState<string | null>(null)

  const blocker = useBlocker(
    ({ nextLocation }) =>
      settingsDirty &&
      location.pathname === '/settings' &&
      nextLocation.pathname !== '/settings'
  )

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

  async function handleConfirmLogout() {
    setLoggingOut(true)
    setLogoutError(null)
    try {
      await logout()
      setShowLogoutConfirm(false)
      navigate('/devices')
    } catch (err) {
      setLogoutError(readErrorMessage(err))
    } finally {
      setLoggingOut(false)
    }
  }

  const getTitle = () => {
    switch (location.pathname) {
      case '/devices':
        return t('nav.devices')
      case '/messages':
        return t('nav.messages')
      case '/transfers':
        return t('nav.transfers')
      case '/logs':
        return t('nav.logs')
      case '/settings':
        return t('nav.settings')
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
          <SidebarLink icon={Computer} label={t('nav.devices')} to="/devices" />
          <SidebarLink icon={MessagesSquare} label={t('nav.messages')} to="/messages" />
          <SidebarLink icon={ArrowUpDown} label={t('nav.transfers')} to="/transfers" />
          <SidebarLink icon={ScrollText} label={t('nav.logs')} to="/logs" />
          <SidebarLink icon={Settings2} label={t('nav.settings')} to="/settings" />
        </nav>

        {/* Bottom area */}
        <div className="mt-auto border-t p-3 space-y-2">
          {/* Connection Status Widget */}
          <div
            className="rounded-lg bg-[hsl(var(--panel-2))] border px-3 py-2 select-none"
            title={getCloudLabel(cloud.state, cloud.attempt, t)}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-[12px] font-medium text-[hsl(var(--muted))] truncate max-w-[140px]">
                {session ? session.userId : t('devices.lan')}
              </span>
              <span className="relative flex h-2 w-2 shrink-0">
                {cloud.connected && (
                  <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[hsl(var(--success))] opacity-75" />
                )}
                <span className={cn(
                  "relative inline-flex h-2 w-2 rounded-full transition-colors duration-300",
                  cloud.connected ? "bg-[hsl(var(--success))]" : "bg-[hsl(var(--muted))]"
                )} />
              </span>
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
                {theme === 'dark' && <Moon className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--text))] transition-colors duration-200" />}
                {theme === 'light' && <Sun className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--text))] transition-colors duration-200" />}
                {theme === 'auto' && <Laptop className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--text))] transition-colors duration-200" />}
              </div>
              <span className="flex h-5 items-center translate-x-0 transition-transform duration-200 group-hover:translate-x-[2px] leading-none">
                {t('nav.theme')}
              </span>
            </button>

            <button
              onClick={() => {
                if (session) {
                  setLogoutError(null)
                  setShowLogoutConfirm(true)
                } else {
                  setShowAuthDialog(true)
                }
              }}
              className={cn(
                "group flex h-[38px] w-full items-center gap-2.5 rounded-lg px-3 text-[13px] font-medium text-[hsl(var(--muted))] transition-all duration-200 hover:bg-[hsl(var(--panel-2))]",
                session ? "hover:text-[hsl(var(--danger))]" : "hover:text-[hsl(var(--text))]"
              )}
              type="button"
            >
              <div className="flex h-5 w-5 shrink-0 items-center justify-center">
                {session ? (
                  <LogOut className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--danger))]" />
                ) : (
                  <LogIn className="h-4 w-4 text-[hsl(var(--muted))] group-hover:text-[hsl(var(--text))]" />
                )}
              </div>
              <span className="flex h-5 items-center translate-x-0 transition-transform duration-200 group-hover:translate-x-[2px] leading-none">
                {session ? t('nav.logout') : t('auth.login')}
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
                  {t('common.refresh')}
                </Button>
              </div>
            )}
            {location.pathname === '/settings' && (
              <Button
                disabled={!settingsDirty}
                form="settings-form"
                type="submit"
                size="sm"
                className="gap-1.5"
              >
                <Save className="h-3.5 w-3.5" />
                {t('common.save')}
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
            <div className="text-[15px] font-semibold text-[hsl(var(--text))]">{t('theme.title')}</div>
            <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">
              {t('theme.subtitle')}
            </p>
            <div className="mt-5 grid grid-cols-3 gap-1.5">
              <ThemeOption
                active={theme === 'light'}
                icon={Sun}
                label={t('theme.light')}
                onClick={() => setTheme('light')}
              />
              <ThemeOption
                active={theme === 'dark'}
                icon={Moon}
                label={t('theme.dark')}
                onClick={() => setTheme('dark')}
              />
              <ThemeOption
                active={theme === 'auto'}
                icon={Laptop}
                label={t('theme.auto')}
                onClick={() => setTheme('auto')}
              />
            </div>

            <div className="mt-5 flex justify-end">
              <Button
                onClick={() => setShowThemeModal(false)}
                size="sm"
                variant="secondary"
              >
                {t('theme.done')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {showLogoutConfirm && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('nav.logout')}</div>
            <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
              {t('auth.logoutConfirmDesc')}
            </p>

            {logoutError && (
              <div className="mt-4 rounded-lg border border-[hsl(var(--danger)/0.15)] bg-[hsl(var(--danger)/0.08)] px-3.5 py-2.5 text-[12px] text-[hsl(var(--danger))]">
                {logoutError}
              </div>
            )}

            <div className="mt-6 flex justify-end gap-2">
              <Button
                disabled={loggingOut}
                onClick={() => setShowLogoutConfirm(false)}
                variant="secondary"
              >
                {t('common.cancel')}
              </Button>
              <Button
                disabled={loggingOut}
                onClick={handleConfirmLogout}
                variant="danger"
              >
                {loggingOut ? t('common.logout') : t('common.confirm')}
              </Button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      {blocker.state === 'blocked' && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
          <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('settings.unsavedChangesTitle')}</div>
            <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
              {t('settings.unsavedChangesDesc')}
            </p>

            <div className="mt-6 flex justify-end gap-2">
              <Button
                onClick={() => blocker.reset()}
                variant="secondary"
              >
                {t('common.cancel')}
              </Button>
              <Button
                onClick={() => blocker.proceed()}
                variant="danger"
              >
                {t('settings.leave')}
              </Button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      <AuthDialog open={showAuthDialog} onClose={() => setShowAuthDialog(false)} />

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


function getCloudLabel(state: string, attempt: number, t: TFunction) {
  if (state === 'connected') {
    return t('cloud.connected')
  }

  if (state === 'connecting') {
    return t('cloud.connecting')
  }

  if (state === 'reconnecting') {
    return t('cloud.reconnecting', { attempt })
  }

  return t('cloud.disconnected')
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
