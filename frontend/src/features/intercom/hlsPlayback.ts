import Hls, { type ErrorData, type HlsConfig } from 'hls.js'

export type PlaybackEngine = 'hls' | 'native'
export type StreamFailure = 'gone' | 'network' | 'media'

export interface StreamCallbacks {
  onFailure(failure: StreamFailure): void
  onAudio?(hasAudio: boolean): void
}

export interface AttachedStream {
  destroy(): void
}

export interface PlaybackWatch {
  dispose(): void
}

// One-second segments in a six-segment window: two segments of latency without stalls, and a hidden tab never
// leaves the picture minutes behind the edge.
export const liveHlsConfig: Partial<HlsConfig> = {
  enableWorker: true,
  lowLatencyMode: false,
  liveSyncDurationCount: 2,
  liveMaxLatencyDurationCount: 6,
  maxLiveSyncPlaybackRate: 1.1,
  maxBufferLength: 8,
  backBufferLength: 10,
}

export const archiveHlsConfig: Partial<HlsConfig> = {
  enableWorker: true,
  lowLatencyMode: false,
  backBufferLength: 30,
}

const retryDelays = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000]
const firstFrameTimeoutMs = 15_000
const stallTimeoutMs = 6_000
const sessionGoneDetails: string[] = [
  Hls.ErrorDetails.MANIFEST_LOAD_ERROR,
  Hls.ErrorDetails.LEVEL_LOAD_ERROR,
  Hls.ErrorDetails.FRAG_LOAD_ERROR,
]

function hasClassicMediaSource(): boolean {
  return typeof window.MediaSource === 'function'
}

// Chrome answers "maybe" for native HLS, but its native player never reports a dead session, so hls.js wins whenever
// classic MSE exists. Native HLS is only for browsers without MediaSource (iPhone Safari).
export function pickPlaybackEngine(video: HTMLVideoElement): PlaybackEngine | null {
  if (hasClassicMediaSource() && Hls.isSupported()) return 'hls'
  if (video.canPlayType('application/vnd.apple.mpegurl')) return 'native'
  return Hls.isSupported() ? 'hls' : null
}

export function retryDelay(attempt: number): number | null {
  const base = retryDelays[attempt - 1]
  return base === undefined ? null : Math.round(base * (0.8 + Math.random() * 0.4))
}

export function isSessionGone(data: ErrorData): boolean {
  return sessionGoneDetails.includes(data.details) && data.response?.code === 404
}

export function isAutoplayBlocked(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'name' in error && error.name === 'NotAllowedError'
}

export function playVideo(video: HTMLVideoElement): Promise<void> {
  try {
    return Promise.resolve(video.play())
  } catch (error) {
    return Promise.reject(error)
  }
}

export function detachVideo(video: HTMLVideoElement) {
  if (!video.hasAttribute('src')) return
  video.removeAttribute('src')
  video.load()
}

export function attachStream(
  video: HTMLVideoElement,
  engine: PlaybackEngine,
  url: string,
  config: Partial<HlsConfig>,
  callbacks: StreamCallbacks,
): AttachedStream {
  let settled = false
  let destroyed = false
  // Reported on the next task: hls.js must not be destroyed from inside its own event dispatch.
  const fail = (failure: StreamFailure) => {
    if (settled) return
    settled = true
    window.setTimeout(() => {
      if (!destroyed) callbacks.onFailure(failure)
    }, 0)
  }

  if (engine === 'native') {
    const onError = () => fail('network')
    video.addEventListener('error', onError)
    video.src = url
    return {
      destroy() {
        destroyed = true
        video.removeEventListener('error', onError)
        detachVideo(video)
      },
    }
  }

  const hls = new Hls(config)
  let mediaRecoveries = 0
  hls.on(Hls.Events.ERROR, (_event, data) => {
    if (isSessionGone(data)) {
      fail('gone')
      return
    }
    if (!data.fatal || settled) return
    if (data.type === Hls.ErrorTypes.MEDIA_ERROR && mediaRecoveries < 2) {
      if (mediaRecoveries === 1) hls.swapAudioCodec()
      mediaRecoveries += 1
      const resume = !video.paused
      hls.recoverMediaError()
      // Re-attaching reloads the element and a reload pauses it, while the watchdog ignores paused video.
      if (resume) void playVideo(video).catch(() => undefined)
      return
    }
    fail(data.type === Hls.ErrorTypes.MEDIA_ERROR ? 'media' : 'network')
  })
  hls.on(Hls.Events.BUFFER_CODECS, (_event, data) => callbacks.onAudio?.(Boolean(data.audio ?? data.audiovideo)))
  if (!hasClassicMediaSource()) video.disableRemotePlayback = true
  hls.attachMedia(video)
  hls.loadSource(url)
  return {
    destroy() {
      destroyed = true
      hls.destroy()
      detachVideo(video)
    },
  }
}

// The first real frame (a presented frame, or `playing` plus moving time) and stalls of a playing element.
// Time spent paused never counts: a blocked autoplay waits for the viewer, not for the watchdog.
export function watchPlayback(video: HTMLVideoElement, handlers: { onFirstFrame(): void; onStall(): void }): PlaybackWatch {
  let firstFrame = false
  let stalled = false
  let playingAt: number | null = null
  let lastTime = video.currentTime
  let lastProgressAt = Date.now()
  let frameRequest: number | null = null

  function markFirstFrame() {
    if (firstFrame || stalled) return
    firstFrame = true
    lastTime = video.currentTime
    lastProgressAt = Date.now()
    handlers.onFirstFrame()
  }
  function onPlaying() {
    if (playingAt === null) playingAt = video.currentTime
  }
  function onTimeUpdate() {
    if (playingAt !== null && video.currentTime > playingAt) markFirstFrame()
  }

  video.addEventListener('playing', onPlaying)
  video.addEventListener('timeupdate', onTimeUpdate)
  if (typeof video.requestVideoFrameCallback === 'function') frameRequest = video.requestVideoFrameCallback(markFirstFrame)
  const timer = window.setInterval(() => {
    if (stalled) return
    const now = Date.now()
    if (video.paused || video.currentTime !== lastTime) {
      lastTime = video.currentTime
      lastProgressAt = now
      return
    }
    if (now - lastProgressAt < (firstFrame ? stallTimeoutMs : firstFrameTimeoutMs)) return
    stalled = true
    handlers.onStall()
  }, 1_000)

  return {
    dispose() {
      stalled = true
      window.clearInterval(timer)
      video.removeEventListener('playing', onPlaying)
      video.removeEventListener('timeupdate', onTimeUpdate)
      if (frameRequest !== null) video.cancelVideoFrameCallback(frameRequest)
    },
  }
}
