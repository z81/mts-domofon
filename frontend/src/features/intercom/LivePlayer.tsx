import { useEffect, useRef, useState } from 'react'
import {
  attachStream,
  isAutoplayBlocked,
  liveHlsConfig,
  pickPlaybackEngine,
  playVideo,
  retryDelay,
  watchPlayback,
  type AttachedStream,
  type PlaybackEngine,
  type PlaybackWatch,
  type StreamFailure,
} from './hlsPlayback'
import { IntercomApiError, type Camera, type IntercomApi } from './intercomApi'
import { acquireLiveSession, type LiveSessionLease } from './liveSessions'
import { captureLiveFrame, createLiveVideo, freshLiveFrame, liveFrameKey, primeLiveAudio, takePrimedVideo } from './livePriming'
import { ExpandIcon, FullscreenIcon, PlayIcon, SoundIcon } from './MediaIcons'

type LivePhase = 'connecting' | 'playing' | 'buffering' | 'reconnecting' | 'error' | 'suspended'
type LiveIssue = 'busy' | 'auth' | 'failed' | 'unsupported'
type LiveFailure = StreamFailure | 'stalled'

interface LiveState {
  phase: LivePhase
  issue: LiveIssue | null
  played: boolean
  muted: boolean
  playBlocked: boolean
  hasAudio: boolean
  frame: string | null
  announcement: string
}

interface LiveConnection {
  retry(): void
  dispose(): void
}

export interface LivePlayerProps {
  api: IntercomApi
  flatId: string
  camera: Camera
  variant: 'preview' | 'detail'
  audioSuspended?: boolean
  onAuthRequired?: () => void
  onOpen?: () => void
}

const bufferingDelayMs = 600
const stablePlaybackMs = 30_000

const issueCopy: Record<LiveIssue, string> = {
  busy: 'Камера сейчас занята, пробуем снова…',
  auth: 'Нужно снова войти в MTS ID',
  failed: 'Не удалось подключиться к камере',
  unsupported: 'Браузер не может показать эфир',
}

function patchLive(current: LiveState, patch: Partial<LiveState>): LiveState {
  const keys = Object.keys(patch) as (keyof LiveState)[]
  return keys.some((key) => patch[key] !== current[key]) ? { ...current, ...patch } : current
}

function isClientError(error: unknown): boolean {
  return error instanceof IntercomApiError && error.status >= 400 && error.status < 500 && error.status !== 408 && error.status !== 429
}

