import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  archiveHlsConfig,
  attachStream,
  isAutoplayBlocked,
  pickPlaybackEngine,
  playVideo,
  watchPlayback,
  type AttachedStream,
  type PlaybackWatch,
} from './hlsPlayback'
import { IntercomApiError, type ArchiveDistribution, type ArchiveInterval, type IntercomApi, type LiveSession } from './intercomApi'
import { primeVideo } from './livePriming'
import { PlayIcon, SoundIcon } from './MediaIcons'

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

type ArchivePlayback =
  | { status: 'idle' }
  | { status: 'starting' | 'playing'; startAt: string }
  | { status: 'interrupted'; startAt: string; resumeAt: string }
  | { status: 'error'; startAt: string; reason: 'not_found' | 'failed' | 'unsupported' }

const eventLeadMs = 10_000
const latestWindowMs = 5 * 60_000

const playbackErrors: Record<'not_found' | 'failed' | 'unsupported', string> = {
  not_found: 'Запись за это время недоступна',
  failed: 'Не удалось загрузить запись',
  unsupported: 'Браузер не может показать запись',
}

function timeOf(value: string): number {
  return new Date(value).getTime()
}

function toLocalInput(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
  return local.toISOString().slice(0, 19)
}

function fromLocalInput(value: string): string | null {
  const date = new Date(value)
  return value === '' || Number.isNaN(date.getTime()) ? null : date.toISOString()
}

// Interval ends are exclusive: the last recorded second starts one second before `to`.
function containsTime(interval: ArchiveInterval, time: number): boolean {
  return timeOf(interval.from) <= time && time < timeOf(interval.to)
}

function newestFirst(intervals: ArchiveInterval[]): ArchiveInterval[] {
  return intervals
    .filter((interval) => timeOf(interval.from) < timeOf(interval.to))
    .sort((left, right) => timeOf(right.from) - timeOf(left.from))
}

function initialStart(intervals: ArchiveInterval[], initialAt?: string): { selected: string | null; start: string | null } {
  if (initialAt) {
    const target = timeOf(initialAt)
    if (Number.isNaN(target)) return { selected: null, start: null }
    const interval = intervals.find((candidate) => containsTime(candidate, target))
    if (!interval) return { selected: new Date(target).toISOString(), start: null }
    const start = new Date(Math.max(timeOf(interval.from), target - eventLeadMs)).toISOString()
    return { selected: start, start }
  }
  const latest = intervals.reduce<ArchiveInterval | undefined>((current, interval) => (
    !current || timeOf(interval.to) > timeOf(current.to) ? interval : current
  ), undefined)
  if (!latest) return { selected: null, start: null }
  const start = new Date(Math.max(timeOf(latest.from), timeOf(latest.to) - latestWindowMs)).toISOString()
  return { selected: start, start }
}

const dayFormat = new Intl.DateTimeFormat('ru-RU', { day: '2-digit', month: '2-digit' })
const clockFormat = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' })
const secondsFormat = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit', second: '2-digit' })

function intervalLabel(interval: ArchiveInterval, withDay: boolean): string {
  const from = timeOf(interval.from)
  const to = timeOf(interval.to)
  const sameDay = dayFormat.format(from) === dayFormat.format(to - 1_000)
  const start = `${withDay ? `${dayFormat.format(from)} ` : ''}${clockFormat.format(from)}`
  const end = `${sameDay ? '' : `${dayFormat.format(to)} `}${clockFormat.format(to)}`
  return `${start}–${end}`
}

function recordingLabel(time: number): string {
  return `${dayFormat.format(time)} ${secondsFormat.format(time)}`
}

