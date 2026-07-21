import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { useSearchParams } from 'react-router-dom'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { LoaderCircle, Terminal as TerminalIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { useAppState } from '../hooks/use-app-state'
import { closeTerminal, getRemoteTerminalSupport, openTerminal, resizeTerminal, writeTerminal } from '../lib/api'
import type { RemoteTerminalSupport } from '../lib/types'
import { cn, formatPlatformName } from '../lib/utils'

interface TerminalEvent {
  sessionId: string
  kind: string
  data?: string
  message?: string
}

interface ActiveSession {
  deviceId: string
  sessionId: string
}

export function TerminalPage() {
  const { t } = useTranslation()
  const { devices, device, setTerminalSessionActive } = useAppState()
  const [searchParams, setSearchParams] = useSearchParams()
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const activeSessionRef = useRef<ActiveSession | null>(null)
  const generationRef = useRef(0)
  const autoConnectDeviceRef = useRef<string | null>(null)
  const [connectingDeviceId, setConnectingDeviceId] = useState<string | null>(null)
  const [connectedDeviceId, setConnectedDeviceId] = useState<string | null>(null)
  const [terminalVersion, setTerminalVersion] = useState(0)
  const [supportState, setSupportState] = useState<{ deviceId: string; value: RemoteTerminalSupport } | null>(null)

  const terminalDevices = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId && item.online && ['windows', 'macos', 'linux'].includes(item.type)),
    [device?.deviceId, devices],
  )
  const selectedDeviceId = useMemo(() => {
    const requestedDeviceId = searchParams.get('deviceId')
    if (requestedDeviceId && terminalDevices.some((item) => item.deviceId === requestedDeviceId)) {
      return requestedDeviceId
    }
    return terminalDevices[0]?.deviceId ?? ''
  }, [searchParams, terminalDevices])
  const selectedDevice = terminalDevices.find((item) => item.deviceId === selectedDeviceId) ?? null
  const connecting = connectingDeviceId === selectedDeviceId
  const connected = connectedDeviceId === selectedDeviceId
  const support = supportState?.deviceId === selectedDeviceId ? supportState.value : 'loading'

  function selectDevice(deviceId: string) {
    setSearchParams({ deviceId })
  }

  const closeActiveTerminal = useCallback(() => {
    const activeSession = activeSessionRef.current
    activeSessionRef.current = null
    queueMicrotask(() => {
      setConnectedDeviceId(null)
      setTerminalSessionActive(false)
    })
    if (activeSession) {
      void closeTerminal(activeSession.deviceId, activeSession.sessionId)
    }
  }, [setTerminalSessionActive])

  useEffect(() => {
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'Consolas, "Cascadia Mono", monospace',
      fontSize: 13,
      theme: { background: '#111827' },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminalRef.current = terminal
    fitRef.current = fit

    terminal.onData((data) => {
      const activeSession = activeSessionRef.current
      if (activeSession) void writeTerminal(activeSession.deviceId, activeSession.sessionId, data)
    })
    terminal.onResize(({ cols, rows }) => {
      const activeSession = activeSessionRef.current
      if (activeSession) void resizeTerminal(activeSession.deviceId, activeSession.sessionId, cols, rows)
    })

    let unlisten: (() => void) | undefined
    void listen<TerminalEvent>('terminal-event', ({ payload }) => {
      if (payload.sessionId !== activeSessionRef.current?.sessionId) return
      if (payload.kind === 'output' && payload.data) {
        terminal.write(Uint8Array.from(atob(payload.data), (char) => char.charCodeAt(0)))
      }
      if (payload.kind === 'failed' || payload.kind === 'closed') {
        activeSessionRef.current = null
        setConnectedDeviceId(null)
        setConnectingDeviceId(null)
        setTerminalSessionActive(false)
        setTerminalVersion((version) => version + 1)
      }
    }).then((value) => { unlisten = value })

    return () => {
      unlisten?.()
      terminal.dispose()
      terminalRef.current = null
      fitRef.current = null
    }
  }, [setTerminalSessionActive, terminalVersion])

  useEffect(() => {
    const terminal = terminalRef.current
    const container = containerRef.current
    const fit = fitRef.current
    if (!terminal || !container || !fit) return

    if (!terminal.element) {
      terminal.open(container)
    }
    const resizeObserver = new ResizeObserver(() => fit.fit())
    resizeObserver.observe(container)
    requestAnimationFrame(() => fit.fit())
    return () => resizeObserver.disconnect()
  }, [selectedDeviceId, support, terminalVersion])

  useEffect(() => {
    generationRef.current += 1
    closeActiveTerminal()
    setTerminalVersion((version) => version + 1)
  }, [closeActiveTerminal, selectedDeviceId])

  useEffect(() => {
    if (!selectedDeviceId) return
    let cancelled = false
    let retryTimer: number | undefined
    const refreshSupport = () => {
      void getRemoteTerminalSupport(selectedDeviceId).then(
        (nextSupport) => {
          if (cancelled) return
          setSupportState({ deviceId: selectedDeviceId, value: nextSupport })
          if (nextSupport === 'unknown') retryTimer = window.setTimeout(refreshSupport, 1000)
        },
        () => {
          if (!cancelled) {
            setSupportState({ deviceId: selectedDeviceId, value: 'unknown' })
            retryTimer = window.setTimeout(refreshSupport, 1000)
          }
        },
      )
    }
    refreshSupport()
    return () => {
      cancelled = true
      if (retryTimer !== undefined) window.clearTimeout(retryTimer)
    }
  }, [selectedDeviceId])

  useEffect(() => () => closeActiveTerminal(), [closeActiveTerminal])

  const connect = useCallback(async () => {
    const terminal = terminalRef.current
    if (!terminal || !selectedDevice || connecting || connected || support === 'unsupported') return

    const generation = generationRef.current
    setConnectingDeviceId(selectedDevice.deviceId)
    closeActiveTerminal()
    try {
      const sessionId = await openTerminal(selectedDevice.deviceId, terminal.cols, terminal.rows)
      if (generation !== generationRef.current || selectedDevice.deviceId !== selectedDeviceId) {
        void closeTerminal(selectedDevice.deviceId, sessionId)
        return
      }
      activeSessionRef.current = { deviceId: selectedDevice.deviceId, sessionId }
      setConnectedDeviceId(selectedDevice.deviceId)
      setTerminalSessionActive(true)
    } catch {
      if (generation === generationRef.current) {
        setTerminalVersion((version) => version + 1)
      }
    } finally {
      if (generation === generationRef.current) setConnectingDeviceId(null)
    }
  }, [closeActiveTerminal, connected, connecting, selectedDevice, selectedDeviceId, setTerminalSessionActive, support, t])

  useEffect(() => {
    if (support === 'loading' || support === 'unsupported' || !selectedDeviceId || autoConnectDeviceRef.current === selectedDeviceId) return
    autoConnectDeviceRef.current = selectedDeviceId
    void connect()
  }, [connect, selectedDeviceId, support])

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] overflow-hidden animate-fade-in">
      <aside className="h-full overflow-y-auto border-r py-6 pl-8 pr-4 scrollbar-thin">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">
          {t('terminal.sidebarTitle')}
        </div>
        {terminalDevices.length === 0 ? (
          <div className="px-1 py-8 text-center text-[13px] text-[hsl(var(--muted))]">{t('terminal.emptyDevices')}</div>
        ) : (
          <div className="space-y-1">
            {terminalDevices.map((item) => (
              <button
                className={cn(
                  'w-full rounded-lg border px-3 py-2.5 text-left transition-all',
                  item.deviceId === selectedDeviceId
                    ? 'border-[hsl(var(--text)/0.25)] bg-[hsl(var(--panel))] shadow-sm'
                    : 'border-transparent hover:bg-[hsl(var(--panel-2)/0.5)]',
                )}
                key={item.deviceId}
                onClick={() => selectDevice(item.deviceId)}
                type="button"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-[13px] font-medium text-[hsl(var(--text))]">{item.name}</span>
                  <span className={cn('h-1.5 w-1.5 shrink-0 rounded-full', item.online ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--muted))]')} />
                </div>
                <div className="mt-1 truncate text-[11px] text-[hsl(var(--muted))]">{formatPlatformName(item.type, t)}</div>
              </button>
            ))}
          </div>
        )}
      </aside>

      <main className="min-w-0 h-full overflow-y-auto px-8 py-6 scrollbar-thin">
        {!selectedDevice ? (
          <div className="flex h-full min-h-[360px] flex-col items-center justify-center text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]"><TerminalIcon className="h-6 w-6" /></div>
            <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">{t('terminal.selectDevice')}</div>
            <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">{t('terminal.selectDeviceDescription')}</p>
          </div>
        ) : (
          <div className="mx-auto flex h-full max-w-6xl flex-col">
            <header className="mb-5 flex items-center gap-4">
              <div className="flex min-w-0 items-center gap-3">
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]"><TerminalIcon className="h-5 w-5" /></div>
                <div className="min-w-0"><div className="truncate text-[15px] font-semibold text-[hsl(var(--text))]">{selectedDevice.name}</div><div className="mt-0.5 flex items-center gap-2 text-[12px] text-[hsl(var(--muted))]"><span>{formatPlatformName(selectedDevice.type, t)}</span><span>·</span>{connected ? <span className="text-[hsl(var(--success))]">{t('terminal.connected')}</span> : connecting ? <span>{t('terminal.connecting')}</span> : <><span>{t('terminal.disconnected')}</span><span>·</span><button className="text-[hsl(var(--primary))] underline underline-offset-2 disabled:cursor-not-allowed disabled:no-underline disabled:opacity-50" disabled={support === 'loading' || support === 'unsupported'} onClick={() => void connect()} type="button">{t('terminal.reconnect')}</button></>}</div></div>
              </div>
            </header>
            {support === 'loading' ? (
              <div className="flex min-h-[300px] flex-1 items-center justify-center">
                <LoaderCircle className="h-5 w-5 animate-spin text-[hsl(var(--muted))]" />
              </div>
            ) : support === 'unsupported' ? (
              <TerminalSupportState support={support} />
            ) : (
              <section className="relative min-h-[360px] flex-1 overflow-hidden rounded-xl border bg-[#111827] p-3 shadow-sm">
                <div className="h-full w-full" ref={containerRef} />
              </section>
            )}
          </div>
        )}
      </main>
    </div>
  )
}

function TerminalSupportState({ support }: { support: RemoteTerminalSupport }) {
  const { t } = useTranslation()
  const unsupported = support === 'unsupported'
  return (
    <div className="flex min-h-[300px] flex-1 flex-col items-center justify-center text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]"><TerminalIcon className="h-6 w-6" /></div>
      <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">{t(unsupported ? 'terminal.unsupportedTitle' : 'terminal.versionUnknownTitle')}</div>
      <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">{t(unsupported ? 'terminal.unsupportedDescription' : 'terminal.versionUnknownDescription')}</p>
    </div>
  )
}
