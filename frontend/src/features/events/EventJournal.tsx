import { useEffect, useMemo, useState } from 'react'
import { eventApi, type EventApi, type EventItem, type EventKind } from './eventApi'

type JournalVariant = 'full' | 'compact'

interface EventJournalProps {
  api?: EventApi
  flatId: string
  variant: JournalVariant
  onOpenArchive?: (cameraId: string, archiveAt: string) => void
}

type JournalRemote =
  | { status: 'loading'; flatId: string }
  | { status: 'error'; flatId: string }
  | { status: 'success'; flatId: string; items: EventItem[]; page: number; hasMore: boolean }

const MAX_EVENT_PAGE = 20

const callLabels: Record<NonNullable<EventItem['callOutcome']>, string> = {
  accepted: 'Принятый вызов',
  missed: 'Пропущенный вызов',
  rejected: 'Отклонённый вызов',
  ignored: 'Проигнорированный вызов',
  unknown: 'Вызов домофона',
}

const kindGlyphs: Record<EventKind, string> = {
  call: '☎',
  access: '⌑',
  camera: '◉',
  other: '•',
}

function eventTime(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Время не указано'
  return new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  }).format(date)
}

function eventDate(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Дата не указана'
  return new Intl.DateTimeFormat('ru-RU', {
    day: 'numeric', month: 'long', year: 'numeric',
  }).format(date)
}

function durationLabel(seconds?: number): string | null {
  if (seconds === undefined) return null
  const minutes = Math.floor(seconds / 60)
  const remainder = seconds % 60
  return minutes > 0 ? `${minutes} мин ${remainder} сек` : `${remainder} сек`
}

export function EventJournal({
  api = eventApi,
  flatId,
  variant,
  onOpenArchive,
}: EventJournalProps) {
  const [filter, setFilter] = useState<EventKind | undefined>()
  const [pagination, setPagination] = useState({ flatId, page: 0 })
  const [reload, setReload] = useState(0)
  const [remote, setRemote] = useState<JournalRemote>({ status: 'loading', flatId })
  const requestedPage = pagination.flatId === flatId ? pagination.page : 0
  const currentRemote: JournalRemote = remote.flatId === flatId
    ? remote
    : { status: 'loading', flatId }
  const range = useMemo(() => {
    const to = new Date()
    const from = new Date(to.getTime() - 30 * 24 * 60 * 60 * 1_000)
    return { from: from.toISOString(), to: to.toISOString() }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    const size = variant === 'compact' ? 3 : 20
    api.getEvents({
      flatId,
      page: requestedPage,
      size,
      from: range.from,
      to: range.to,
      kind: filter,
    }, controller.signal).then((page) => {
      if (controller.signal.aborted) return
      setRemote((previous) => ({
        status: 'success',
        flatId,
        items: requestedPage > 0 && previous.status === 'success' && previous.flatId === flatId
          ? [...previous.items, ...page.items]
          : page.items,
        page: page.page,
        hasMore: page.hasMore && page.page < MAX_EVENT_PAGE,
      }))
    }).catch(() => {
      if (!controller.signal.aborted) setRemote({ status: 'error', flatId })
    })
    return () => controller.abort()
  }, [api, filter, flatId, pagination.flatId, range.from, range.to, reload, requestedPage, variant])

  function chooseFilter(next: EventKind | undefined) {
    if (filter === next) return
    setFilter(next)
    setPagination({ flatId, page: 0 })
    setRemote({ status: 'loading', flatId })
  }

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
      <button className="secondary-button" onClick={() => { setRemote({ status: 'loading', flatId }); setPagination({ flatId, page: 0 }); setReload((value) => value + 1) }} type="button">Повторить</button>
    </div>}
    {currentRemote.status === 'success' && currentRemote.items.length === 0 && <p className="empty-copy">
      Событий за выбранный период нет
    </p>}
    {currentRemote.status === 'success' && currentRemote.items.length > 0 && <div className="event-list">
      {currentRemote.items.map((event, index) => {
        const showDate = index === 0 || eventDate(currentRemote.items[index - 1].occurredAt) !== eventDate(event.occurredAt)
        const duration = durationLabel(event.durationSeconds)
        return <div className="event-row-wrap" key={event.id}>
          {showDate && <h3 className="event-date">{eventDate(event.occurredAt)}</h3>}
          <article className={`event-row event-${event.kind}`}>
            <span className="event-glyph" aria-hidden="true">{kindGlyphs[event.kind]}</span>
            <span className="event-copy">
              <b>{event.title}</b>
              <small>{[
                event.sourceLabel,
                event.callOutcome ? callLabels[event.callOutcome] : null,
                duration,
              ].filter(Boolean).join(' · ') || 'Домофон'}</small>
            </span>
            <time dateTime={event.occurredAt}>{eventTime(event.occurredAt)}</time>
            {event.cameraId && event.archiveAt && onOpenArchive && <button
              className="text-button event-archive-button"
              onClick={() => onOpenArchive(event.cameraId!, event.archiveAt!)}
              type="button"
            >Открыть запись камеры</button>}
          </article>
        </div>
      })}
      {currentRemote.hasMore && variant !== 'compact' && <button className="secondary-button journal-more" onClick={() => setPagination({ flatId, page: currentRemote.page + 1 })} type="button">Показать ещё</button>}
    </div>}
  </>

  if (variant === 'full') {
    return <>
      <header className="screen-heading"><h1>События</h1></header>
      <section className="content-card event-journal">{content}</section>
    </>
  }
  return <section className={`content-card event-journal event-journal-${variant}`}>
    {content}
  </section>
}
