import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { AppShell } from './AppShell'

describe('AppShell', () => {
  it('renders only the three resident destinations', () => {
    render(<AppShell
      destination="home"
      header={<span>Квартира 81</span>}
      onNavigate={() => undefined}
    ><p>Контент</p></AppShell>)

    const destinationButtons = screen.getAllByRole('button').filter((button) => (
      ['Главная', 'События', 'Ещё'].includes(button.textContent ?? '')
    ))
    expect(destinationButtons).toHaveLength(3)
    expect(screen.queryByText('Доступ')).not.toBeInTheDocument()
    expect(screen.queryByText('Звонки')).not.toBeInTheDocument()
  })

  it('marks the current destination and reports destination changes', async () => {
    const user = userEvent.setup()
    const onNavigate = vi.fn()
    render(<AppShell
      destination="events"
      header={<span>Квартира 81</span>}
      onNavigate={onNavigate}
      status={<p>Автооткрытие включено</p>}
    ><p>Контент</p></AppShell>)

    expect(screen.getByRole('button', { name: 'События' })).toHaveAttribute('aria-current', 'page')
    expect(screen.getByText('Автооткрытие включено')).toBeVisible()
    expect(screen.getByText('Контент')).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Ещё' }))
    expect(onNavigate).toHaveBeenCalledWith('more')
  })
})
