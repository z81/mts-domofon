import { useEffect, useRef, useState } from 'react'
import Hls from 'hls.js'
import { ArchivePlayer } from './ArchivePlayer'
import type { Camera, IntercomApi } from './intercomApi'

export type CameraExperienceMode = 'idle' | 'snapshot' | 'live' | 'archive'

export interface CameraExperienceProps {
  api: IntercomApi
  camera: Camera | null
  flatId: string
  presentation: 'preview' | 'detail'
  initialArchiveAt?: string
  onOpenDetail?: () => void
}

type SnapshotRemote =
  | { status: 'loading'; requestKey: string }
  | { status: 'success'; requestKey: string; objectUrl: string }
  | { status: 'error'; requestKey: string }

function cameraCapabilityStatus(api: IntercomApi, camera: Camera): string {
  const modes = [
    camera.capabilities.snapshot.state === 'available' ? 'кадр' : null,
    camera.capabilities.live.state === 'available' && api.startLive && api.stopLive ? 'эфир' : null,
    camera.capabilities.archive.state === 'available' && api.getArchiveDistribution && api.startArchive && api.stopLive ? 'архив' : null,
  ].filter((mode): mode is string => Boolean(mode))
  return modes.length > 0 ? `Доступно: ${modes.join(' · ')}` : 'Медиа недоступно'
}

