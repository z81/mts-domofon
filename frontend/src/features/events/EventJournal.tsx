import { useEffect, useRef, useState, type ReactNode } from 'react'
import { eventApi, type EventApi, type EventItem, type EventKind } from './eventApi'

type JournalVariant = 'full' | 'compact'

interface EventJournalProps {
  api?: EventApi
  flatId: string
  variant: JournalVariant
  onOpenArchive?: (cameraId: string, archiveAt: string) => void
  canOpenArchive?: (cameraId: string) => boolean
  refreshKey?: number
}

type JournalRemote =
  | { status: 'loading'; flatId: string }
  | { status: 'error'; flatId: string }
  | { status: 'success'; flatId: string; items: EventItem[]; page: number; hasMore: boolean; loadedAt: number }

const MAX_EVENT_PAGE = 20

const callLabels: Record<NonNullable<EventItem['callOutcome']>, string> = {
  accepted: 'Принятый вызов',
  missed: 'Пропущенный вызов',
  rejected: 'Отклонённый вызов',
  ignored: 'Проигнорированный вызов',
  unknown: 'Вызов домофона',
}

const iconProps = {
  'aria-hidden': true,
  fill: 'none',
  height: 20,
  stroke: 'currentColor',
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
  strokeWidth: 1.9,
  viewBox: '0 0 24 24',
  width: 20,
} as const

const kindIcons: Record<EventKind, ReactNode> = {
  call: <path d="M7.6 3.5H5.4a1.9 1.9 0 0 0-1.9 2C4.1 13.4 10.6 19.9 18.5 20.5a1.9 1.9 0 0 0 2-1.9v-2.2a1.4 1.4 0 0 0-1.1-1.4l-3.1-.7a1.4 1.4 0 0 0-1.4.4l-1.3 1.4a12.6 12.6 0 0 1-5.4-5.4l1.4-1.3a1.4 1.4 0 0 0 .4-1.4L9 4.6a1.4 1.4 0 0 0-1.4-1.1Z" />,
  access: <path d="M6 21V4.8C6 3.8 6.8 3 7.8 3h8.4c1 0 1.8.8 1.8 1.8V21M4 21h16M14.5 11.5v2" />,
  camera: <path d="M4.5 6.5h9a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-9a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Zm11 4 4.4-2.6a.7.7 0 0 1 1.1.6v7a.7.7 0 0 1-1.1.6l-4.4-2.6" />,
  other: <path d="M5.2 17h13.6l-1.8-2.2V10a5 5 0 0 0-10 0v4.8ZM10 19.6a2.2 2.2 0 0 0 4 0" />,
}

const shortMonths = ['янв', 'фев', 'мар', 'апр', 'мая', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек']
const clockFormat = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' })
const dayFormat = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long' })
const dayWithYearFormat = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', year: 'numeric' })

function sameDay(left: Date, right: Date): boolean {
  return left.getFullYear() === right.getFullYear() && left.getMonth() === right.getMonth() && left.getDate() === right.getDate()
}

function relativeDay(date: Date, now: Date): string | null {
  if (sameDay(date, now)) return 'Сегодня'
  if (sameDay(date, new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1))) return 'Вчера'
  return null
}

function eventDate(value: string, now: Date): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Дата не указана'
  return relativeDay(date, now) ?? (date.getFullYear() === now.getFullYear() ? dayFormat : dayWithYearFormat).format(date)
}

// The compact list has no day headings, so each time carries its day: «Вчера, 17:45», «27 окт, 21:53».
function eventTime(value: string, now: Date, withDay: boolean): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Время не указано'
  const time = clockFormat.format(date)
  if (!withDay) return time
  return `${relativeDay(date, now) ?? `${date.getDate()} ${shortMonths[date.getMonth()]}`}, ${time}`
}

function durationLabel(seconds?: number): string | null {
  if (seconds === undefined) return null
  const total = Math.round(seconds)
  const minutes = Math.floor(total / 60)
  const remainder = total % 60
  if (minutes === 0) return `${remainder} сек`
  return remainder > 0 ? `${minutes} мин ${remainder} сек` : `${minutes} мин`
}

function eventDetails(event: EventItem): string {
  const outcome = event.callOutcome ? callLabels[event.callOutcome] : null
  const duration = durationLabel(event.durationSeconds)
  return [
    event.sourceLabel,
    // Providers usually title a call with its outcome; the line below must not repeat it.
    outcome && outcome.toLocaleLowerCase('ru-RU') !== event.title.trim().toLocaleLowerCase('ru-RU') ? outcome : null,
    duration && event.kind === 'call' ? `звонок ${duration}` : duration,
  ].filter(Boolean).join(' · ') || 'Домофон'
}

