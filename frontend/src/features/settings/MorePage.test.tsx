import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { CallApi, CurrentCall } from '../calls/callApi'
import { MorePage } from './MorePage'

const call: CurrentCall = {
  id: 'call-1', revision: 1, phase: 'ringing', callerName: 'Домофон',
  startedAt: '2026-08-07T10:00:00Z', remoteSdp: 'v=0', canOpen: false,
}

function callsApi(): CallApi {
  return {
    answer: async () => call,
    createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
    createOpenIntent: async () => ({ intentId: 'call-intent', expiresAt: '2026-08-07T10:00:30Z' }),
    decline: async () => undefined,
    disableAutoOpen: async () => undefined,
    executeAutoOpenIntent: async () => ({ mode: 'disabled', dismissAfterOpen: false }),
    executeOpenIntent: async () => undefined,
    getAutoOpenPolicy: async () => ({ mode: 'disabled', dismissAfterOpen: false }),
    getCurrent: async () => null,
    hangup: async () => undefined,
    ignore: async () => undefined,
  }
}

describe('MorePage', () => {
  it('limits settings to connection and safe auto-open controls', async () => {
    const onOpenAuth = vi.fn()
    const onPolicyChange = vi.fn()
    const user = userEvent.setup()
    render(<MorePage callsApi={callsApi()} onOpenAuth={onOpenAuth} onPolicyChange={onPolicyChange} />)

    expect(screen.getByRole('heading', { name: 'Ещё' })).toBeVisible()
    const connectionButton = screen.getByRole('button', { name: 'MTS ID и подключение' })
    expect(connectionButton).toBeVisible()
    expect(await screen.findByRole('heading', { name: /Автооткрытие/ })).toBeVisible()
    expect(screen.queryByText(/FAQ|Документы|Тема|Избранное/)).not.toBeInTheDocument()

    await user.click(connectionButton)
    expect(onOpenAuth).toHaveBeenCalledTimes(1)
  })

  it('shows a stable connection issue without exposing raw provider details', () => {
    render(<MorePage
      callsApi={callsApi()}
      connectionIssue="Не удалось проверить подключение"
      onOpenAuth={() => undefined}
      onPolicyChange={() => undefined}
    />)

    expect(screen.getByRole('status')).toHaveTextContent('Не удалось проверить подключение')
  })
})
