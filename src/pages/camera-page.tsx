import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Camera, Gauge, LoaderCircle, RefreshCw, Square, Video } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { DeviceSidebar, useTargetDevices } from '../components/device-sidebar'
import {
  closeRemoteCamera,
  configureRemoteCamera,
  getRemoteCameraSupport,
  getRemoteCameraTransportHint,
  listRemoteCameras,
  openRemoteCamera,
  sendCameraAlive,
} from '../lib/api'
import { isReleaseBuild } from '../lib/app-meta'
import type { CameraEntry, CameraMode, CameraResolution, RemoteCameraSupport } from '../lib/types'
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

interface PendingModeSwitch {
  mode: CameraMode
  frames: CameraEvent[]
  bytes: number
}

interface ModeSwitchRequest {
  mode: CameraMode
  source: 'auto' | 'manual'
  probeFromIndex?: number
}

interface AdaptiveCameraState {
  windowStartedAt: number
  windowReceived: number
  windowMissing: number
  windowStartDrift: number
  lastDrift: number
  lastFrameAt: number
  lastSequence?: number
  badWindows: number
  stableSince: number
  cooldownUntil: number
  probe?: { fromIndex: number; deadline: number }
}

const CAMERA_DEBUG_LOGGING_ENABLED = !isReleaseBuild
const CAMERA_MODE_WINDOW_MS = 5_000
const CAMERA_MODE_STABLE_UPGRADE_MS = 30_000
const CAMERA_MODE_PROBE_MS = 10_000
const CAMERA_MODE_UPGRADE_COOLDOWN_MS = 60_000
const CAMERA_MODE_STALL_MS = 1_800
const CAMERA_SWITCH_MAX_FRAMES = 120
const CAMERA_SWITCH_MAX_BYTES = 24 * 1024 * 1024

function cameraModeKey(mode: CameraMode) {
  return `${mode.width}x${mode.height}@${mode.fps}`
}

function cameraResolutionKey(resolution: Pick<CameraResolution, 'width' | 'height'>) {
  return `${resolution.width}x${resolution.height}`
}

function sameCameraMode(left: CameraMode | null | undefined, right: CameraMode | null | undefined) {
  return !!left && !!right && cameraModeKey(left) === cameraModeKey(right)
}

function cameraModeScore(mode: CameraMode) {
  return mode.width * mode.height * mode.fps
}

function fpsCandidates(resolution: CameraResolution) {
  const values = new Set<number>()
  for (const range of resolution.fps ?? []) {
    if (range.min <= 0 || range.max < range.min) continue
    values.add(range.min)
    values.add(range.max)
    for (const standard of [15, 24, 30, 60]) {
      if (standard >= range.min && standard <= range.max) values.add(standard)
    }
  }
  return [...values].sort((left, right) => left - right)
}

function buildCameraModeLadder(camera: CameraEntry) {
  const modes = new Map<string, CameraMode>()
  for (const resolution of camera.capabilities?.resolutions ?? []) {
    for (const fps of fpsCandidates(resolution)) {
      const mode = { width: resolution.width, height: resolution.height, fps }
      modes.set(cameraModeKey(mode), mode)
    }
  }
  return [...modes.values()].sort((left, right) => {
    const score = cameraModeScore(left) - cameraModeScore(right)
    if (score !== 0) return score
    const pixels = left.width * left.height - right.width * right.height
    return pixels !== 0 ? pixels : left.fps - right.fps
  })
}

function manualModeForResolution(resolution: CameraResolution): CameraMode | null {
  const rates = fpsCandidates(resolution)
  if (rates.length === 0) return null
  const fps = rates.includes(30)
    ? 30
    : [...rates].reverse().find((value) => value < 30) ?? rates[0]
  return { width: resolution.width, height: resolution.height, fps }
}

function initialCameraMode(modes: CameraMode[], transport: 'lan' | 'relay') {
  if (modes.length === 0) return null
  const targetPixels = transport === 'lan' ? 1280 * 720 : 640 * 360
  const targetFps = transport === 'lan' ? 30 : 15
  const eligible = modes.filter(
    (mode) => mode.width * mode.height <= targetPixels && mode.fps <= targetFps,
  )
  return eligible.at(-1) ?? modes[0]
}

