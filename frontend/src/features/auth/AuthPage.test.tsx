import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { AuthPage } from './AuthPage'
import type { AuthApi, AuthChallenge, AuthSession } from './authApi'

function sequenceApi(challenges: readonly AuthChallenge[], session: AuthSession | null = null): AuthApi {
  let index = 0
  return {
    getSession: async () => session,
    startAuth: async () => ({ flowId: 'memory-only-flow', challenge: challenges[0] }),
    submitChallenge: async () => ({ flowId: 'memory-only-flow', challenge: challenges[++index] }),
    logout: async () => undefined,
  }
}

describe('AuthPage', () => {
  it('advances from phone to OTP and clears the submitted phone secret', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ kind: 'phone' }, { kind: 'otp', destination: '+7 ••• •••-00-00' }])
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти по номеру телефона' }))
    await user.type(screen.getByRole('textbox', { name: 'Номер телефона' }), '+7 999 000-00-00')
    await user.click(screen.getByRole('button', { name: 'Получить код' }))

    const otp = await screen.findByRole('textbox', { name: 'Код из SMS' })
    expect(otp).toHaveAttribute('autocomplete', 'one-time-code')
    expect(screen.queryByDisplayValue('+7 999 000-00-00')).not.toBeInTheDocument()
  })

  it('renders captcha, current password, account selection and consent rejection', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([
      { kind: 'captcha', imageAlt: 'Проверочный рисунок' },
      { kind: 'password' },
      { kind: 'account_selection', accounts: [{ id: 'flat-1', label: 'Квартира 81' }] },
      { kind: 'consent', text: 'Разрешить доступ к домофону' },
      { kind: 'consent_rejected' },
    ])
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    await user.type(screen.getByRole('textbox', { name: 'Код с картинки' }), 'abcd')
    await user.click(screen.getByRole('button', { name: 'Продолжить' }))
    const password = await screen.findByLabelText('Пароль')
    expect(password).toHaveAttribute('autocomplete', 'current-password')
    await user.type(password, 'not-a-real-secret')
    await user.click(screen.getByRole('button', { name: 'Продолжить' }))
    await user.click(await screen.findByRole('button', { name: 'Квартира 81' }))
    await user.click(await screen.findByRole('button', { name: 'Отклонить согласие' }))

    expect(await screen.findByRole('heading', { name: 'Согласие отклонено' })).toBeVisible()
    expect(screen.queryByDisplayValue('not-a-real-secret')).not.toBeInTheDocument()
  })

  it('restores and logs out a session, and renders a rate-limit countdown', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ kind: 'rate_limited', retryAfterSeconds: 2 }], { userId: 'demo', displayName: 'Мария' })
    render(<AuthPage api={api} onClose={() => undefined} />)

    expect(await screen.findByRole('heading', { name: 'Сессия восстановлена' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Выйти' }))
    await user.click(await screen.findByRole('button', { name: 'Войти по номеру телефона' }))

    expect(await screen.findByText(/Повторите через 2 сек/)).toBeVisible()
  })

  it('clears a password immediately when challenge submission fails', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ kind: 'password' }])
    api.submitChallenge = async () => { throw new Error('synthetic failure') }
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    const password = screen.getByLabelText('Пароль')
    await user.type(password, 'erase-me')
    await user.click(screen.getByRole('button', { name: 'Продолжить' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось продолжить вход')
    expect(password).toHaveValue('')
  })
})
