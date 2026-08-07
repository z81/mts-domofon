import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { EventJournal } from './EventJournal'
import type { EventApi, EventPage } from './eventApi'

const events: EventPage = {
  page: 0,
  hasMore: false,
  items: [
    {
      id: 'call-1',
      occurredAt: '2026-08-07T10:00:00Z',
      kind: 'call',
      title: 'Incoming call',
      callOutcome: 'missed',
      sourceLabel: 'Подъезд 1',
    },
    {
      id: 'camera-1',
      occurredAt: '2026-08-07T09:30:00Z',
      kind: 'camera',
      title: 'Движение у двери',
      cameraId: 'camera / 1',
      archiveAt: '2026-08-07T09:30:00Z',
    },
  ],
}

describe('event journal', () => {
  it('renders normalized events, filters calls, and opens only a backed camera record', async () => {
    const user = userEvent.setup()
    const getEvents = vi.fn<EventApi['getEvents']>(async () => events)
    const onOpenArchive = vi.fn()
    render(<EventJournal
      api={{ getEvents }}
      flatId="flat-81"
      onOpenArchive={onOpenArchive}
      variant="full"
    />)

    expect(await screen.findByRole('heading', { name: 'Журнал событий' })).toBeVisible()
    expect(screen.getByText('Пропущенный вызов')).toBeVisible()
    expect(screen.getByText('Движение у двери')).toBeVisible()
    expect(screen.getAllByRole('button', { name: 'Открыть запись камеры' })).toHaveLength(1)
    await user.click(screen.getByRole('button', { name: 'Открыть запись камеры' }))
    expect(onOpenArchive).toHaveBeenCalledWith('camera / 1', '2026-08-07T09:30:00Z')

    await user.click(screen.getByRole('button', { name: 'Только звонки' }))
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
    render(<EventJournal api={{ getEvents }} flatId="flat-81" variant="calls" />)

    expect(await screen.findByText('Не удалось загрузить историю звонков')).toBeVisible()
    expect(document.body).not.toHaveTextContent('private upstream response')
    await user.click(screen.getByRole('button', { name: 'Повторить' }))
    expect(await screen.findByText('История звонков пока пуста')).toBeVisible()
  })
})