function connectLive({ api, flatId, cameraId, video, report }: {
  api: IntercomApi
  flatId: string
  cameraId: string
  video: HTMLVideoElement
  report(patch: Partial<LiveState>): void
}): LiveConnection {
  const frameKey = liveFrameKey(flatId, cameraId)
  let phase: LivePhase = 'connecting'
  let issue: LiveIssue | null = null
  let disposed = false
  let generation = 0
  let lease: LiveSessionLease | null = null
  let sessionId: string | null = null
  let stream: AttachedStream | null = null
  let watch: PlaybackWatch | null = null
  let played = false
  let attempt = 0
  let goneBefore = false
  let retryTimer: number | undefined
  let bufferTimer: number | undefined
  let stableTimer: number | undefined

  function setPhase(next: LivePhase, nextIssue: LiveIssue | null, announcement?: string) {
    phase = next
    issue = nextIssue
    report(announcement ? { phase: next, issue: nextIssue, announcement } : { phase: next, issue: nextIssue })
  }

  // The element stays; only the stream goes. The last frame becomes the poster for the reconnect and the next mount.
  function teardown(invalidate: boolean) {
    generation += 1
    window.clearTimeout(retryTimer)
    window.clearTimeout(bufferTimer)
    window.clearTimeout(stableTimer)
    retryTimer = undefined
    const frame = captureLiveFrame(video, frameKey)
    if (frame) report({ frame })
    watch?.dispose()
    watch = null
    stream?.destroy()
    stream = null
    if (lease && invalidate && sessionId) lease.invalidate(sessionId)
    lease?.release()
    lease = null
    sessionId = null
  }

  function connect() {
    teardown(false)
    if (disposed) return
    if (document.visibilityState === 'hidden') {
      setPhase('suspended', null)
      return
    }
    const engine = pickPlaybackEngine(video)
    if (!engine) {
      setPhase('error', 'unsupported', issueCopy.unsupported)
      return
    }
    const current = generation
    const currentLease = acquireLiveSession(api, flatId, cameraId)
    lease = currentLease
    currentLease.session.then((session) => {
      if (current !== generation) return
      sessionId = session.sessionId
      attach(engine, session.playlistUrl, current)
    }, (error: unknown) => {
      if (current !== generation) return
      currentLease.release()
      lease = null
      if (error instanceof IntercomApiError && error.status === 401) {
        setPhase('error', 'auth', issueCopy.auth)
      } else if (isClientError(error)) {
        setPhase('error', 'failed', issueCopy.failed)
      } else {
        retryLater(error instanceof IntercomApiError && error.status === 503 ? 'busy' : null)
      }
    })
  }

  function attach(engine: PlaybackEngine, url: string, current: number) {
    stream = attachStream(video, engine, url, liveHlsConfig, {
      onFailure: (failure) => {
        if (current === generation) fail(failure)
      },
      onAudio: (hasAudio) => {
        if (current === generation) report({ hasAudio })
      },
    })
    watch = watchPlayback(video, {
      onFirstFrame: () => {
        if (current === generation) showFirstFrame()
      },
      onStall: () => {
        if (current === generation) fail('stalled')
      },
    })
    void startPlayback(current)
  }

  // Sound is tried first when the element is unmuted; a refusal falls back to muted playback and says so.
  async function startPlayback(current: number) {
    try {
      await playVideo(video)
      return
    } catch (error) {
      if (current !== generation || !isAutoplayBlocked(error)) return
    }
    if (!video.muted) {
      video.muted = true
      report({ announcement: 'Звук выключен' })
      try {
        await playVideo(video)
        return
      } catch (error) {
        if (current !== generation || !isAutoplayBlocked(error)) return
      }
    }
    report({ playBlocked: true })
  }

  function showFirstFrame() {
    window.clearTimeout(bufferTimer)
    played = true
    report({ played: true, playBlocked: false })
    setPhase('playing', null, 'Эфир подключён')
    stableTimer = window.setTimeout(() => {
      attempt = 0
      goneBefore = false
    }, stablePlaybackMs)
  }

  // A dead session (404 or a frozen picture) reconnects at once; only a repeat waits for the backoff.
  function fail(failure: LiveFailure) {
    if (disposed) return
    const gone = failure === 'gone' || failure === 'stalled'
    teardown(failure !== 'media')
    if (gone && !goneBefore) {
      goneBefore = true
      setPhase('reconnecting', null, 'Переподключаем эфир')
      connect()
      return
    }
    retryLater(null)
  }

  function retryLater(nextIssue: LiveIssue | null) {
    attempt += 1
    const delay = retryDelay(attempt)
    if (delay === null) {
      setPhase('error', 'failed', issueCopy.failed)
      return
    }
    setPhase('reconnecting', nextIssue, 'Переподключаем эфир')
    retryTimer = window.setTimeout(connect, delay)
  }

  function restart() {
    attempt = 0
    goneBefore = false
    setPhase(played ? 'reconnecting' : 'connecting', null, played ? 'Переподключаем эфир' : undefined)
    connect()
  }

  function onWaiting() {
    if (phase !== 'playing') return
    window.clearTimeout(bufferTimer)
    bufferTimer = window.setTimeout(() => {
      if (phase === 'playing') setPhase('buffering', null)
    }, bufferingDelayMs)
  }

  function onProgress() {
    window.clearTimeout(bufferTimer)
    if (phase === 'buffering') setPhase('playing', null)
  }

  function onPlaying() {
    onProgress()
    report({ playBlocked: false })
  }

  function onVolumeChange() {
    report({ muted: video.muted })
  }

  function onVisibilityChange() {
    if (document.visibilityState === 'hidden') {
      if (phase === 'error' || phase === 'suspended') return
      teardown(false)
      setPhase('suspended', null)
    } else if (phase === 'suspended') {
      setPhase(played ? 'reconnecting' : 'connecting', null)
      connect()
    } else if (phase === 'error' && issue === 'failed') {
      restart()
    }
  }

  function onOnline() {
    if (phase === 'reconnecting' && retryTimer !== undefined) connect()
    else if (phase === 'error' && issue === 'failed') restart()
  }

  video.addEventListener('waiting', onWaiting)
  video.addEventListener('playing', onPlaying)
  video.addEventListener('timeupdate', onProgress)
  video.addEventListener('volumechange', onVolumeChange)
  document.addEventListener('visibilitychange', onVisibilityChange)
  window.addEventListener('online', onOnline)
  // A player of the same camera that unmounted in this commit has just left its last frame.
  const recentFrame = freshLiveFrame(frameKey)
  if (recentFrame) report({ frame: recentFrame })
  connect()

  return {
    retry: restart,
    dispose() {
      if (disposed) return
      teardown(false)
      disposed = true
      video.removeEventListener('waiting', onWaiting)
      video.removeEventListener('playing', onPlaying)
      video.removeEventListener('timeupdate', onProgress)
      video.removeEventListener('volumechange', onVolumeChange)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('online', onOnline)
    },
  }
}

