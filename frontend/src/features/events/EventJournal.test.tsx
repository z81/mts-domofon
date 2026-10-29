import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { EventJournal } from './EventJournal'
import type { EventApi, EventPage } from './eventApi'

const events: EventPage = {
  page: 0,
  hasMore: true,
  items: [
    {
      id: 'call-1',
      occurredAt: '2026-08-07T10:00:00Z',
      kind: 'call',
      title: 'Пропущенный вызов',
      callOutcome: 'missed',
      sourceLabel: 'Подъезд 1',
      durationSeconds: 10,
    },
    {
      id: 'camera-1',
      occurredAt: '2026-08-07T09:30:00Z',
      kind: 'camera',
      title: 'Движение у двери',
      cameraId: 'camera / 1',
      archiveAt: '2026-08-07T09:30:00Z',
    },
    {
      id: 'camera-without-archive-1',
      occurredAt: '2026-08-06T11:30:00Z',
      kind: 'camera',
      title: 'Камера недоступна',
      cameraId: 'camera / 2',
      archiveAt: '2026-08-06T11:30:00Z',
    },
    {
      id: 'archive-without-camera-1',
      occurredAt: '2026-08-06T11:00:00Z',
      kind: 'camera',
      title: 'Запись без камеры',
      archiveAt: '2026-08-06T11:00:00Z',
    },
  ],
}