export function EventJournal({
  api = eventApi,
  flatId,
  variant,
  onOpenArchive,
  canOpenArchive,
  refreshKey = 0,
}: EventJournalProps) {
  const [filter, setFilter] = useState<EventKind | undefined>()
  const [pagination, setPagination] = useState({ flatId, refreshKey, page: 0 })
  const [reload, setReload] = useState(0)
  const [remote, setRemote] = useState<JournalRemote>({ status: 'loading', flatId })
  const rangeEnd = useRef<number | null>(null)
  const requestedPage = pagination.flatId === flatId && pagination.refreshKey === refreshKey ? pagination.page : 0
  const currentRemote: JournalRemote = remote.flatId === flatId
    ? remote
    : { status: 'loading', flatId }

  useEffect(() => {
    const controller = new AbortController()
    const size = variant === 'compact' ? 3 : 20
    // The first page reads up to now; later pages keep its end so new events do not shift them.
    const to = requestedPage === 0 || rangeEnd.current === null ? Date.now() : rangeEnd.current
    rangeEnd.current = to
    api.getEvents({
      flatId,
      page: requestedPage,
      size,
      from: new Date(to - 30 * 24 * 60 * 60 * 1_000).toISOString(),
      to: new Date(to).toISOString(),
      kind: filter,
    }, controller.signal).then((page) => {
      if (controller.signal.aborted) return
      const loadedAt = Date.now()
      setRemote((previous) => {
        const appended = requestedPage > 0 && previous.status === 'success' && previous.flatId === flatId
        return {
          status: 'success',
          flatId,
          items: appended
            ? [...previous.items, ...page.items.filter((item) => !previous.items.some((known) => known.id === item.id))]
            : page.items,
          page: page.page,
          hasMore: page.hasMore && page.page < MAX_EVENT_PAGE,
          loadedAt,
        }
      })
    }).catch(() => {
      if (!controller.signal.aborted) setRemote({ status: 'error', flatId })
    })
    return () => controller.abort()
  }, [api, filter, flatId, pagination.flatId, refreshKey, reload, requestedPage, variant])

  // Coming back to the app reloads the newest events; older pages someone is reading stay as they are.
  useEffect(() => {
    if (requestedPage > 0) return
    function reloadWhenVisible() {
      if (document.visibilityState === 'visible') setReload((value) => value + 1)
    }
    document.addEventListener('visibilitychange', reloadWhenVisible)
    return () => document.removeEventListener('visibilitychange', reloadWhenVisible)
  }, [requestedPage])

  function chooseFilter(next: EventKind | undefined) {
    if (filter === next) return
    setFilter(next)
    setPagination({ flatId, refreshKey, page: 0 })
    setRemote({ status: 'loading', flatId })
  }

  const now = currentRemote.status === 'success' ? new Date(currentRemote.loadedAt) : null

  const content = <>
    {variant === 'full' && <div className="event-filters" role="group" aria-label="Фильтр событий">
      <button aria-pressed={filter === undefined} className={filter === undefined ? 'selected' : ''} onClick={() => chooseFilter(undefined)} type="button">Все</button>
      <button aria-pressed={filter === 'call'} className={filter === 'call' ? 'selected' : ''} onClick={() => chooseFilter('call')} type="button">Звонки</button>
      <button aria-pressed={filter === 'access'} className={filter === 'access' ? 'selected' : ''} onClick={() => chooseFilter('access')} type="button">Доступ</button>
      <button aria-pressed={filter === 'camera'} className={filter === 'camera' ? 'selected' : ''} onClick={() => chooseFilter('camera')} type="button">Камеры</button>
    </div>}
    {currentRemote.status === 'loading' && <div className="journal-state" role="status"><span className="loading-ring" />Загружаем журнал…</div>}
    {currentRemote.status === 'error' && <div className="journal-state" role={variant === 'full' ? 'alert' : undefined}>
      <p>Не удалось загрузить события</p>
      <button className="secondary-button" onClick={() => { setRemote({ status: 'loading', flatId }); setPagination({ flatId, refreshKey, page: 0 }); setReload((value) => value + 1) }} type="button">Повторить</button>
    </div>}
    {currentRemote.status === 'success' && currentRemote.items.length === 0 && <p className="empty-copy">
      За последние 30 дней событий нет
    </p>}
    {currentRemote.status === 'success' && now && currentRemote.items.length > 0 && <div className="event-list">
      {currentRemote.items.map((event, index) => {
        const date = eventDate(event.occurredAt, now)
        const showDate = variant === 'full' && (index === 0 || eventDate(currentRemote.items[index - 1].occurredAt, now) !== date)
        return <div className="event-row-wrap" key={event.id}>
          {showDate && <h3 className="event-date">{date}</h3>}
          <article className={`event-row event-${event.kind}`}>
            <span className="event-icon" aria-hidden="true"><svg {...iconProps}>{kindIcons[event.kind]}</svg></span>
            <b className="event-title">{event.title}</b>
            <small className="event-details">{eventDetails(event)}</small>
            <time dateTime={event.occurredAt}>{eventTime(event.occurredAt, now, variant === 'compact')}</time>
            {variant === 'full' && event.cameraId && event.archiveAt && onOpenArchive && canOpenArchive?.(event.cameraId) && <button
              className="text-button event-archive-button"
              onClick={() => onOpenArchive(event.cameraId!, event.archiveAt!)}
              type="button"
            >Открыть запись камеры</button>}
          </article>
        </div>
      })}
      {currentRemote.hasMore && variant !== 'compact' && <button className="secondary-button journal-more" onClick={() => setPagination({ flatId, refreshKey, page: currentRemote.page + 1 })} type="button">Показать ещё</button>}
    </div>}
  </>

  if (variant === 'full') {
    return <>
      <header className="screen-heading"><h1 tabIndex={-1}>События</h1></header>
      <section className="content-card event-journal">{content}</section>
    </>
  }
  return <section className={`content-card event-journal event-journal-${variant}`}>
    {content}
  </section>
}
