import { useEffect, useMemo, useState } from 'react'
import { eventApi, type EventApi, type EventItem, type EventKind } from './eventApi'

type JournalVariant = 'full' | 'compact' | 'calls'

interface EventJournalProps {
  api?: EventApi
  flatId: string
  variant: JournalVariant
  onOpenArchive?: (cameraId: string, archiveAt: string) => void
}

type JournalRemote =
  | { status: 'loading' }
  | { status: 'error' }
  | { status: 'success'; items: EventItem[]; page: number; hasMore: boolean }

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

function eventTitle(event: EventItem): string {
  if (event.kind === 'call' && event.callOutcome) return callLabels[event.callOutcome]
  return event.title
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
  const [filter, setFilter] = useState<EventKind | undefined>(variant === 'calls' ? 'call' : undefined)
  const [requestedPage, setRequestedPage] = useState(0)
  const [reload, setReload] = useState(0)
  const [remote, setRemote] = useState<JournalRemote>({ status: 'loading' })
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
      kind: variant === 'calls' ? 'call' : filter,
    }, controller.signal).then((page) => {
      if (controller.signal.aborted) return
      setRemote((previous) => ({
        status: 'success',
        items: requestedPage > 0 && previous.status === 'success'
          ? [...previous.items, ...page.items]
          : page.items,
        page: page.page,
        hasMore: page.hasMore,
      }))
    }).catch(() => {
      if (!controller.signal.aborted) setRemote({ status: 'error' })
    })
    return () => controller.abort()
  }, [api, filter, flatId, range.from, range.to, reload, requestedPage, variant])

  function chooseFilter(next: EventKind | undefined) {
    setFilter(next)
    setRequestedPage(0)
    setRemote({ status: 'loading' })
  }

  const title = variant === 'calls' ? 'История звонков' : variant === 'compact' ? 'Последние события' : 'Журнал событий'
  const content = <>
    {variant === 'full' && <div className="event-filters" role="group" aria-label="Фильтр событий">
      <button className={filter === undefined ? 'selected' : ''} onClick={() => chooseFilter(undefined)} type="button">Все события</button>
      <button className={filter === 'call' ? 'selected' : ''} onClick={() => chooseFilter('call')} type="button" aria-label="Только звонки">Звонки</button>
      <button className={filter === 'access' ? 'selected' : ''} onClick={() => chooseFilter('access')} type="button">Доступ</button>
      <button className={filter === 'camera' ? 'selected' : ''} onClick={() => chooseFilter('camera')} type="button">Камеры</button>
    </div>}
    {remote.status === 'loading' && <div className="journal-state" role="status"><span className="loading-ring" />Загружаем журнал…</div>}
    {remote.status === 'error' && <div className="journal-state" role={variant === 'full' ? 'alert' : undefined}>
      <p>{variant === 'calls' ? 'Не удалось загрузить историю звонков' : 'Не удалось загрузить журнал событий'}</p>
      <button className="secondary-button" onClick={() => { setRemote({ status: 'loading' }); setRequestedPage(0); setReload((value) => value + 1) }} type="button">Повторить</button>
    </div>}
    {remote.status === 'success' && remote.items.length === 0 && <p className="empty-copy">
      {variant === 'calls' ? 'История звонков пока пуста' : 'Событий за выбранный период нет'}
    </p>}
    {remote.status === 'success' && remote.items.length > 0 && <div className="event-list">
      {remote.items.map((event, index) => {
        const showDate = index === 0 || eventDate(remote.items[index - 1].occurredAt) !== eventDate(event.occurredAt)
        const duration = durationLabel(event.durationSeconds)
        return <div className="event-row-wrap" key={event.id}>
          {showDate && <h3 className="event-date">{eventDate(event.occurredAt)}</h3>}
          <article className={`event-row event-${event.kind}`}>
            <span className="event-glyph" aria-hidden="true">{kindGlyphs[event.kind]}</span>
            <span className="event-copy">
              <b>{eventTitle(event)}</b>
              <small>{[event.sourceLabel, duration].filter(Boolean).join(' · ') || 'Домофон'}</small>
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
      {remote.hasMore && variant !== 'compact' && <button className="secondary-button journal-more" onClick={() => setRequestedPage(remote.page + 1)} type="button">Показать ещё</button>}
    </div>}
  </>

  if (variant === 'full') {
    return <>
      <section className="page-heading"><div><p className="eyebrow">События домофона</p><h1>{title}</h1><p>Реальные события выбранного объекта за последние 30 дней.</p></div></section>
      <section className="content-card event-journal">{content}</section>
    </>
  }
  return <section className={`content-card event-journal event-journal-${variant}`}>
    <div className="section-title-row"><div><p className="eyebrow">{variant === 'calls' ? 'Звонки' : 'События'}</p><h2>{title}</h2></div></div>
    {content}
  </section>
}
