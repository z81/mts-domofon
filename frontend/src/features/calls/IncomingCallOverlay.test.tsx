import { fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { IncomingCallOverlay } from './IncomingCallOverlay'
import type { AutoOpenResult, CallApi, CurrentCall } from './callApi'

const ringingCall: CurrentCall = {
  id: 'call-81',
  revision: 3,
  phase: 'ringing',
  callerName: 'Домофон у подъезда',
  startedAt: '2026-08-06T12:00:00Z',
  remoteSdp: 'v=0\r\na=ice-ufrag:remote\r\n',
  canOpen: true,
}

const activeCall: CurrentCall = { ...ringingCall, phase: 'active', revision: 5 }
const endingCall: CurrentCall = { ...ringingCall, phase: 'ending', revision: 6 }

const api: CallApi = {
  answer: async () => ({ ...ringingCall, phase: 'connecting', revision: 4 }),
  createOpenIntent: async () => ({ intentId: 'intent-1', expiresAt: new Date(Date.now() + 60_000).toISOString() }),
  decline: async () => undefined,
  executeOpenIntent: async () => undefined,
  getCurrent: async () => null,
  hangup: async () => undefined,
  ignore: async () => undefined,
}

afterEach(() => {
  document.querySelector('[data-test-return-focus]')?.remove()
})

describe('incoming call overlay', () => {
  it('shows the owned call as a dialog without page navigation or a redundant page heading', () => {
    render(<IncomingCallOverlay api={api} call={ringingCall} />)

    expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Принять вызов' })).toBeVisible()
    expect(screen.queryByRole('heading', { name: 'Звонок' })).not.toBeInTheDocument()
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument()
  })

  it('ignores Escape and backdrop dismissal while trapping and restoring focus', async () => {
    const user = userEvent.setup()
    const returnFocus = document.createElement('button')
    returnFocus.dataset.testReturnFocus = 'true'
    document.body.append(returnFocus)
    returnFocus.focus()
    const view = render(<IncomingCallOverlay api={api} call={ringingCall} />)
    const dialog = screen.getByRole('dialog', { name: 'Входящий вызов' })
    const first = screen.getByRole('button', { name: 'Принять вызов' })
    const last = screen.getByRole('button', { name: 'Открыть дверь' })

    expect(first).toHaveFocus()
    last.focus()
    await user.tab()
    expect(first).toHaveFocus()
    await user.tab({ shift: true })
    expect(last).toHaveFocus()

    const escape = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' })
    document.dispatchEvent(escape)
    fireEvent.mouseDown(dialog.parentElement as HTMLElement)

    expect(escape.defaultPrevented).toBe(false)
    expect(dialog).toBeVisible()
    view.unmount()
    expect(returnFocus).toHaveFocus()
  })

  it('traps focus in the inner exact call-door confirmation and lets Escape close only it', async () => {
    const user = userEvent.setup()
    render(<IncomingCallOverlay api={api} call={ringingCall} />)
    const outer = screen.getByRole('dialog', { name: 'Входящий вызов' })
    const open = within(outer).getByRole('button', { name: 'Открыть дверь' })

    await user.click(open)
    const inner = await screen.findByRole('dialog', { name: 'Открыть дверь во время звонка?' })
    expect(within(inner).getByText('Домофон у подъезда')).toBeVisible()
    const first = within(inner).getByRole('button', { name: 'Закрыть' })
    const last = within(inner).getByRole('button', { name: 'Подтвердить открытие' })
    last.focus()
    await user.tab()
    expect(first).toHaveFocus()
    await user.tab({ shift: true })
    expect(last).toHaveFocus()
    await user.keyboard('{Escape}')

    expect(screen.queryByRole('dialog', { name: 'Открыть дверь во время звонка?' })).not.toBeInTheDocument()
    expect(outer).toBeVisible()
    expect(open).toHaveFocus()
  })

  it('focuses the dialog fallback when an ending call initially has no enabled actions', async () => {
    const user = userEvent.setup()
    render(<IncomingCallOverlay api={api} call={endingCall} />)
    const dialog = screen.getByRole('dialog', { name: 'Входящий вызов' })

    expect(dialog).toHaveFocus()
    await user.tab()
    expect(dialog).toHaveFocus()
    await user.tab({ shift: true })
    expect(dialog).toHaveFocus()
  })

  it('returns focus to the dialog fallback when an active call enters ending', async () => {
    const user = userEvent.setup()
    const view = render(<>
      <button type="button">Действие назначения</button>
      <IncomingCallOverlay api={api} call={activeCall} />
    </>)
    const dialog = screen.getByRole('dialog', { name: 'Входящий вызов' })
    expect(screen.getByRole('button', { name: 'Завершить вызов' })).toHaveFocus()

    view.rerender(<>
      <button type="button">Действие назначения</button>
      <IncomingCallOverlay api={api} call={endingCall} />
    </>)

    expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBe(dialog)
    expect(dialog).toHaveFocus()
    await user.tab()
    expect(dialog).toHaveFocus()
  })

  it('does not invent an overlay without a current call', () => {
    render(<IncomingCallOverlay api={api} call={null} loading pollingError />)

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(screen.queryByText('Нет активного звонка')).not.toBeInTheDocument()
  })

  it.each([
    ['opened', 'status', 'Дверь открыта автоматически'],
    ['failed', 'alert', 'Автооткрытие не сработало. Автоматического повтора не будет.'],
    ['outcome_unknown', 'alert', 'Результат автооткрытия неизвестен. Автоматического повтора не будет.'],
  ] as const)('renders the %s auto-open result with %s semantics', (status, role, message) => {
    const autoOpenResult: AutoOpenResult = { status, completedAt: '2026-08-06T12:00:03Z' }

    render(<IncomingCallOverlay api={api} autoOpenResult={autoOpenResult} call={ringingCall} />)

    expect(within(screen.getByRole('dialog', { name: 'Входящий вызов' })).getByText(message)).toHaveAttribute('role', role)
  })
})
