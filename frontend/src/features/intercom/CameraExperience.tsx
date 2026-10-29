import { useCallback, useEffect, useState } from 'react'
import { ArchivePlayer } from './ArchivePlayer'
import { LivePlayer } from './LivePlayer'
import { primeLiveAudio } from './livePriming'
import { ExpandIcon, RefreshIcon } from './MediaIcons'
import type { Camera, IntercomApi } from './intercomApi'

export type CameraExperienceMode = 'live' | 'archive'

export interface CameraExperienceProps {
  api: IntercomApi
  camera: Camera | null
  flatId: string
  presentation: 'preview' | 'detail'
  initialArchiveAt?: string
  audioSuspended?: boolean
  onAuthRequired?: () => void
  onOpenDetail?: () => void
}

interface SnapshotFrame {
  url: string
  takenAt: number
}

const snapshotRefreshMs = 15_000

function snapshotTime(value: number): string {
  return new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  }).format(value)
}

// Stale-while-revalidate: the previous frame stays on screen while the next one loads.
function CameraSnapshot({ api, camera, flatId, onOpen }: { api: IntercomApi; camera: Camera; flatId: string; onOpen?: () => void }) {
  const [request, setRequest] = useState(0)
  const [loading, setLoading] = useState(true)
  const [failed, setFailed] = useState(false)
  const [frame, setFrame] = useState<SnapshotFrame | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    api.getSnapshot(flatId, camera.id, controller.signal).then((blob) => {
      if (controller.signal.aborted) return
      setFrame({ url: URL.createObjectURL(blob), takenAt: Date.now() })
      setFailed(false)
      setLoading(false)
    }).catch(() => {
      if (controller.signal.aborted) return
      setFailed(true)
      setLoading(false)
    })
    return () => controller.abort()
  }, [api, camera.id, flatId, request])

  useEffect(() => {
    if (!frame) return
    const { url } = frame
    return () => URL.revokeObjectURL(url)
  }, [frame])

  const refresh = useCallback(() => {
    setLoading(true)
    setRequest((value) => value + 1)
  }, [])

  useEffect(() => {
    if (loading) return
    let due = false
    const refreshWhenVisible = () => {
      if (due && document.visibilityState === 'visible') refresh()
    }
    const timer = window.setTimeout(() => {
      due = true
      refreshWhenVisible()
    }, snapshotRefreshMs)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      window.clearTimeout(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
    }
  }, [loading, refresh])

  const caption = <span className="media-caption">
    <b>{camera.name}</b>
    {frame && <small>Кадр {snapshotTime(frame.takenAt)}</small>}
  </span>

  return <div className="camera-snapshot media-frame">
    {frame && <img alt={`Кадр с камеры ${camera.name}`} src={frame.url} />}
    {onOpen
      ? <button aria-label={`Открыть камеру ${camera.name}`} className="media-open" onClick={onOpen} type="button">{caption}<ExpandIcon /></button>
      : <div className="media-caption-row">{caption}</div>}
    {!frame && loading && <div className="live-status" role="status"><span><span className="loading-ring" />Загружаем кадр…</span></div>}
    {!frame && !loading && failed && <div className="live-status live-status-error" role="alert">
      <p>Не удалось загрузить кадр</p>
      <button className="media-action" onClick={refresh} type="button">Повторить</button>
    </div>}
    {frame && <button aria-busy={loading} aria-label="Обновить кадр" className="media-corner-button snapshot-refresh" onClick={() => {
      if (!loading) refresh()
    }} type="button"><RefreshIcon /></button>}
  </div>
}

export function CameraExperience(props: CameraExperienceProps) {
  const archiveAvailable = props.camera?.capabilities.archive.state === 'available'
    && Boolean(props.api.getArchiveDistribution && props.api.startArchive && props.api.stopLive)
  const [mode, setMode] = useState<CameraExperienceMode>(props.initialArchiveAt && archiveAvailable ? 'archive' : 'live')

  if (!props.camera) return <div className="camera-empty">Камер по этому адресу нет</div>
  const camera = props.camera
  const openDetail = props.presentation === 'preview' ? props.onOpenDetail : undefined

  const liveView = camera.capabilities.live.state === 'available' && props.api.startLive
    ? <LivePlayer
        api={props.api}
        audioSuspended={props.audioSuspended}
        camera={camera}
        flatId={props.flatId}
        onAuthRequired={props.onAuthRequired}
        onOpen={openDetail}
        variant={props.presentation}
      />
    : camera.capabilities.snapshot.state === 'available'
      ? <CameraSnapshot api={props.api} camera={camera} flatId={props.flatId} onOpen={openDetail} />
      : openDetail
        ? <button aria-label={`Открыть камеру ${camera.name}`} className="camera-placeholder" onClick={openDetail} type="button"><span>Камера сейчас недоступна</span></button>
        : <div className="camera-placeholder"><p>Камера сейчас недоступна</p></div>

  if (props.presentation === 'preview') return <article className="camera-card camera-preview">{liveView}</article>

  return <article className="camera-card camera-detail">
    {archiveAvailable && <div aria-label="Режим камеры" className="camera-mode-tabs" role="group">
      <button aria-pressed={mode === 'live'} className={mode === 'live' ? 'selected' : ''} onClick={() => {
        if (mode === 'live') return
        primeLiveAudio()
        setMode('live')
      }} type="button">Эфир</button>
      <button aria-pressed={mode === 'archive'} className={mode === 'archive' ? 'selected' : ''} onClick={() => setMode('archive')} type="button">Архив</button>
    </div>}
    {mode === 'archive' && archiveAvailable
      ? <ArchivePlayer api={props.api} cameraId={camera.id} cameraName={camera.name} flatId={props.flatId} initialAt={props.initialArchiveAt} />
      : liveView}
  </article>
}