function CameraSnapshot({ api, camera, flatId }: { api: IntercomApi; camera: Camera; flatId: string }) {
  const [reload, setReload] = useState(0)
  const requestKey = `${flatId}\u0000${camera.id}\u0000${reload}`
  const [snapshot, setSnapshot] = useState<SnapshotRemote>({ status: 'loading', requestKey })

  useEffect(() => {
    if (camera.capabilities.snapshot.state !== 'available') return

    const controller = new AbortController()
    let active = true
    let objectUrl: string | null = null

    api.getSnapshot(flatId, camera.id, controller.signal).then((blob) => {
      if (!active) return
      objectUrl = URL.createObjectURL(blob)
      setSnapshot({ status: 'success', requestKey, objectUrl })
    }).catch(() => {
      if (active && !controller.signal.aborted) setSnapshot({ status: 'error', requestKey })
    })

    return () => {
      active = false
      controller.abort()
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [api, camera.capabilities.snapshot.state, camera.id, flatId, reload, requestKey])

  const currentSnapshot: SnapshotRemote = snapshot.requestKey === requestKey
    ? snapshot
    : { status: 'loading', requestKey }

  if (camera.capabilities.snapshot.state !== 'available') {
    return <div className="camera-placeholder"><p>Снимки с этой камеры недоступны</p><small>{cameraCapabilityStatus(api, camera)}</small></div>
  }
  if (currentSnapshot.status === 'loading') {
    return <div className="camera-placeholder" role="status"><span className="loading-ring" /><p>Загружаем кадр…</p></div>
  }
  if (currentSnapshot.status === 'error') {
    return <div className="camera-placeholder" role="alert">
      <p>Не удалось загрузить кадр</p>
      <button className="secondary-button" onClick={() => setReload((value) => value + 1)} type="button">Повторить</button>
    </div>
  }

  return <div className="camera-snapshot media-frame">
    <img alt={`Кадр с камеры ${camera.name}`} src={currentSnapshot.objectUrl} />
    <span className="snapshot-badge">СНИМОК</span>
    <div className="camera-caption">
      <span><b>{camera.name}</b><small>{cameraCapabilityStatus(api, camera)}</small></span>
      <button onClick={() => setReload((value) => value + 1)} type="button">Обновить</button>
    </div>
  </div>
}

function CameraLive({ api, camera, flatId, onStop }: { api: IntercomApi; camera: Camera; flatId: string; onStop: () => void }) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const [reload, setReload] = useState(0)
  const [status, setStatus] = useState<'starting' | 'ready' | 'paused' | 'error'>(
    api.startLive && api.stopLive ? 'starting' : 'error',
  )

  useEffect(() => {
    const startLive = api.startLive
    const stopLive = api.stopLive
    if (!startLive || !stopLive) return
    const video = videoRef.current
    if (!video) return
    const controller = new AbortController()
    let active = true
    let sessionId: string | null = null
    let hls: Hls | null = null
    let playPending = false

    const stopSession = () => {
      const previousSessionId = sessionId
      sessionId = null
      if (previousSessionId) void stopLive(previousSessionId).catch(() => undefined)
    }
    const failPlayback = () => {
      if (!active) return
      active = false
      controller.abort()
      hls?.destroy()
      hls = null
      stopSession()
      setStatus('error')
    }
    const play = async () => {
      if (!active || playPending || !sessionId) return
      playPending = true
      try {
        await video.play()
      } catch (error) {
        if (!active) return
        if (error instanceof DOMException && error.name === 'NotAllowedError') {
          // Startup can outlive the user gesture that allows playback with sound.
          video.muted = true
          try {
            await video.play()
          } catch {
            if (active) setStatus('paused')
          }
        } else if (!(error instanceof DOMException && error.name === 'AbortError')) {
          failPlayback()
        }
      } finally {
        playPending = false
      }
    }
    const ready = () => {
      if (active && sessionId) setStatus('ready')
    }
    const canPlay = () => { void play() }
    const mediaError = () => { failPlayback() }
    video.addEventListener('playing', ready)
    video.addEventListener('canplay', canPlay)
    video.addEventListener('error', mediaError)

    startLive(flatId, camera.id, controller.signal).then((session) => {
      if (!active) {
        void stopLive(session.sessionId).catch(() => undefined)
        return
      }
      sessionId = session.sessionId
      if (video.canPlayType('application/vnd.apple.mpegurl')) {
        video.src = session.playlistUrl
      } else if (Hls.isSupported()) {
        hls = new Hls({ enableWorker: true, lowLatencyMode: true })
        hls.on(Hls.Events.ERROR, (_event, data) => {
          if (data.fatal) mediaError()
        })
        hls.on(Hls.Events.MANIFEST_PARSED, canPlay)
        hls.attachMedia(video)
        hls.loadSource(session.playlistUrl)
      } else {
        failPlayback()
        return
      }
      void play()
    }).catch(() => {
      if (active && !controller.signal.aborted) failPlayback()
    })

    return () => {
      active = false
      controller.abort()
      video.removeEventListener('playing', ready)
      video.removeEventListener('canplay', canPlay)
      video.removeEventListener('error', mediaError)
      hls?.destroy()
      if (video.getAttribute('src')) {
        video.pause()
        video.removeAttribute('src')
        video.load()
      }
      stopSession()
    }
  }, [api, camera.id, flatId, reload])

  return <div className="camera-live media-frame">
    <video aria-label={`Live-видео с камеры ${camera.name}`} controls playsInline ref={videoRef} />
    <span className="live-badge"><i />LIVE</span>
    {status === 'starting' && <div className="live-status" role="status"><span className="loading-ring" />Подключаем видео…</div>}
    {status === 'paused' && <div className="live-status" role="status">
      <button className="secondary-button" onClick={() => { void videoRef.current?.play().catch(() => undefined) }} type="button">Воспроизвести</button>
    </div>}
    {status === 'error' && <div className="live-status" role="alert">
      <span>Не удалось запустить видео</span>
      <button className="secondary-button" onClick={() => { setStatus('starting'); setReload((value) => value + 1) }} type="button">Повторить</button>
    </div>}
    <button className="live-stop-button" onClick={onStop} type="button">Остановить</button>
  </div>
}

export function CameraExperience(props: CameraExperienceProps) {
  const snapshotAvailable = props.camera?.capabilities.snapshot.state === 'available'
  const liveAvailable = props.camera?.capabilities.live.state === 'available' && Boolean(props.api.startLive && props.api.stopLive)
  const archiveAvailable = props.camera?.capabilities.archive.state === 'available' && Boolean(props.api.getArchiveDistribution && props.api.startArchive && props.api.stopLive)
  const initialMode: CameraExperienceMode = props.initialArchiveAt && archiveAvailable
    ? 'archive'
    : snapshotAvailable ? 'snapshot' : 'idle'
  const [mode, setMode] = useState<CameraExperienceMode>(initialMode)

  if (!props.camera) return <div className="camera-empty">Камер для этого объекта нет</div>
  if (props.presentation === 'preview') return <article className="camera-card camera-preview">
    <CameraSnapshot api={props.api} camera={props.camera} flatId={props.flatId} />
    <button aria-label={`Смотреть ${props.camera.name}`} className="camera-action" onClick={props.onOpenDetail} type="button">Смотреть</button>
  </article>

  return <article className="camera-card camera-detail">
    <div aria-label="Режим камеры" className="camera-mode-tabs" role="group">
      {liveAvailable && <button aria-pressed={mode === 'live'} className={mode === 'live' ? 'selected' : ''} onClick={() => setMode('live')} type="button">Эфир</button>}
      {snapshotAvailable && <button aria-pressed={mode === 'snapshot'} className={mode === 'snapshot' ? 'selected' : ''} onClick={() => setMode('snapshot')} type="button">Кадр</button>}
      {archiveAvailable && <button aria-pressed={mode === 'archive'} className={mode === 'archive' ? 'selected' : ''} onClick={() => setMode('archive')} type="button">Архив</button>}
    </div>
    {mode === 'live' && liveAvailable
      ? <CameraLive api={props.api} camera={props.camera} flatId={props.flatId} key={`${props.flatId}\u0000${props.camera.id}`} onStop={() => setMode(snapshotAvailable ? 'snapshot' : 'idle')} />
      : mode === 'archive' && archiveAvailable
        ? <ArchivePlayer api={props.api} cameraId={props.camera.id} cameraName={props.camera.name} flatId={props.flatId} initialAt={props.initialArchiveAt} />
        : mode === 'snapshot' && snapshotAvailable
          ? <CameraSnapshot api={props.api} camera={props.camera} flatId={props.flatId} />
          : <div className="camera-placeholder"><p>Выберите режим камеры</p></div>}
  </article>
}
