import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { HomePage } from './HomePage'

describe('HomePage', () => {
  it('orders camera, doors, auto-open, and recent activity', () => {
    render(<HomePage
      activity={<div>События из API</div>}
      autoOpen={<div role="status">Автооткрытие из API</div>}
      camera={<div>Камера из API</div>}
      doors={<div>Двери из API</div>}
      onOpenAllEvents={() => undefined}
    />)

    expect(screen.getByRole('heading', { name: 'Главная' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Камеры' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Двери' })).toBeVisible()
    expect(screen.getByText('Камера из API')).toBeVisible()
    expect(screen.getByText('Двери из API')).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Автооткрытие из API')
    expect(screen.getByText('События из API')).toBeVisible()
    const cameraSection = screen.getByRole('heading', { name: 'Камеры' }).closest('section')!
    const doorSection = screen.getByRole('heading', { name: 'Двери' }).closest('section')!
    const autoOpenStatus = screen.getByRole('status')
    const activitySection = screen.getByRole('heading', { name: 'Последние события' }).closest('section')!
    expect(cameraSection.compareDocumentPosition(doorSection) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(doorSection.compareDocumentPosition(autoOpenStatus) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(autoOpenStatus.compareDocumentPosition(activitySection) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('renders armed status in place and opens the complete event destination', async () => {
    const user = userEvent.setup()
    const onOpenAllEvents = vi.fn()
    render(<HomePage
      activity={<div>События из API</div>}
      autoOpen={<div role="status">Следующий звонок</div>}
      camera={<div>Камера из API</div>}
      doors={<div>Двери из API</div>}
      onOpenAllEvents={onOpenAllEvents}
    />)

    expect(screen.getByRole('status')).toHaveTextContent('Следующий звонок')
    await user.click(screen.getByRole('button', { name: 'Все события' }))
    expect(onOpenAllEvents).toHaveBeenCalledTimes(1)
  })
})
