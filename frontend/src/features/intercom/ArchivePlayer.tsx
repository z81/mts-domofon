import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Hls from 'hls.js'
import type { ArchiveDistribution, IntercomApi } from './intercomApi'

interface ArchivePlayerProps {
  api: IntercomApi
  flatId: string
  cameraId: string
  cameraName: string
  initialAt?: string
}

type DistributionRemote =
  | { status: 'loading' }
  | { status: 'error' }
  | { status: 'success'; data: ArchiveDistribution }

function toLocalInput(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
  return local.toISOString().slice(0, 16)
}

function fromLocalInput(value: string): string | null {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : date.toISOString()
}

function intervalLabel(from: string, to: string): string {
  const formatter = new Intl.DateTimeFormat('ru-RU', {
    day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit',
  })
  const fromDate = new Date(from)
  const toDate = new Date(to)
  if (Number.isNaN(fromDate.getTime()) || Number.isNaN(toDate.getTime())) return 'Некорректный интервал'
  return `${formatter.format(fromDate)} — ${formatter.format(toDate)}`
}

export function ArchivePlayer({ api, flatId, cameraId, cameraName, initialAt }: ArchivePlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const sessionRef = useRef<string | null>(null)
  const hlsRef = useRef<Hls | null>(null)
  const startAbortRef = useRef<AbortController | null>(null)
  const [reload, setReload] = useState(0)
  const [distribution, setDistribution] = useState<DistributionRemote>(
    api.getArchiveDistribution ? { status: 'loading' } : { status: 'error' },
  )
  const [selected, setSelected] = useState('')
  const [playback, setPlayback] = useState<'idle' | 'starting' | 'playing' | 'error'>('idle')

  useEffect(() => {
    const getDistribution = api.getArchiveDistribution
    if (!getDistribution) return
    const controller = new AbortController()
    getDistribution(flatId, cameraId, controller.signal).then((data) => {
      if (controller.signal.aborted) return
      setDistribution({ status: 'success', data })
      const initial = initialAt && data.intervals.some((interval) => {
        const target = new Date(initialAt).getTime()
        return target >= new Date(interval.from).getTime() && target <= new Date(interval.to).getTime()
      }) ? initialAt : data.intervals.at(-1)?.from
      setSelected(initial ? toLocalInput(initial) : '')
    }).catch(() => {
      if (!controller.signal.aborted) setDistribution({ status: 'error' })
    })
    return () => controller.abort()
  }, [api, cameraId, flatId, initialAt, reload])

  const cleanupPlayback = useCallback((setIdle: boolean) => {
    startAbortRef.current?.abort()
    startAbortRef.current = null
    hlsRef.current?.destroy()
    hlsRef.current = null
    const video = videoRef.current
    const hadPlayback = Boolean(sessionRef.current || video?.getAttribute('src'))
    if (video && hadPlayback) {
      video.pause()
      video.removeAttribute('src')
      video.load()
    }
    const sessionId = sessionRef.current
    sessionRef.current = null
    if (sessionId && api.stopLive) void api.stopLive(sessionId).catch(() => undefined)
    if (setIdle) setPlayback('idle')
  }, [api])

  useEffect(() => () => cleanupPlayback(false), [cleanupPlayback])

  const selectedIso = useMemo(() => fromLocalInput(selected), [selected])
  const selectedIsRecorded = distribution.status === 'success' && selectedIso !== null
    && distribution.data.intervals.some((interval) => {
      const target = new Date(selectedIso).getTime()
      return target >= new Date(interval.from).getTime() && target <= new Date(interval.to).getTime()
    })

  async function startPlayback() {
    const startArchive = api.startArchive
    if (!startArchive || !api.stopLive || !selectedIso || !selectedIsRecorded || playback === 'starting') return
    cleanupPlayback(true)
    setPlayback('starting')
    const controller = new AbortController()
    startAbortRef.current = controller
    try {
      const session = await startArchive(flatId, cameraId, selectedIso, controller.signal)
      if (controller.signal.aborted) {
        await api.stopLive(session.sessionId).catch(() => undefined)
        return
      }
      sessionRef.current = session.sessionId
      const video = videoRef.current
      if (!video) {
        cleanupPlayback(false)
        return
      }
      if (video.canPlayType('application/vnd.apple.mpegurl')) {
        video.src = session.playlistUrl
      } else if (Hls.isSupported()) {
        const hls = new Hls({ enableWorker: true, lowLatencyMode: false })
        hlsRef.current = hls
        hls.loadSource(session.playlistUrl)
        hls.attachMedia(video)
      } else {
        cleanupPlayback(false)
        setPlayback('error')
        return
      }
      setPlayback('playing')
      await video.play().catch(() => undefined)
    } catch {
      if (!controller.signal.aborted) setPlayback('error')
    } finally {
      if (startAbortRef.current === controller) startAbortRef.current = null
    }
  }

  return <div className="archive-player">
    <video aria-label={`Архив камеры ${cameraName}`} controls playsInline ref={videoRef} />
    {playback === 'playing' && <span className="archive-badge">АРХИВ</span>}
    <div className="archive-controls">
      <div className="section-title-row"><div><p className="eyebrow">Записи камеры</p><h2>Архив</h2></div></div>
      {distribution.status === 'loading' && <div className="journal-state" role="status"><span className="loading-ring" />Ищем записи…</div>}
      {distribution.status === 'error' && <div className="journal-state" role="alert"><p>Не удалось загрузить интервалы архива</p><button className="secondary-button" onClick={() => { setDistribution({ status: 'loading' }); setReload((value) => value + 1) }} type="button">Повторить</button></div>}
      {distribution.status === 'success' && distribution.data.intervals.length === 0 && <p className="empty-copy">Записей за доступный период нет</p>}
      {distribution.status === 'success' && distribution.data.intervals.length > 0 && <>
        <div className="archive-intervals" aria-label="Доступные интервалы">
          {distribution.data.intervals.map((interval) => <button
            onClick={() => setSelected(toLocalInput(interval.from))}
            type="button"
            key={`${interval.from}-${interval.to}`}
          >{intervalLabel(interval.from, interval.to)}</button>)}
        </div>
        <label className="archive-time-field">Начало просмотра
          <input max={toLocalInput(distribution.data.intervals.at(-1)!.to)} min={toLocalInput(distribution.data.intervals[0].from)} onChange={(event) => setSelected(event.target.value)} type="datetime-local" value={selected} />
        </label>
        {!selectedIsRecorded && selected !== '' && <p className="field-error" role="alert">В это время записи нет</p>}
        <div className="archive-actions">
          <button className="primary-button" disabled={!selectedIsRecorded || playback === 'starting'} onClick={() => void startPlayback()} type="button">{playback === 'starting' ? 'Запускаем…' : 'Запустить архив'}</button>
          {playback === 'playing' && <button className="secondary-button" onClick={() => cleanupPlayback(true)} type="button">Остановить архив</button>}
        </div>
      </>}
      {playback === 'error' && <p className="field-error" role="alert">Не удалось запустить архив. Повторите попытку.</p>}
    </div>
  </div>
}