function useSnapshotPoster(api: IntercomApi, flatId: string, cameraId: string, enabled: boolean): string | null {
  const [poster, setPoster] = useState<string | null>(null)

  useEffect(() => {
    if (!enabled || freshLiveFrame(liveFrameKey(flatId, cameraId))) return
    const controller = new AbortController()
    let objectUrl: string | null = null
    api.getSnapshot(flatId, cameraId, controller.signal).then((blob) => {
      if (controller.signal.aborted) return
      objectUrl = URL.createObjectURL(blob)
      setPoster(objectUrl)
    }).catch(() => undefined)
    return () => {
      controller.abort()
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [api, cameraId, enabled, flatId])

  return enabled ? poster : null
}

function fullscreenSupported(): boolean {
  return document.fullscreenEnabled || 'webkitEnterFullscreen' in HTMLVideoElement.prototype
}

export function LivePlayer({ api, flatId, camera, variant, audioSuspended = false, onAuthRequired, onOpen }: LivePlayerProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const hostRef = useRef<HTMLDivElement>(null)
  const videoRef = useRef<HTMLVideoElement | null>(null)
  const connectionRef = useRef<LiveConnection | null>(null)
  const audioSuspendedRef = useRef(audioSuspended)
  const [live, setLive] = useState<LiveState>(() => ({
    phase: 'connecting',
    issue: null,
    played: false,
    muted: variant === 'preview' || audioSuspended,
    playBlocked: false,
    hasAudio: true,
    frame: freshLiveFrame(liveFrameKey(flatId, camera.id)),
    announcement: '',
  }))
  const [canFullscreen] = useState(() => variant === 'detail' && fullscreenSupported())
  const [fullscreen, setFullscreen] = useState(false)
  const snapshotPoster = useSnapshotPoster(
    api,
    flatId,
    camera.id,
    camera.capabilities.snapshot.state === 'available' && live.frame === null && !live.played,
  )

  useEffect(() => {
    audioSuspendedRef.current = audioSuspended
    if (audioSuspended && videoRef.current) videoRef.current.muted = true
  }, [audioSuspended])

  // One element for the whole life of the player: reconnects reuse it, so a primed or unmuted element keeps its rights.
  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    let video = videoRef.current
    if (!video) {
      video = (variant === 'detail' ? takePrimedVideo() : null) ?? createLiveVideo()
      video.muted = variant === 'preview' || audioSuspendedRef.current
      videoRef.current = video
    }
    host.appendChild(video)
    const connection = connectLive({
      api,
      flatId,
      cameraId: camera.id,
      video,
      report: (patch) => setLive((current) => patchLive(current, patch)),
    })
    connectionRef.current = connection
    return () => {
      connection.dispose()
      if (connectionRef.current === connection) connectionRef.current = null
    }
  }, [api, camera.id, flatId, variant])

  useEffect(() => {
    videoRef.current?.setAttribute('aria-label', `Эфир с камеры ${camera.name}`)
  }, [camera.name])

  useEffect(() => {
    if (!canFullscreen) return
    const onFullscreenChange = () => setFullscreen(document.fullscreenElement !== null && document.fullscreenElement === containerRef.current)
    document.addEventListener('fullscreenchange', onFullscreenChange)
    return () => document.removeEventListener('fullscreenchange', onFullscreenChange)
  }, [canFullscreen])

  function openCamera() {
    if (videoRef.current) captureLiveFrame(videoRef.current, liveFrameKey(flatId, camera.id))
    primeLiveAudio()
    onOpen?.()
  }

  function toggleSound() {
    const video = videoRef.current
    if (!video || audioSuspended) return
    video.muted = !video.muted
    if (video.paused) void playVideo(video).catch(() => undefined)
  }

  function resumePlayback() {
    const video = videoRef.current
    if (!video) return
    if (variant === 'detail' && !audioSuspended) video.muted = false
    void playVideo(video).catch(() => undefined)
  }

  function toggleFullscreen() {
    const container = containerRef.current
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => undefined)
    } else if (container && document.fullscreenEnabled) {
      void container.requestFullscreen().catch(() => undefined)
    } else {
      (videoRef.current as (HTMLVideoElement & { webkitEnterFullscreen?: () => void }) | null)?.webkitEnterFullscreen?.()
    }
  }

  const poster = live.frame ?? snapshotPoster
  const pending = (live.phase === 'connecting' || live.phase === 'reconnecting') && !live.playBlocked
  const showSound = live.hasAudio && live.phase !== 'error'
  const soundLabel = live.muted ? 'Включить звук' : 'Выключить звук'

  return <div className={`live-player live-player-${variant}`} ref={containerRef}>
    {poster && <img alt="" className="live-poster" src={poster} />}
    <div className="live-video" ref={hostRef} />
    {variant === 'preview' && <button aria-label={`Открыть камеру ${camera.name}`} className="media-open" onClick={openCamera} type="button">
      <span className="media-caption"><b>{camera.name}</b><span className="live-badge"><i />Эфир</span></span>
      <ExpandIcon />
    </button>}
    {pending && <div className="live-status live-status-pending"><span><span className="loading-ring" />{live.issue === 'busy' ? issueCopy.busy : live.phase === 'connecting' ? 'Подключаем эфир…' : 'Переподключаем эфир…'}</span></div>}
    {live.phase === 'buffering' && <div className="live-buffering"><span className="loading-ring" /></div>}
    {live.phase === 'error' && <div className="live-status live-status-error">
      <p>{issueCopy[live.issue ?? 'failed']}</p>
      {live.issue === 'auth' && onAuthRequired && <button className="media-action" onClick={onAuthRequired} type="button">Войти</button>}
      {live.issue === 'failed' && <button className="media-action" onClick={() => connectionRef.current?.retry()} type="button">Повторить</button>}
    </div>}
    {live.playBlocked && live.phase !== 'error' && <button className="live-play" onClick={resumePlayback} type="button"><PlayIcon />Смотреть</button>}
    {variant === 'preview' && showSound && <button aria-label={soundLabel} className="media-corner-button" disabled={audioSuspended} onClick={toggleSound} type="button"><SoundIcon muted={live.muted} /></button>}
    {variant === 'detail' && <div className="live-controls">
      <span className="live-badge"><i />Эфир</span>
      {showSound && <button aria-label={soundLabel} className={live.muted ? 'live-sound live-sound-off' : 'live-sound'} disabled={audioSuspended} onClick={toggleSound} type="button">
        <SoundIcon muted={live.muted} />{live.muted && <span>Включить звук</span>}
      </button>}
      {canFullscreen && <button aria-label={fullscreen ? 'Свернуть' : 'На весь экран'} className="live-fullscreen" onClick={toggleFullscreen} type="button"><FullscreenIcon active={fullscreen} /></button>}
    </div>}
    <p aria-live="polite" className="visually-hidden">{live.announcement}</p>
  </div>
}
