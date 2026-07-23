import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { useSearchParams } from 'react-router-dom'
import { Camera, LoaderCircle, RefreshCw, Square, Video } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { useAppState } from '../hooks/use-app-state'
import { closeRemoteCamera, getRemoteCameraSupport, listRemoteCameras, openRemoteCamera, sendCameraAlive } from '../lib/api'
import { isReleaseBuild } from '../lib/app-meta'
import type { CameraEntry, RemoteCameraSupport } from '../lib/types'
import { cn, formatPlatformName } from '../lib/utils'
import { Button } from '../components/ui/button'

interface CameraEvent {
  sessionId: string
  kind: string
  data?: string
  codec?: 'h264' | 'webp' | 'jpeg'
  transport?: 'lan' | 'relay'
  width?: number
  height?: number
  fps?: number
  keyframe?: boolean
  sequence?: number
  timestampMs?: number
  message?: string
}

interface CameraDebugCounters {
  sessionId: string
  codec: string
  transport: string
  width: number
  height: number
  fps: number
  startedAt: number
  intervalStartedAt: number
  receivedFrames: number
  receivedBytes: number
  decodedFrames: number
  renderedFrames: number
  keyframes: number
  sequenceGaps: number
  missingFrames: number
  decodeDrops: number
  renderDrops: number
  decodeErrors: number
  lastSequence?: number
  lastFrameBytes: number
  lastNalTypes: string
  baseArrivalMs?: number
  baseTimestampMs?: number
  delayDriftMs: number
}

interface CameraDebugSnapshot {
  sessionId: string
  codec: string
  transport: string
  width: number
  height: number
  fps: number
  elapsedSeconds: number
  receiveFps: number
  receiveKbps: number
  decodeFps: number
  renderFps: number
  keyframes: number
  sequenceGaps: number
  missingFrames: number
  decodeDrops: number
  renderDrops: number
  decodeErrors: number
  decodeQueue: number
  waitingForKeyframe: boolean
  lastSequence?: number
  lastFrameBytes: number
  lastNalTypes: string
  delayDriftMs: number
}

const CAMERA_DEBUG_LOGGING_ENABLED = !isReleaseBuild

function newCameraDebugCounters(sessionId = ''): CameraDebugCounters {
  const now = performance.now()
  return {
    sessionId,
    codec: '',
    transport: '',
    width: 0,
    height: 0,
    fps: 0,
    startedAt: now,
    intervalStartedAt: now,
    receivedFrames: 0,
    receivedBytes: 0,
    decodedFrames: 0,
    renderedFrames: 0,
    keyframes: 0,
    sequenceGaps: 0,
    missingFrames: 0,
    decodeDrops: 0,
    renderDrops: 0,
    decodeErrors: 0,
    lastFrameBytes: 0,
    lastNalTypes: '',
    delayDriftMs: 0,
  }
}

function annexBNalTypes(bytes: Uint8Array) {
  const names = new Map([
    [1, 'P'],
    [5, 'IDR'],
    [6, 'SEI'],
    [7, 'SPS'],
    [8, 'PPS'],
    [9, 'AUD'],
  ])
  const types: string[] = []
  for (let index = 0; index + 3 < bytes.length; index += 1) {
    let nalOffset = -1
    if (bytes[index] === 0 && bytes[index + 1] === 0 && bytes[index + 2] === 1) {
      nalOffset = index + 3
    } else if (
      index + 4 < bytes.length &&
      bytes[index] === 0 &&
      bytes[index + 1] === 0 &&
      bytes[index + 2] === 0 &&
      bytes[index + 3] === 1
    ) {
      nalOffset = index + 4
    }
    if (nalOffset >= 0 && nalOffset < bytes.length) {
      const type = bytes[nalOffset] & 0x1f
      types.push(names.get(type) ?? String(type))
      index = nalOffset
    }
  }
  return types.join(',')
}