function newAdaptiveCameraState(): AdaptiveCameraState {
  const now = performance.now()
  return {
    windowStartedAt: now,
    windowReceived: 0,
    windowMissing: 0,
    windowStartDrift: 0,
    lastDrift: 0,
    lastFrameAt: now,
    badWindows: 0,
    stableSince: now,
    cooldownUntil: 0,
  }
}

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
  const {
    devices: cameraDevices,
    selectedDeviceId,
    selectedDevice,
    selectDevice,
  } = useTargetDevices({
    onlineOnly: true,
    autoSelectFirst: false,
  })

  const [supportState, setSupportState] = useState<{ deviceId: string; value: RemoteCameraSupport } | null>(null)
  const [cameras, setCameras] = useState<CameraEntry[]>([])
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [streamReady, setStreamReady] = useState(false)
  const [hasFrame, setHasFrame] = useState(false)
  const [loading, setLoading] = useState(false)
  const [fetchingCameras, setFetchingCameras] = useState(false)
  const [debugStats, setDebugStats] = useState<CameraDebugSnapshot | null>(null)
  const [activeCamera, setActiveCamera] = useState<CameraEntry | null>(null)
  const [currentMode, setCurrentMode] = useState<CameraMode | null>(null)
  const [quality, setQuality] = useState('auto')
  const [configuring, setConfiguring] = useState(false)
  const sessionRef = useRef<string | null>(null)
  const decoderRef = useRef<VideoDecoder | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const expectedSequenceRef = useRef<number | null>(null)
  const decoderSyncedRef = useRef(false)
  const imageDecodeGenerationRef = useRef(0)
  const pendingVideoFrameRef = useRef<VideoFrame | null>(null)
  const renderAnimationRef = useRef<number | null>(null)
  const debugCountersRef = useRef(newCameraDebugCounters())
  const cameraModesRef = useRef<CameraMode[]>([])
  const currentModeRef = useRef<CameraMode | null>(null)
  const automaticModeRef = useRef(true)
  const configuringRef = useRef(false)
  const pendingModeSwitchRef = useRef<PendingModeSwitch | null>(null)
  const queuedModeSwitchRef = useRef<ModeSwitchRequest | null>(null)
  const switchGenerationRef = useRef(0)
  const decodeFrameRef = useRef<(event: CameraEvent) => void>(() => {})
  const resetDecoderRef = useRef<() => void>(() => {})
  const adaptiveRef = useRef(newAdaptiveCameraState())

  const support = supportState?.deviceId === selectedDeviceId ? supportState.value : 'loading'

  const closeStream = useCallback(() => {
    const currentSession = sessionRef.current
    sessionRef.current = null
    setSessionId(null)
    setStreamReady(false)
    setHasFrame(false)
    setDebugStats(null)
    setActiveCamera(null)
    setCurrentMode(null)
    setQuality('auto')
    setConfiguring(false)
    debugCountersRef.current = newCameraDebugCounters()
    cameraModesRef.current = []
    currentModeRef.current = null
    automaticModeRef.current = true
    configuringRef.current = false
    pendingModeSwitchRef.current = null
    queuedModeSwitchRef.current = null
    switchGenerationRef.current += 1
    adaptiveRef.current = newAdaptiveCameraState()
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

  const requestModeSwitch = useCallback((request: ModeSwitchRequest) => {
    queuedModeSwitchRef.current = request
    if (configuringRef.current) return
    configuringRef.current = true
    setConfiguring(true)
    const generation = switchGenerationRef.current

    const run = async () => {
      while (queuedModeSwitchRef.current) {
        const next = queuedModeSwitchRef.current
        queuedModeSwitchRef.current = null
        const activeSession = sessionRef.current
        if (!activeSession || !selectedDeviceId || sameCameraMode(currentModeRef.current, next.mode)) {
          continue
        }

        const pending: PendingModeSwitch = { mode: next.mode, frames: [], bytes: 0 }
        pendingModeSwitchRef.current = pending
        try {
          const ack = await configureRemoteCamera(
            selectedDeviceId,
            activeSession,
            next.mode.width,
            next.mode.height,
            next.mode.fps,
          )
          if (
            generation !== switchGenerationRef.current ||
            sessionRef.current !== activeSession
          ) {
            return
          }
          if (
            !ack.applied ||
            ack.width !== next.mode.width ||
            ack.height !== next.mode.height ||
            ack.fps !== next.mode.fps ||
            ack.effectiveFromSequence === undefined
          ) {
            throw new Error('camera mode acknowledgement did not match the requested mode')
          }

          pendingModeSwitchRef.current = null
          resetDecoderRef.current()
          currentModeRef.current = next.mode
          setCurrentMode(next.mode)
          const counters = debugCountersRef.current
          counters.width = next.mode.width
          counters.height = next.mode.height
          counters.fps = next.mode.fps
          counters.baseArrivalMs = undefined
          counters.baseTimestampMs = undefined
          counters.delayDriftMs = 0
          const adaptive = newAdaptiveCameraState()
          if (next.source === 'auto') {
            adaptive.cooldownUntil = performance.now() + CAMERA_MODE_UPGRADE_COOLDOWN_MS
            if (next.probeFromIndex !== undefined) {
              adaptive.probe = {
                fromIndex: next.probeFromIndex,
                deadline: performance.now() + CAMERA_MODE_PROBE_MS,
              }
            }
          }
          adaptiveRef.current = adaptive

          for (const frame of pending.frames) {
            if (
              frame.sequence !== undefined &&
              frame.sequence >= ack.effectiveFromSequence
            ) {
              adaptive.windowReceived += 1
              adaptive.lastFrameAt = performance.now()
              if (
                adaptive.lastSequence !== undefined &&
                frame.sequence > adaptive.lastSequence + 1
              ) {
                adaptive.windowMissing += frame.sequence - adaptive.lastSequence - 1
              }
              adaptive.lastSequence = frame.sequence
              decodeFrameRef.current(frame)
            }
          }
        } catch (error) {
          if (CAMERA_DEBUG_LOGGING_ENABLED) {
            console.error(
              `[Camera][viewer] session=${activeSession.slice(0, 8)} mode switch to ` +
                `${next.mode.width}x${next.mode.height}@${next.mode.fps} failed`,
              error,
            )
          }
          if (
            generation !== switchGenerationRef.current ||
            sessionRef.current !== activeSession
          ) {
            return
          }
          const buffered = pendingModeSwitchRef.current?.frames ?? []
          pendingModeSwitchRef.current = null
          let recoveryIndex = -1
          for (let index = buffered.length - 1; index >= 0; index -= 1) {
            if (buffered[index].keyframe) {
              recoveryIndex = index
              break
            }
          }
          if (recoveryIndex >= 0) {
            resetDecoderRef.current()
          }
          for (const frame of buffered.slice(Math.max(0, recoveryIndex))) {
            decodeFrameRef.current(frame)
          }
          if (next.source === 'manual') {
            const actualResolution = activeCamera?.capabilities?.resolutions.find(
              (resolution) =>
                resolution.width === currentModeRef.current?.width &&
                resolution.height === currentModeRef.current?.height,
            )
            if (actualResolution) {
              setQuality(cameraResolutionKey(actualResolution))
            } else {
              automaticModeRef.current = true
              adaptiveRef.current = newAdaptiveCameraState()
              setQuality('auto')
            }
          }
          toast.error(t('camera.qualitySwitchFailed'))
        }
      }

      if (generation === switchGenerationRef.current) {
        configuringRef.current = false
        setConfiguring(false)
      }
    }

    void run()
  }, [activeCamera, selectedDeviceId, t])

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (
        document.hidden ||
        !streamReady ||
        !automaticModeRef.current ||
        configuringRef.current
      ) {
        return
      }
      const mode = currentModeRef.current
      const modes = cameraModesRef.current
      if (!mode || modes.length < 2) return
      const currentIndex = modes.findIndex((candidate) => sameCameraMode(candidate, mode))
      if (currentIndex < 0) return

      const now = performance.now()
      const adaptive = adaptiveRef.current
      if (
        adaptive.lastSequence !== undefined &&
        now - adaptive.lastFrameAt >= CAMERA_MODE_STALL_MS &&
        currentIndex > 0
      ) {
        adaptive.badWindows = 0
        requestModeSwitch({ mode: modes[currentIndex - 1], source: 'auto' })
        return
      }
      const elapsed = now - adaptive.windowStartedAt
      if (elapsed < CAMERA_MODE_WINDOW_MS) return

      const expectedFrames = mode.fps * elapsed / 1_000
      const deliveryRatio = expectedFrames > 0 ? adaptive.windowReceived / expectedFrames : 1
      const observedFrames = adaptive.windowReceived + adaptive.windowMissing
      const gapRatio = observedFrames > 0 ? adaptive.windowMissing / observedFrames : 0
      const driftGrowth = adaptive.lastDrift - adaptive.windowStartDrift
      const badWindow = deliveryRatio < 0.75 || gapRatio > 0.05 || driftGrowth > 400

      adaptive.windowStartedAt = now
      adaptive.windowReceived = 0
      adaptive.windowMissing = 0
      adaptive.windowStartDrift = adaptive.lastDrift

      if (adaptive.probe) {
        if (badWindow) {
          const fallback = modes[adaptive.probe.fromIndex]
          adaptive.probe = undefined
          if (fallback) requestModeSwitch({ mode: fallback, source: 'auto' })
          return
        }
        if (now >= adaptive.probe.deadline) {
          adaptive.probe = undefined
          adaptive.stableSince = now
        }
      }

      if (badWindow) {
        adaptive.badWindows += 1
        adaptive.stableSince = now
        if (adaptive.badWindows >= 2 && currentIndex > 0) {
          adaptive.badWindows = 0
          requestModeSwitch({ mode: modes[currentIndex - 1], source: 'auto' })
        }
        return
      }

      adaptive.badWindows = 0
      if (
        !adaptive.probe &&
        currentIndex < modes.length - 1 &&
        now - adaptive.stableSince >= CAMERA_MODE_STABLE_UPGRADE_MS &&
        now >= adaptive.cooldownUntil
      ) {
        adaptive.cooldownUntil = now + CAMERA_MODE_UPGRADE_COOLDOWN_MS
        requestModeSwitch({
          mode: modes[currentIndex + 1],
          source: 'auto',
          probeFromIndex: currentIndex,
        })
      }
    }, 1_000)
    return () => window.clearInterval(timer)
  }, [requestModeSwitch, streamReady])

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

    const decodeFrame = (payload: CameraEvent) => {
      if (!payload.data) return
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
    decodeFrameRef.current = decodeFrame

    const resetDecoder = () => {
      imageDecodeGenerationRef.current += 1
      expectedSequenceRef.current = null
      decoderSyncedRef.current = false
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
    resetDecoderRef.current = resetDecoder

    void listen<CameraEvent>('camera-event', ({ payload }) => {
      if (disposed) return
      if (payload.sessionId !== sessionRef.current) return
      if (payload.kind === 'frame' && payload.data) {
        if (payload.sequence !== undefined) {
          const adaptive = adaptiveRef.current
          adaptive.windowReceived += 1
          adaptive.lastFrameAt = performance.now()
          if (
            adaptive.lastSequence !== undefined &&
            payload.sequence > adaptive.lastSequence + 1
          ) {
            adaptive.windowMissing += payload.sequence - adaptive.lastSequence - 1
          }
          adaptive.lastSequence = payload.sequence
          adaptive.lastDrift = debugCountersRef.current.delayDriftMs
        }
        const pending = pendingModeSwitchRef.current
        if (pending) {
          const frameBytes = Math.ceil(payload.data.length * 3 / 4)
          pending.frames.push(payload)
          pending.bytes += frameBytes
          while (
            pending.frames.length > CAMERA_SWITCH_MAX_FRAMES ||
            pending.bytes > CAMERA_SWITCH_MAX_BYTES
          ) {
            const removed = pending.frames.shift()
            if (!removed?.data) break
            pending.bytes = Math.max(0, pending.bytes - Math.ceil(removed.data.length * 3 / 4))
          }
        } else {
          decodeFrame(payload)
        }
      }
      if (payload.kind === 'opened' && payload.codec) {
        setHasFrame(false)
        setStreamReady(true)
        resetDecoder()
        const openedMode = payload.width && payload.height && payload.fps
          ? { width: payload.width, height: payload.height, fps: payload.fps }
          : null
        currentModeRef.current = openedMode
        setCurrentMode(openedMode)
        adaptiveRef.current = newAdaptiveCameraState()
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
        setActiveCamera(null)
        setCurrentMode(null)
        setQuality('auto')
        setConfiguring(false)
        if (CAMERA_DEBUG_LOGGING_ENABLED) {
          console.info(
            `[Camera][viewer] session=${payload.sessionId.slice(0, 8)} ${payload.kind} message=${payload.message ?? ''}`,
          )
        }
        debugCountersRef.current = newCameraDebugCounters()
        cameraModesRef.current = []
        currentModeRef.current = null
        automaticModeRef.current = true
        configuringRef.current = false
        pendingModeSwitchRef.current = null
        queuedModeSwitchRef.current = null
        switchGenerationRef.current += 1
        adaptiveRef.current = newAdaptiveCameraState()
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
      decodeFrameRef.current = () => {}
      resetDecoderRef.current = () => {}
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
      const modes = buildCameraModeLadder(camera)
      const preferredCodecs = ['webp', 'jpeg']
      if ('VideoDecoder' in window) {
        const support = await VideoDecoder.isConfigSupported({ codec: 'avc1.42001f' }).catch(() => null)
        if (support?.supported) preferredCodecs.unshift('h264')
      }
      const transport = modes.length > 0
        ? await getRemoteCameraTransportHint(selectedDeviceId).catch(() => 'relay' as const)
        : 'relay'
      const initialMode = initialCameraMode(modes, transport)
      const id = await openRemoteCamera(
        selectedDeviceId,
        camera.cameraId,
        preferredCodecs,
        initialMode?.width,
        initialMode?.height,
        initialMode?.fps,
      )
      sessionRef.current = id
      setSessionId(id)
      setActiveCamera(camera)
      setQuality('auto')
      setCurrentMode(initialMode)
      cameraModesRef.current = modes
      currentModeRef.current = initialMode
      automaticModeRef.current = true
      adaptiveRef.current = newAdaptiveCameraState()
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

  const changeQuality = (value: string) => {
    setQuality(value)
    if (value === 'auto') {
      automaticModeRef.current = true
      queuedModeSwitchRef.current = null
      adaptiveRef.current = newAdaptiveCameraState()
      return
    }
    automaticModeRef.current = false
    const resolution = activeCamera?.capabilities?.resolutions.find(
      (candidate) => cameraResolutionKey(candidate) === value,
    )
    const mode = resolution ? manualModeForResolution(resolution) : null
    if (mode) requestModeSwitch({ mode, source: 'manual' })
  }

  const qualityResolutions = (activeCamera?.capabilities?.resolutions ?? []).filter(
    (resolution) => manualModeForResolution(resolution) !== null,
  )

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)] overflow-hidden">
      <DeviceSidebar
        devices={cameraDevices}
        emptyText={t('camera.emptyDevices')}
        onSelectDevice={selectDevice}
        selectedDeviceId={selectedDeviceId}
        title={t('camera.sidebarTitle')}
      />

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
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
                  <Camera className="h-5 w-5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[15px] font-semibold text-[hsl(var(--text))]">{selectedDevice.name}</div>
                  <div className="mt-0.5 flex items-center gap-2 text-[12px] text-[hsl(var(--muted))]">
                    <span>{formatPlatformName(selectedDevice.type, t)}</span>
                    {activeCamera && sessionId && (
                      <>
                        <span>·</span>
                        <span className="truncate">{activeCamera.label}</span>
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
              <div className="flex flex-1 flex-col gap-5 pb-2">
                <section className="flex min-h-[300px] flex-1 items-center justify-center">
                  <div className="relative aspect-video w-full max-w-5xl overflow-hidden rounded-2xl border border-white/10 bg-black shadow-lg">
                    <div className="flex h-full w-full items-center justify-center">
                      <canvas className={cn('h-full w-full object-contain', !hasFrame && 'hidden')} ref={canvasRef} />
                      {!hasFrame && (
                        <div className="flex flex-col items-center justify-center gap-2 text-white/70">
                          <LoaderCircle className="h-6 w-6 animate-spin" />
                          <span className="text-sm">{t('camera.waitingStream')}</span>
                        </div>
                      )}
                    </div>
                    {hasFrame && debugStats && (
                      <div className="pointer-events-none absolute bottom-3 left-3 flex items-center gap-2 rounded-lg border border-white/10 bg-black/65 px-3 py-1.5 text-[11px] font-medium text-white/85 shadow-sm backdrop-blur-md">
                        <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                        <span>{debugStats.codec.toUpperCase()} · {debugStats.transport.toUpperCase()}</span>
                        <span className="text-white/35">·</span>
                        <span>{debugStats.width} × {debugStats.height} @ {debugStats.fps} FPS</span>
                      </div>
                    )}
                  </div>
                </section>
                <div className="grid shrink-0 gap-4">
                  {streamReady && (
                    <CameraQualityPanel
                      configuring={configuring}
                      currentMode={currentMode}
                      onChange={changeQuality}
                      quality={quality}
                      resolutions={qualityResolutions}
                    />
                  )}
                  <CameraInfoPanel stats={debugStats} />
                </div>
              </div>
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

interface CameraQualityPanelProps {
  configuring: boolean
  currentMode: CameraMode | null
  onChange: (value: string) => void
  quality: string
  resolutions: CameraResolution[]
}

function CameraQualityPanel({ configuring, currentMode, onChange, quality, resolutions }: CameraQualityPanelProps) {
  const { t } = useTranslation()
  const options = [
    {
      value: 'auto',
      label: t('camera.qualityAuto'),
      detail: null,
    },
    ...resolutions.map((resolution) => {
      const mode = manualModeForResolution(resolution)
      return {
        value: cameraResolutionKey(resolution),
        label: `${resolution.width} × ${resolution.height}`,
        detail: mode ? `${mode.fps} FPS` : null,
      }
    }),
  ]

  return (
    <section className="rounded-2xl border bg-[hsl(var(--panel))] p-5 shadow-sm">
      <div className="flex items-center gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
          <Gauge className="h-4 w-4" />
        </div>
        <div className="min-w-0">
          <h2 className="text-[14px] font-semibold text-[hsl(var(--text))]">{t('camera.qualityLabel')}</h2>
          {currentMode && (
            <p className="mt-0.5 truncate text-[11px] text-[hsl(var(--muted))]">
              {currentMode.width} × {currentMode.height} @ {currentMode.fps} FPS
            </p>
          )}
        </div>
      </div>

      <div aria-label={t('camera.qualityLabel')} className="mt-4 grid grid-cols-2 gap-2" role="radiogroup">
        {options.map((option) => {
          const selected = quality === option.value
          return (
            <button
              aria-checked={selected}
              className={cn(
                'flex min-h-[64px] items-center justify-between gap-3 rounded-xl border px-3.5 py-2.5 text-left transition-all',
                selected
                  ? 'border-[hsl(var(--accent)/0.45)] bg-[hsl(var(--accent)/0.1)] shadow-sm'
                  : 'border-transparent bg-[hsl(var(--panel-2)/0.55)] hover:border-[hsl(var(--text)/0.14)] hover:bg-[hsl(var(--panel-2))]',
              )}
              key={option.value}
              onClick={() => {
                if (!selected) onChange(option.value)
              }}
              role="radio"
              type="button"
            >
              <span className="min-w-0">
                <span className={cn('block truncate text-[13px]', selected ? 'font-semibold text-[hsl(var(--text))]' : 'font-medium text-[hsl(var(--text-secondary))]')}>
                  {option.label}
                </span>
                {option.detail && (
                  <span className="mt-1 block truncate text-[10px] text-[hsl(var(--muted))]">{option.detail}</span>
                )}
              </span>
              {configuring && selected ? (
                <LoaderCircle className="h-4 w-4 shrink-0 animate-spin text-[hsl(var(--accent))]" />
              ) : (
                <span
                  className={cn(
                    'flex h-4 w-4 shrink-0 items-center justify-center rounded-full border',
                    selected ? 'border-[hsl(var(--accent))]' : 'border-[hsl(var(--muted)/0.65)]',
                  )}
                >
                  {selected && <span className="h-2 w-2 rounded-full bg-[hsl(var(--accent))]" />}
                </span>
              )}
            </button>
          )
        })}
      </div>
    </section>
  )
}

function CameraInfoPanel({ stats }: { stats: CameraDebugSnapshot | null }) {
  const { t } = useTranslation()
  const value = stats
  return (
    <section className="rounded-2xl border bg-[hsl(var(--panel))] p-5 shadow-sm">
      <div className="flex items-center gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--accent))]">
          <Video className="h-4 w-4" />
        </div>
        <h2 className="text-[14px] font-semibold text-[hsl(var(--text))]">{t('camera.infoTitle')}</h2>
      </div>
      <dl className="mt-4 divide-y rounded-xl bg-[hsl(var(--panel-2)/0.42)] px-4">
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
      </dl>
    </section>
  )
}

function CameraDebugRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid min-w-0 gap-1 py-3 sm:grid-cols-[120px_minmax(0,1fr)] sm:gap-4">
      <dt className="text-[11px] font-medium text-[hsl(var(--muted))]">{label}</dt>
      <dd className="min-w-0 break-words font-mono text-[11px] leading-relaxed text-[hsl(var(--text-secondary))]">{value}</dd>
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