describe('event journal', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('renders chronological event details and opens only a complete camera archive target', async () => {
    const user = userEvent.setup()
    const getEvents = vi.fn<EventApi['getEvents']>(async () => events)
    const onOpenArchive = vi.fn()
    render(<EventJournal
      api={{ getEvents }}
      canOpenArchive={(cameraId) => cameraId === 'camera / 1'}
      flatId="flat-81"
      onOpenArchive={onOpenArchive}
      variant="full"
    />)

    expect(await screen.findByRole('heading', { name: 'События' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Все' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Все' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('button', { name: 'Звонки' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Звонки' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('button', { name: 'Доступ' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Камеры' })).toBeVisible()
    expect(screen.getByText('Пропущенный вызов')).toBeVisible()
    expect(screen.getAllByText(/Пропущенный вызов/)).toHaveLength(1)
    expect(screen.getByText('Подъезд 1 · звонок 10 сек')).toBeVisible()
    expect(screen.getByText('Движение у двери')).toBeVisible()
    expect(screen.getAllByText(/^\d{2}:\d{2}$/)).toHaveLength(4)
    expect(screen.getAllByRole('button', { name: 'Открыть запись камеры' })).toHaveLength(1)
    expect(screen.getByRole('button', { name: 'Показать ещё' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Открыть запись камеры' }))
    expect(onOpenArchive).toHaveBeenCalledWith('camera / 1', '2026-08-07T09:30:00Z')

    await user.click(screen.getByRole('button', { name: 'Звонки' }))
    expect(screen.getByRole('button', { name: 'Все' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('button', { name: 'Звонки' })).toHaveAttribute('aria-pressed', 'true')
    await waitFor(() => expect(getEvents).toHaveBeenLastCalledWith(
      expect.objectContaining({ flatId: 'flat-81', kind: 'call' }),
      expect.any(AbortSignal),
    ))
  })

  it('shows a stable retry state without rendering a raw error', async () => {
    const user = userEvent.setup()
    const getEvents = vi.fn<EventApi['getEvents']>()
      .mockRejectedValueOnce(new Error('private upstream response'))
      .mockResolvedValueOnce({ items: [], page: 0, hasMore: false })
    render(<EventJournal api={{ getEvents }} flatId="flat-81" variant="full" />)

    expect(await screen.findByText('Не удалось загрузить события')).toBeVisible()
    expect(document.body).not.toHaveTextContent('private upstream response')
    await user.click(screen.getByRole('button', { name: 'Повторить' }))
    expect(await screen.findByText('За последние 30 дней событий нет')).toBeVisible()
  })

  it('leaves compact heading ownership to its parent and dates each compact row', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 7, 7, 12, 0))
    const getEvents = vi.fn<EventApi['getEvents']>(async () => ({
      page: 0,
      hasMore: false,
      items: [
        { id: 'today', occurredAt: new Date(2026, 7, 7, 10, 5).toISOString(), kind: 'call', title: 'Пропущенный вызов', callOutcome: 'missed', durationSeconds: 75 },
        { id: 'yesterday', occurredAt: new Date(2026, 7, 6, 21, 53).toISOString(), kind: 'access', title: 'Дверь открыта', sourceLabel: 'Подъезд 1' },
        { id: 'older', occurredAt: new Date(2026, 6, 27, 9, 0).toISOString(), kind: 'access', title: 'Дверь открыта', sourceLabel: 'Подъезд 1' },
      ],
    }))
    render(<section aria-label="Последние события">
      <h2>Последние события</h2>
      <EventJournal api={{ getEvents }} flatId="flat-81" variant="compact" />
    </section>)

    expect(await screen.findByText('Сегодня, 10:05')).toBeVisible()
    expect(screen.getByText('Вчера, 21:53')).toBeVisible()
    expect(screen.getByText('27 июл, 09:00')).toBeVisible()
    expect(screen.getByText('звонок 1 мин 15 сек')).toBeVisible()
    expect(screen.getAllByRole('heading')).toHaveLength(1)
    expect(screen.getAllByRole('heading', { name: 'Последние события' })).toHaveLength(1)
  })

  it('stops pagination at the API page limit even when hasMore remains true', async () => {
    const getEvents = vi.fn<EventApi['getEvents']>(async () => ({
      items: [{
        id: 'terminal-event',
        occurredAt: '2026-08-07T10:00:00Z',
        kind: 'access',
        title: 'Последнее доступное событие',
      }],
      page: 20,
      hasMore: true,
    }))
    render(<EventJournal api={{ getEvents }} flatId="flat-81" variant="full" />)

    expect(await screen.findByText('Последнее доступное событие')).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Показать ещё' })).not.toBeInTheDocument()
  })

  it('starts from page zero when the flat changes and reloads only a first page when the app becomes visible', async () => {
    const getEvents = vi.fn<EventApi['getEvents']>(async (query) => ({
      items: [{
        id: `${query.flatId}-${query.page}`,
        occurredAt: '2026-08-07T10:00:00Z',
        kind: 'access',
        title: `${query.flatId} страница ${query.page}`,
      }],
      page: query.page,
      hasMore: query.flatId === 'flat-81' && query.page === 0,
    }))
    const view = render(<EventJournal api={{ getEvents }} flatId="flat-81" variant="full" />)

    expect(await screen.findByText('flat-81 страница 0')).toBeVisible()
    await userEvent.click(screen.getByRole('button', { name: 'Показать ещё' }))
    expect(await screen.findByText('flat-81 страница 1')).toBeVisible()
    const loadsWhileReadingOlderPages = getEvents.mock.calls.length
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'))
    })
    expect(getEvents).toHaveBeenCalledTimes(loadsWhileReadingOlderPages)

    view.rerender(<EventJournal api={{ getEvents }} flatId="flat-22" variant="full" />)

    expect(await screen.findByText('flat-22 страница 0')).toBeVisible()
    expect(screen.queryByText(/flat-81 страница/)).not.toBeInTheDocument()
    expect(getEvents).toHaveBeenLastCalledWith(
      expect.objectContaining({ flatId: 'flat-22', page: 0 }),
      expect.any(AbortSignal),
    )
    const loadsOnFirstPage = getEvents.mock.calls.length
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'))
    })
    await waitFor(() => expect(getEvents).toHaveBeenCalledTimes(loadsOnFirstPage + 1))
    expect(getEvents).toHaveBeenLastCalledWith(
      expect.objectContaining({ flatId: 'flat-22', page: 0 }),
      expect.any(AbortSignal),
    )
  })

  it('keeps loaded content when every already-selected filter is clicked again', async () => {
    const user = userEvent.setup()
    const getEvents = vi.fn<EventApi['getEvents']>(async (query) => ({
      items: [{
        id: query.kind ?? 'all',
        occurredAt: '2026-08-07T10:00:00Z',
        kind: query.kind ?? 'other',
        title: `Загружено: ${query.kind ?? 'all'}`,
      }],
      page: 0,
      hasMore: false,
    }))
    render(<EventJournal api={{ getEvents }} flatId="flat-81" variant="full" />)

    const filters = [
      ['Все', 'all'],
      ['Звонки', 'call'],
      ['Доступ', 'access'],
      ['Камеры', 'camera'],
    ] as const
    for (const [label, result] of filters) {
      if (label !== 'Все') await user.click(screen.getByRole('button', { name: label }))
      expect(await screen.findByText(`Загружено: ${result}`)).toBeVisible()
      const callsBeforeRepeat = getEvents.mock.calls.length

      await user.click(screen.getByRole('button', { name: label }))

      expect(screen.getByText(`Загружено: ${result}`)).toBeVisible()
      expect(screen.queryByText('Загружаем журнал…')).not.toBeInTheDocument()
      expect(getEvents).toHaveBeenCalledTimes(callsBeforeRepeat)
    }
  })
})