export function ArchivePlayer({ api, flatId, cameraId, cameraName, initialAt }: ArchivePlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const generationRef = useRef(0)
  const sessionRef = useRef<string | null>(null)
  const startAtRef = useRef<string | null>(null)
  const streamRef = useRef<AttachedStream | null>(null)
  const watchRef = useRef<PlaybackWatch | null>(null)
  const [reload, setReload] = useState(0)
  const [distribution, setDistribution] = useState<DistributionRemote>(
    api.getArchiveDistribution ? { status: 'loading' } : { status: 'error' },
  )
  const [selectedInput, setSelectedInput] = useState('')
  const [playback, setPlayback] = useState<ArchivePlayback>({ status: 'idle' })
  const [playedSeconds, setPlayedSeconds] = useState(0)
  const [soundBlocked, setSoundBlocked] = useState(false)
  const [playBlocked, setPlayBlocked] = useState(false)

  // Archive sessions are private, so the previous one is always DELETEd (keepalive survives page unload).
  const stopCurrent = useCallback((): number => {
    generationRef.current += 1
    watchRef.current?.dispose()
    watchRef.current = null
    streamRef.current?.destroy()
    streamRef.current = null
    const sessionId = sessionRef.current
    sessionRef.current = null
    if (sessionId) void api.stopLive?.(sessionId).catch(() => undefined)
    return generationRef.current
  }, [api])

  const interrupt = useCallback((generation: number) => {
    const startAt = startAtRef.current
    if (generation !== generationRef.current || !startAt) return
    const resumeAt = new Date(timeOf(startAt) + Math.floor(videoRef.current?.currentTime ?? 0) * 1_000).toISOString()
    stopCurrent()
    setPlayBlocked(false)
    setPlayback({ status: 'interrupted', startAt, resumeAt })
  }, [stopCurrent])

  const startPlayback = useCallback(async (startAt: string) => {
    const video = videoRef.current
    const startArchive = api.startArchive
    if (!video || !startArchive) return
    const generation = stopCurrent()
    startAtRef.current = startAt
    setSelectedInput(toLocalInput(startAt))
    setPlayedSeconds(0)
    setSoundBlocked(false)
    setPlayBlocked(false)
    setPlayback({ status: 'starting', startAt })
    const engine = pickPlaybackEngine(video)
    if (!engine) {
      setPlayback({ status: 'error', startAt, reason: 'unsupported' })
      return
    }
    let session: LiveSession
    try {
      // Not aborted on leave: a session created after the abort could not be deleted without its id.
      session = await startArchive(flatId, cameraId, startAt)
    } catch (error) {
      if (generation !== generationRef.current) return
      setPlayback({ status: 'error', startAt, reason: error instanceof IntercomApiError && error.status === 404 ? 'not_found' : 'failed' })
      return
    }
    if (generation !== generationRef.current) {
      void api.stopLive?.(session.sessionId).catch(() => undefined)
      return
    }
    sessionRef.current = session.sessionId
    streamRef.current = attachStream(video, engine, session.playlistUrl, archiveHlsConfig, {
      onFailure: () => interrupt(generation),
    })
    watchRef.current = watchPlayback(video, {
      onFirstFrame: () => {
        if (generation === generationRef.current) setPlayback({ status: 'playing', startAt })
      },
      onStall: () => interrupt(generation),
    })
    try {
      await playVideo(video)
      return
    } catch (error) {
      if (generation !== generationRef.current || !isAutoplayBlocked(error)) return
    }
    if (!video.muted) {
      video.muted = true
      setSoundBlocked(true)
      try {
        await playVideo(video)
        return
      } catch (error) {
        if (generation !== generationRef.current || !isAutoplayBlocked(error)) return
      }
    }
    setPlayBlocked(true)
  }, [api, cameraId, flatId, interrupt, stopCurrent])

  useEffect(() => {
    const getDistribution = api.getArchiveDistribution
    if (!getDistribution) return
    const controller = new AbortController()
    getDistribution(flatId, cameraId, controller.signal).then((data) => {
      if (controller.signal.aborted) return
      setDistribution({ status: 'success', data })
      const initial = initialStart(newestFirst(data.intervals), initialAt)
      setSelectedInput(initial.selected ? toLocalInput(initial.selected) : '')
      if (initial.start) void startPlayback(initial.start)
    }).catch(() => {
      if (!controller.signal.aborted) setDistribution({ status: 'error' })
    })
    return () => controller.abort()
  }, [api, cameraId, flatId, initialAt, reload, startPlayback])

  useEffect(() => () => {
    stopCurrent()
  }, [stopCurrent])

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    const onTimeUpdate = () => setPlayedSeconds(Math.floor(video.currentTime))
    const onVolumeChange = () => {
      if (!video.muted) setSoundBlocked(false)
    }
    const onPlaying = () => setPlayBlocked(false)
    const onPageHide = () => {
      if (sessionRef.current) interrupt(generationRef.current)
    }
    video.addEventListener('timeupdate', onTimeUpdate)
    video.addEventListener('volumechange', onVolumeChange)
    video.addEventListener('playing', onPlaying)
    window.addEventListener('pagehide', onPageHide)
    return () => {
      video.removeEventListener('timeupdate', onTimeUpdate)
      video.removeEventListener('volumechange', onVolumeChange)
      video.removeEventListener('playing', onPlaying)
      window.removeEventListener('pagehide', onPageHide)
    }
  }, [interrupt])

  const intervals = useMemo(() => distribution.status === 'success' ? newestFirst(distribution.data.intervals) : [], [distribution])
  const selectedIso = useMemo(() => fromLocalInput(selectedInput), [selectedInput])
  const selectedTime = selectedIso === null ? null : timeOf(selectedIso)
  const selectedIsRecorded = selectedTime !== null && intervals.some((interval) => containsTime(interval, selectedTime))
  const withDay = intervals.length > 0 && new Set(intervals.flatMap((interval) => [
    dayFormat.format(timeOf(interval.from)),
    dayFormat.format(timeOf(interval.to) - 1_000),
  ])).size > 1
  const earliest = intervals.at(-1)
  const latestEnd = intervals.reduce((end, interval) => Math.max(end, timeOf(interval.to)), 0)
  const activeStart = playback.status === 'idle' ? null : playback.startAt
  const showWatchFrom = selectedIsRecorded && selectedIso !== null && (playback.status === 'idle' || selectedIso !== activeStart)

  function play(startAt: string) {
    if (videoRef.current) primeVideo(videoRef.current)
    void startPlayback(startAt)
  }

  function enableSound() {
    const video = videoRef.current
    if (!video) return
    video.muted = false
    if (video.paused) void playVideo(video).catch(() => undefined)
  }

  function resumePlayback() {
    const video = videoRef.current
    if (video) void playVideo(video).catch(() => undefined)
  }

  let stageOverlay: ReactNode = null
  if (playback.status === 'idle') {
    stageOverlay = <div className="archive-placeholder">
      {distribution.status === 'loading' ? <><span className="loading-ring" /><p>Ищем записи…</p></>
        : distribution.status === 'error' ? <p>Не удалось загрузить записи</p>
          : intervals.length === 0 ? <p>Записей за доступный период нет</p>
            : <p>Выберите время записи</p>}
    </div>
  } else if (playback.status === 'starting') {
    stageOverlay = <div className="live-status live-status-pending"><span><span className="loading-ring" />Загружаем запись…</span></div>
  } else if (playback.status === 'interrupted') {
    stageOverlay = <div className="live-status live-status-error">
      <p>Запись прервалась</p>
      <button className="media-action" onClick={() => play(playback.resumeAt)} type="button">Продолжить</button>
    </div>
  } else if (playback.status === 'error') {
    stageOverlay = <div className="live-status live-status-error">
      <p>{playbackErrors[playback.reason]}</p>
      {playback.reason === 'failed' && <button className="media-action" onClick={() => play(playback.startAt)} type="button">Повторить</button>}
    </div>
  }

  return <div className="archive-player">
    <div className="archive-stage">
      <video aria-label={`Архив камеры ${cameraName}`} controls={playback.status === 'playing' || playBlocked} playsInline ref={videoRef} />
      {stageOverlay}
      {playback.status === 'playing' && <span className="archive-badge">Запись: {recordingLabel(timeOf(playback.startAt) + playedSeconds * 1_000)}</span>}
      {playBlocked && <button className="live-play" onClick={resumePlayback} type="button"><PlayIcon />Смотреть</button>}
      {soundBlocked && <button className="live-sound live-sound-off archive-sound" onClick={enableSound} type="button"><SoundIcon muted /><span>Включить звук</span></button>}
    </div>
    <div className="archive-controls">
      <div className="section-title-row"><div><p className="eyebrow">Записи камеры</p><h2>Архив</h2></div></div>
      {distribution.status === 'loading' && <div className="journal-state" role="status"><span className="loading-ring" />Ищем записи…</div>}
      {distribution.status === 'error' && <div className="journal-state" role="alert"><p>Не удалось загрузить записи</p><button className="secondary-button" onClick={() => { setDistribution({ status: 'loading' }); setReload((value) => value + 1) }} type="button">Повторить</button></div>}
      {distribution.status === 'success' && intervals.length === 0 && <p className="empty-copy">Записей за доступный период нет</p>}
      {intervals.length > 0 && earliest && <>
        <div aria-label="Доступные интервалы" className="archive-intervals" role="group">
          {intervals.map((interval) => {
            const pressed = selectedTime !== null && containsTime(interval, selectedTime)
            return <button
              aria-pressed={pressed}
              className={pressed ? 'selected' : ''}
              key={`${interval.from}-${interval.to}`}
              onClick={() => play(new Date(timeOf(interval.from)).toISOString())}
              type="button"
            >{intervalLabel(interval, withDay)}</button>
          })}
        </div>
        <label className="archive-time-field">Начало просмотра
          <input
            max={toLocalInput(new Date(latestEnd - 1_000).toISOString())}
            min={toLocalInput(earliest.from)}
            onChange={(event) => setSelectedInput(event.target.value)}
            step="1"
            type="datetime-local"
            value={selectedInput}
          />
        </label>
        {!selectedIsRecorded && selectedInput !== '' && <p className="field-error" role="alert">В это время записи нет</p>}
        {showWatchFrom && <div className="archive-actions">
          <button className="primary-button" onClick={() => play(selectedIso)} type="button">Смотреть с {secondsFormat.format(selectedTime ?? 0)}</button>
        </div>}
      </>}
    </div>
  </div>
}
