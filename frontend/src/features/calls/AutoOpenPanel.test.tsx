import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { AutoOpenPanel } from './AutoOpenPanel'
import type { AutoOpenPolicy, CallApi, CurrentCall } from './callApi'

const call: CurrentCall = {
  id: 'call-1', revision: 1, phase: 'ringing', callerName: 'Домофон',
  startedAt: '2026-08-07T10:00:00Z', remoteSdp: 'v=0', canOpen: false,
}

function api(overrides: Partial<CallApi>): CallApi {
  return {
    answer: async () => call,
    createOpenIntent: async () => ({ intentId: 'call-intent', expiresAt: '2026-08-07T10:00:30Z' }),
    decline: async () => undefined,
    executeOpenIntent: async () => undefined,
    getCurrent: async () => null,
    hangup: async () => undefined,
    ignore: async () => undefined,
    ...overrides,
  }
}

const disabled: AutoOpenPolicy = {
  mode: 'disabled', dismissAfterOpen: false,
}

describe('auto-open panel', () => {
  it('requires final confirmation to arm and disables immediately', async () => {
    const user = userEvent.setup()
    const createAutoOpenIntent = vi.fn(async () => ({
      intentId: 'auto / intent', expiresAt: new Date(Date.now() + 60_000).toISOString(),
    }))
    const armed: AutoOpenPolicy = { mode: 'next_call', dismissAfterOpen: true }
    const executeAutoOpenIntent = vi.fn(async () => armed)
    const disableAutoOpen = vi.fn(async () => undefined)
    const onPolicyChange = vi.fn()
    render(<AutoOpenPanel
      api={api({
        createAutoOpenIntent,
        disableAutoOpen,
        executeAutoOpenIntent,
        getAutoOpenPolicy: async () => disabled,
      })}
      onPolicyChange={onPolicyChange}
    />)

    expect(await screen.findByText('Автооткрытие выключено')).toBeVisible()
    await user.click(screen.getByRole('checkbox', { name: 'Сбрасывать звонок после открытия' }))
    await user.click(screen.getByRole('button', { name: 'Открыть следующий звонок' }))
    expect(createAutoOpenIntent).toHaveBeenCalledWith('next_call', true, expect.any(AbortSignal))
    expect(executeAutoOpenIntent).not.toHaveBeenCalled()

    const dialog = await screen.findByRole('dialog', { name: 'Включить автооткрытие?' })
    await user.click(within(dialog).getByRole('button', { name: 'Подтвердить включение' }))
    expect(executeAutoOpenIntent).toHaveBeenCalledWith('auto / intent', expect.any(AbortSignal))
    expect(await screen.findByText('Откроется следующий входящий звонок')).toBeVisible()
    expect(onPolicyChange).toHaveBeenLastCalledWith(armed)

    await user.click(screen.getByRole('button', { name: 'Выключить автооткрытие' }))
    expect(disableAutoOpen).toHaveBeenCalledTimes(1)
    expect(await screen.findByText('Автооткрытие выключено')).toBeVisible()
  })

  it('does not offer arming while the server policy is unknown', async () => {
    render(<AutoOpenPanel api={api({
      createAutoOpenIntent: vi.fn(),
      disableAutoOpen: vi.fn(),
      executeAutoOpenIntent: vi.fn(),
      getAutoOpenPolicy: async () => { throw new Error('unavailable') },
    })} />)

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось обновить автооткрытие')
    expect(screen.queryByRole('button', { name: 'Открыть следующий звонок' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Включить на 30 минут' })).not.toBeInTheDocument()
  })
})