export function CameraPage() {
  const { t } = useTranslation()
  const { devices, device } = useAppState()
  const [searchParams, setSearchParams] = useSearchParams()

  const [supportState, setSupportState] = useState<{ deviceId: string; value: RemoteCameraSupport } | null>(null)
  const [cameras, setCameras] = useState<CameraEntry[]>([])
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [streamReady, setStreamReady] = useState(false)
  const [hasFrame, setHasFrame] = useState(false)
  const [loading, setLoading] = useState(false)
  const [fetchingCameras, setFetchingCameras] = useState(false)
  const [debugStats, setDebugStats] = useState<CameraDebugSnapshot | null>(null)
  const sessionRef = useRef<string | null>(null)
  const decoderRef = useRef<VideoDecoder | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const expectedSequenceRef = useRef<number | null>(null)
  const decoderSyncedRef = useRef(false)
  const imageDecodeGenerationRef = useRef(0)
  const pendingVideoFrameRef = useRef<VideoFrame | null>(null)
  const renderAnimationRef = useRef<number | null>(null)
  const debugCountersRef = useRef(newCameraDebugCounters())

  const cameraDevices = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId && item.online),
    [device?.deviceId, devices],
  )

  const selectedDeviceId = useMemo(() => {
    const requestedDeviceId = searchParams.get('deviceId')
    if (requestedDeviceId && cameraDevices.some((item) => item.deviceId === requestedDeviceId)) {
      return requestedDeviceId
    }
    return cameraDevices[0]?.deviceId ?? ''
  }, [searchParams, cameraDevices])

  const selectedDevice = cameraDevices.find((item) => item.deviceId === selectedDeviceId) ?? null
  const support = supportState?.deviceId === selectedDeviceId ? supportState.value : 'loading'

  function selectDevice(deviceId: string) {
    setSearchParams({ deviceId })
  }

  const closeStream = useCallback(() => {
    const currentSession = sessionRef.current
    sessionRef.current = null
    setSessionId(null)
    setStreamReady(false)
    setHasFrame(false)
    setDebugStats(null)
    debugCountersRef.current = newCameraDebugCounters()
    imageDecodeGenerationRef.current += 1
    expectedSequenceRef.current = null
    decoderSyncedRef.current = false
    decoderRef.current?.close()
    decoderRef.current = null
    pendingVideoFrameRef.current?.close()
    pendingVideoFrameRef.current = null
    if (renderAnimationRef.current !== null) {
      window.cancelAnimationFrame(renderAnimationRef.current)
      renderAnimationRef.current = null
    }
    if (currentSession && selectedDeviceId) {
      void closeRemoteCamera(selectedDeviceId, currentSession)
    }
  }, [selectedDeviceId])

  useEffect(() => {
    const timer = window.setInterval(() => {
      const counters = debugCountersRef.current
      if (!counters.sessionId) return
      const now = performance.now()
      const intervalSeconds = Math.max((now - counters.intervalStartedAt) / 1_000, 0.001)
      const snapshot: CameraDebugSnapshot = {
        sessionId: counters.sessionId,
        codec: counters.codec,
        transport: counters.transport,
        width: counters.width,
        height: counters.height,
        fps: counters.fps,
        elapsedSeconds: (now - counters.startedAt) / 1_000,
        receiveFps: counters.receivedFrames / intervalSeconds,
        receiveKbps: (counters.receivedBytes * 8) / intervalSeconds / 1_000,
        decodeFps: counters.decodedFrames / intervalSeconds,
        renderFps: counters.renderedFrames / intervalSeconds,
        keyframes: counters.keyframes,
        sequenceGaps: counters.sequenceGaps,
        missingFrames: counters.missingFrames,
        decodeDrops: counters.decodeDrops,
        renderDrops: counters.renderDrops,
        decodeErrors: counters.decodeErrors,
        decodeQueue: decoderRef.current?.decodeQueueSize ?? 0,
        waitingForKeyframe: !decoderSyncedRef.current,
        lastSequence: counters.lastSequence,
        lastFrameBytes: counters.lastFrameBytes,
        lastNalTypes: counters.lastNalTypes,
        delayDriftMs: counters.delayDriftMs,
      }
      setDebugStats(snapshot)
      counters.intervalStartedAt = now
      counters.receivedFrames = 0
      counters.receivedBytes = 0
      counters.decodedFrames = 0
      counters.renderedFrames = 0
    }, 1_000)
    return () => window.clearInterval(timer)
  }, [])

  // Reset stream when changing selected device
  useEffect(() => {
    closeStream()
    setCameras([])
  }, [closeStream, selectedDeviceId])

  // Query remote camera support
  useEffect(() => {
    if (!selectedDeviceId) return
    let cancelled = false
    let retryTimer: number | undefined

    const checkSupport = () => {
      void getRemoteCameraSupport(selectedDeviceId).then(
        (nextSupport) => {
          if (cancelled) return
          setSupportState({ deviceId: selectedDeviceId, value: nextSupport })
          if (nextSupport === 'unknown') {
            retryTimer = window.setTimeout(checkSupport, 1000)
          }
        },
        () => {
          if (!cancelled) {
            setSupportState({ deviceId: selectedDeviceId, value: 'unknown' })
            retryTimer = window.setTimeout(checkSupport, 1000)
          }
        },
      )
    }

    checkSupport()
    return () => {
      cancelled = true
      if (retryTimer !== undefined) window.clearTimeout(retryTimer)
    }
  }, [selectedDeviceId])

  // Automatically fetch cameras if supported and no active stream
  const fetchCameras = useCallback(async () => {
    if (!selectedDeviceId || support !== 'supported') return
    setFetchingCameras(true)
    try {
      const list = await listRemoteCameras(selectedDeviceId)
      setCameras(list)
    } catch {
      setCameras([])
      toast.error(t('camera.listFailed'))
    } finally {
      setFetchingCameras(false)
    }
  }, [selectedDeviceId, support, t])

  useEffect(() => {
    if (support === 'supported' && !sessionId) {
      void fetchCameras()
    }
  }, [fetchCameras, sessionId, support])

  // Listen to camera events
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    let canvasContext: CanvasRenderingContext2D | null = null
    const getContext = () => {
      const canvas = canvasRef.current
      if (!canvas) return null
      if (!canvasContext || canvasContext.canvas !== canvas) {
        canvasContext = canvas.getContext('2d', { alpha: false, desynchronized: true })
      }
      return canvasContext
    }

    const base64ToBytes = (data: string) => {
      const binary = atob(data)
      const bytes = new Uint8Array(binary.length)
      for (let index = 0; index < binary.length; index += 1) {
        bytes[index] = binary.charCodeAt(index)
      }
      return bytes
    }

    const scheduleVideoFrameRender = (frame: VideoFrame) => {
      if (disposed) {
        frame.close()
        return
      }
      const counters = debugCountersRef.current
      counters.decodedFrames += 1
      if (pendingVideoFrameRef.current) {
        counters.renderDrops += 1
        pendingVideoFrameRef.current.close()
      }
      pendingVideoFrameRef.current = frame
      if (renderAnimationRef.current !== null) return
      renderAnimationRef.current = window.requestAnimationFrame(() => {
        renderAnimationRef.current = null
        const nextFrame = pendingVideoFrameRef.current
        pendingVideoFrameRef.current = null
        if (!nextFrame) return
        const canvas = canvasRef.current
        const context = getContext()
        if (canvas && context) {
          if (
            canvas.width !== nextFrame.displayWidth ||
            canvas.height !== nextFrame.displayHeight
          ) {
            canvas.width = nextFrame.displayWidth
            canvas.height = nextFrame.displayHeight
          }
          context.drawImage(nextFrame, 0, 0)
          counters.renderedFrames += 1
          setHasFrame(true)
        }
        nextFrame.close()
      })
    }

    const createDecoder = () => {
      const decoder = new VideoDecoder({
        output: scheduleVideoFrameRender,
        error: () => {
          // Keep the session alive; wait for the next keyframe instead of tearing down.
          if (decoderRef.current === decoder) {
            decoderSyncedRef.current = false
          }
          debugCountersRef.current.decodeErrors += 1
        },
      })
      decoder.configure({
        codec: 'avc1.42001f',
        optimizeForLatency: true,
        hardwareAcceleration: 'prefer-hardware',
      } as VideoDecoderConfig)
      decoderRef.current = decoder
      return decoder
    }

    void listen<CameraEvent>('camera-event', ({ payload }) => {
      if (disposed) return
      if (payload.sessionId !== sessionRef.current) return
      if (payload.kind === 'frame' && payload.data) {
        if (payload.codec === 'h264' && payload.sequence !== undefined) {
          const bytes = base64ToBytes(payload.data)
          const counters = debugCountersRef.current
          counters.receivedFrames += 1
          counters.receivedBytes += bytes.byteLength
          counters.lastFrameBytes = bytes.byteLength
          counters.lastSequence = payload.sequence
          if (payload.timestampMs !== undefined) {
            const arrival = performance.now()
            if (counters.baseArrivalMs === undefined || counters.baseTimestampMs === undefined) {
              counters.baseArrivalMs = arrival
              counters.baseTimestampMs = payload.timestampMs
            }
            counters.delayDriftMs =
              arrival - counters.baseArrivalMs - (payload.timestampMs - counters.baseTimestampMs)
          }
          if (
            expectedSequenceRef.current !== null &&
            payload.sequence !== expectedSequenceRef.current
          ) {
            // Sequence gap: do NOT reset/reconfigure the decoder (that freezes the picture).
            // Wait for the next keyframe and resume. This was a major source of stutter/garbled video.
            decoderSyncedRef.current = false
            counters.sequenceGaps += 1
            if (payload.sequence > expectedSequenceRef.current) {
              counters.missingFrames += payload.sequence - expectedSequenceRef.current
            }
          }
          expectedSequenceRef.current = payload.sequence + 1
          if (!decoderSyncedRef.current && !payload.keyframe) {
            counters.decodeDrops += 1
            return
          }

          let decoder =
            decoderRef.current?.state === 'configured' ? decoderRef.current : null
          if (!decoder) {
            try {
              decoder = createDecoder()
            } catch {
              return
            }
          }
          // Bound decode backlog under bursty delivery so latency stays live.
          if (decoder.decodeQueueSize > 2) {
            decoderSyncedRef.current = false
            counters.decodeDrops += 1
            return
          }
          decoderSyncedRef.current = true
          if (payload.keyframe) {
            counters.keyframes += 1
            counters.lastNalTypes = annexBNalTypes(bytes)
          }
          try {
            decoder.decode(
              new EncodedVideoChunk({
                type: payload.keyframe ? 'key' : 'delta',
                timestamp: (payload.timestampMs ?? payload.sequence) * 1000,
                data: bytes,
              }),
            )
          } catch (error) {
            decoderSyncedRef.current = false
            counters.decodeErrors += 1
          }
        } else if (payload.codec === 'jpeg' || payload.codec === 'webp') {
          const generation = ++imageDecodeGenerationRef.current
          const bytes = base64ToBytes(payload.data)
          const counters = debugCountersRef.current
          counters.receivedFrames += 1
          counters.receivedBytes += bytes.byteLength
          counters.lastFrameBytes = bytes.byteLength
          counters.lastSequence = payload.sequence
          counters.keyframes += 1
          void createImageBitmap(new Blob([bytes], { type: `image/${payload.codec}` }))
            .then((bitmap) => {
              if (disposed || generation !== imageDecodeGenerationRef.current) {
                bitmap.close()
                return
              }
              const canvas = canvasRef.current
              const context = getContext()
              if (canvas && context) {
                if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
                  canvas.width = bitmap.width
                  canvas.height = bitmap.height
                }
                context.drawImage(bitmap, 0, 0)
                counters.decodedFrames += 1
                counters.renderedFrames += 1
                setHasFrame(true)
              }
              bitmap.close()
            })
            .catch(() => {
              counters.decodeErrors += 1
            })
        }
      }
      if (payload.kind === 'opened' && payload.codec) {
        setHasFrame(false)
        setStreamReady(true)
        imageDecodeGenerationRef.current += 1
        expectedSequenceRef.current = null
        decoderSyncedRef.current = false
        debugCountersRef.current = {
          ...newCameraDebugCounters(payload.sessionId),
          codec: payload.codec,
          transport: payload.transport ?? 'unknown',
          width: payload.width ?? 0,
          height: payload.height ?? 0,
          fps: payload.fps ?? 0,
        }
        if (CAMERA_DEBUG_LOGGING_ENABLED) {
          console.info(
            `[Camera][viewer] session=${payload.sessionId.slice(0, 8)} opened transport=${payload.transport ?? 'unknown'} ` +
              `codec=${payload.codec} stream=${payload.width ?? 0}x${payload.height ?? 0}@${payload.fps ?? 0}`,
          )
        }
        if (payload.codec === 'h264') {
          try {
            decoderRef.current?.close()
          } catch {
            // ignore
          }
          decoderRef.current = null
          try {
            createDecoder()
          } catch {
            // Decoder will be created lazily on the first keyframe.
          }
        }
      }
      if (payload.kind === 'closed' || payload.kind === 'failed') {
        if (payload.kind === 'failed' || payload.message) {
          toast.error(t('camera.streamFailed'))
        } else {
          toast.info(t('camera.streamClosed'))
        }
        try {
          decoderRef.current?.close()
        } catch {
          // ignore
        }
        decoderRef.current = null
        pendingVideoFrameRef.current?.close()
        pendingVideoFrameRef.current = null
        if (renderAnimationRef.current !== null) {
          window.cancelAnimationFrame(renderAnimationRef.current)
          renderAnimationRef.current = null
        }
        sessionRef.current = null
        setSessionId(null)
        setStreamReady(false)
        setHasFrame(false)
        setDebugStats(null)
        if (CAMERA_DEBUG_LOGGING_ENABLED) {
          console.info(
            `[Camera][viewer] session=${payload.sessionId.slice(0, 8)} ${payload.kind} message=${payload.message ?? ''}`,
          )
        }
        debugCountersRef.current = newCameraDebugCounters()
        imageDecodeGenerationRef.current += 1
        expectedSequenceRef.current = null
        decoderSyncedRef.current = false
      }
    }).then((value) => {
      if (disposed) {
        value()
      } else {
        unlisten = value
      }
    }).catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
      imageDecodeGenerationRef.current += 1
      try {
        decoderRef.current?.close()
      } catch {
        // Decoder may already be closed.
      }
      decoderRef.current = null
      pendingVideoFrameRef.current?.close()
      pendingVideoFrameRef.current = null
      if (renderAnimationRef.current !== null) {
        window.cancelAnimationFrame(renderAnimationRef.current)
        renderAnimationRef.current = null
      }
    }
  }, [t])

  // Heartbeat to keep camera stream alive
  useEffect(() => {
    if (!sessionId || !selectedDeviceId || !streamReady) return
    const sendAlive = () => {
      void sendCameraAlive(selectedDeviceId, sessionId).catch(() => {
        if (sessionRef.current !== sessionId) return
        toast.error(t('camera.streamFailed'))
        closeStream()
      })
    }
    const timer = window.setInterval(() => {
      sendAlive()
    }, 5000)
    sendAlive()
    return () => window.clearInterval(timer)
  }, [closeStream, selectedDeviceId, sessionId, streamReady, t])

  // Clean up on unmount
  useEffect(() => () => closeStream(), [closeStream])

  const openStream = async (camera: CameraEntry) => {
    if (!selectedDeviceId) return
    setLoading(true)
    setStreamReady(false)
    try {
      const preferredCodecs = ['webp', 'jpeg']
      if ('VideoDecoder' in window) {
        const support = await VideoDecoder.isConfigSupported({ codec: 'avc1.42001f' }).catch(() => null)
        if (support?.supported) preferredCodecs.unshift('h264')
      }
      const id = await openRemoteCamera(selectedDeviceId, camera.cameraId, preferredCodecs)
      sessionRef.current = id
      setSessionId(id)
      debugCountersRef.current = newCameraDebugCounters(id)
      if (CAMERA_DEBUG_LOGGING_ENABLED) {
        console.info(
          `[Camera][viewer] session=${id.slice(0, 8)} request device=${selectedDeviceId.slice(0, 8)} camera=${camera.cameraId} codecs=${preferredCodecs.join(',')}`,
        )
      }
    } catch {
      toast.error(t('camera.openFailed'))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] overflow-hidden animate-fade-in">
      <aside className="h-full overflow-y-auto border-r py-6 pl-8 pr-4 scrollbar-thin">
        <div className="px-1 pb-2 text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">
          {t('camera.sidebarTitle')}
        </div>
        {cameraDevices.length === 0 ? (
          <div className="px-1 py-8 text-center text-[13px] text-[hsl(var(--muted))]">{t('camera.emptyDevices')}</div>
        ) : (
          <div className="space-y-1">
            {cameraDevices.map((item) => (
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
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]">
              <Camera className="h-6 w-6" />
            </div>
            <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">{t('camera.selectDevice')}</div>
            <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">{t('camera.selectDeviceDescription')}</p>
          </div>
        ) : (
          <div className="mx-auto flex h-full max-w-6xl flex-col">
            <header className="mb-5 flex items-center justify-between gap-4">
              <div className="flex min-w-0 items-center gap-3">
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]">
                  <Camera className="h-5 w-5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[15px] font-semibold text-[hsl(var(--text))]">{selectedDevice.name}</div>
                  <div className="mt-0.5 flex items-center gap-2 text-[12px] text-[hsl(var(--muted))]">
                    <span>{formatPlatformName(selectedDevice.type, t)}</span>
                    {support === 'supported' && !sessionId && (
                      <>
                        <span>·</span>
                        <span>{t('camera.selectCameraDescription')}</span>
                      </>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex shrink-0 items-center gap-2">
                {sessionId ? (
                  <Button onClick={closeStream} size="sm" variant="danger">
                    <Square className="h-3.5 w-3.5" />
                    {t('camera.closeStream')}
                  </Button>
                ) : support === 'supported' ? (
                  <Button disabled={fetchingCameras} onClick={() => void fetchCameras()} size="sm" variant="secondary">
                    <RefreshCw className={cn('mr-1.5 h-3.5 w-3.5', fetchingCameras && 'animate-spin')} />
                    {t('camera.refresh')}
                  </Button>
                ) : null}
              </div>
            </header>

            {support === 'loading' ? (
              <div className="flex min-h-[300px] flex-1 items-center justify-center">
                <LoaderCircle className="h-5 w-5 animate-spin text-[hsl(var(--muted))]" />
              </div>
            ) : support === 'unsupported' ? (
              <CameraSupportState support={support} />
            ) : sessionId ? (
              <>
                <section className="flex flex-1 items-center justify-center py-4">
                  <div className="aspect-video w-full max-w-5xl overflow-hidden rounded-xl border bg-black shadow-sm">
                    <div className="flex h-full w-full items-center justify-center">
                      <canvas className={cn('h-full w-full object-contain', !hasFrame && 'hidden')} ref={canvasRef} />
                      {!hasFrame && (
                        <div className="flex flex-col items-center justify-center gap-2 text-white/70">
                          <LoaderCircle className="h-6 w-6 animate-spin" />
                          <span className="text-sm">{t('camera.waitingStream')}</span>
                        </div>
                      )}
                    </div>
                  </div>
                </section>
                <CameraInfoPanel stats={debugStats} />
              </>
            ) : (
              <section className="flex-1">
                {fetchingCameras ? (
                  <div className="flex min-h-[200px] items-center justify-center">
                    <LoaderCircle className="h-5 w-5 animate-spin text-[hsl(var(--muted))]" />
                  </div>
                ) : cameras.length === 0 ? (
                  <div className="flex min-h-[200px] flex-col items-center justify-center rounded-xl border border-dashed text-center">
                    <Video className="h-8 w-8 text-[hsl(var(--muted))]" />
                    <div className="mt-2 text-[13px] font-medium text-[hsl(var(--text))]">{t('camera.noCamerasFound')}</div>
                  </div>
                ) : (
                  <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                    {cameras.map((camera) => (
                      <button
                        className="flex items-start gap-3 rounded-xl border bg-[hsl(var(--panel))] p-4 text-left transition-all hover:border-[hsl(var(--text)/0.25)] hover:bg-[hsl(var(--panel-2))] hover:shadow-sm"
                        disabled={loading}
                        key={camera.cameraId}
                        onClick={() => void openStream(camera)}
                        type="button"
                      >
                        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
                          <Camera className="h-5 w-5" />
                        </div>
                        <div className="min-w-0">
                          <div className="truncate font-medium text-[14px] text-[hsl(var(--text))]">{camera.label}</div>
                          <div className="mt-1 truncate text-[12px] text-[hsl(var(--muted))]">{camera.position ?? camera.cameraId}</div>
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </section>
            )}
          </div>
        )}
      </main>
    </div>
  )
}

function CameraInfoPanel({ stats }: { stats: CameraDebugSnapshot | null }) {
  const { t } = useTranslation()
  const value = stats
  return (
    <section className="mt-4 rounded-xl border bg-[hsl(var(--panel))] p-4 text-[12px] text-[hsl(var(--text-secondary))]">
      <div className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-[hsl(var(--muted))]">
        {t('camera.infoTitle')}
      </div>
      <div className="grid gap-x-6 gap-y-2 md:grid-cols-2">
        <CameraDebugRow
          label={t('camera.debugSession')}
          value={value ? t('camera.infoSessionValue', { session: value.sessionId.slice(0, 8), seconds: value.elapsedSeconds.toFixed(0) }) : t('camera.debugUnknown')}
        />
        <CameraDebugRow
          label={t('camera.debugStream')}
          value={value ? t('camera.infoStreamValue', { codec: value.codec, transport: value.transport, width: value.width, height: value.height, fps: value.fps }) : t('camera.debugUnknown')}
        />
        <CameraDebugRow
          label={t('camera.debugReceive')}
          value={value ? t('camera.infoReceiveValue', { fps: value.receiveFps.toFixed(1), kbps: Math.round(value.receiveKbps), bytes: value.lastFrameBytes }) : t('camera.debugUnknown')}
        />
        <CameraDebugRow
          label={t('camera.debugDecoder')}
          value={
            value
              ? t('camera.infoDecoderValue', {
                decodeFps: value.decodeFps.toFixed(1),
                renderFps: value.renderFps.toFixed(1),
                queue: value.decodeQueue,
                sync: t(value.waitingForKeyframe ? 'camera.debugWaitingKeyframe' : 'camera.debugSynced'),
              })
              : t('camera.debugUnknown')
          }
        />
        <CameraDebugRow
          label={t('camera.debugIntegrity')}
          value={
            value
              ? t('camera.infoIntegrityValue', {
                gaps: value.sequenceGaps,
                missing: value.missingFrames,
                decodeDrops: value.decodeDrops,
                renderDrops: value.renderDrops,
                errors: value.decodeErrors,
              })
              : t('camera.debugUnknown')
          }
        />
        <CameraDebugRow
          label={t('camera.debugFrame')}
          value={
            value
              ? t('camera.infoFrameValue', {
                sequence: value.lastSequence ?? '-',
                keyframes: value.keyframes,
                drift: Math.round(value.delayDriftMs),
                nalTypes: value.lastNalTypes || '-',
              })
              : t('camera.debugUnknown')
          }
        />
      </div>
    </section>
  )
}

function CameraDebugRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 items-start justify-between gap-4">
      <span className="shrink-0 text-[hsl(var(--muted))]">{label}</span>
      <span className="min-w-0 break-all text-right font-mono text-[11px] text-[hsl(var(--text))]">{value}</span>
    </div>
  )
}

function CameraSupportState({ support }: { support: RemoteCameraSupport }) {
  const { t } = useTranslation()
  const unsupported = support === 'unsupported'
  return (
    <div className="flex min-h-[300px] flex-1 flex-col items-center justify-center text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-[hsl(var(--panel-2))] text-[hsl(var(--muted))]">
        <Camera className="h-6 w-6" />
      </div>
      <div className="mt-4 text-[15px] font-semibold text-[hsl(var(--text))]">
        {t(unsupported ? 'camera.unsupportedTitle' : 'camera.versionUnknownTitle')}
      </div>
      <p className="mt-1 max-w-sm text-[13px] leading-relaxed text-[hsl(var(--muted))]">
        {t(unsupported ? 'camera.unsupportedDescription' : 'camera.versionUnknownDescription')}
      </p>
    </div>
  )
}
