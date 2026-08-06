import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { AuthPage } from './AuthPage'
import { AuthApiError, type AuthApi, type AuthChallenge, type SessionView } from './authApi'

function sequenceApi(challenges: readonly AuthChallenge[], session: SessionView = { authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null }): AuthApi {
  let index = 0
  return {
    completeCallback: async () => ({ status: 'authenticated', session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' } }),
    getSession: async () => session,
    startAuth: async () => ({ status: 'challenge', flow_id: 'memory-only-flow', challenge: challenges[0] }),
    submitChallenge: async () => {
      const next = challenges[++index]
      return next
        ? { status: 'challenge', flow_id: 'memory-only-flow', challenge: next }
        : { status: 'authenticated', session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' } }
    },
    logout: async () => undefined,
  }
}

describe('AuthPage', () => {
  it('advances from phone to OTP and clears the submitted phone secret', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }, { type: 'sms_otp', resend_after_seconds: 30 }])
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    await user.type(screen.getByRole('textbox', { name: 'Номер телефона' }), '+7 999 000-00-00')
    await user.click(screen.getByRole('button', { name: 'Получить код' }))

    const otp = await screen.findByRole('textbox', { name: 'Код из SMS' })
    expect(otp).toHaveAttribute('autocomplete', 'one-time-code')
    expect(screen.queryByDisplayValue('+7 999 000-00-00')).not.toBeInTheDocument()
  })

  it('renders captcha, current password, account selection and consent rejection', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([
      { type: 'captcha', image_data_url: 'data:image/png;base64,c3ludGhldGlj' },
      { type: 'password' },
      { type: 'account_selection', options: [{ id: 'flat-1', label: 'Квартира 81' }] },
      { type: 'consent', title: 'Доступ к домофону', body: 'Разрешить доступ к домофону' },
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
    const api = sequenceApi([{ type: 'phone' }], { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' })
    api.startAuth = async () => { throw new AuthApiError({ code: 'rate_limited', message: 'Слишком много попыток', retryable: true, requestId: 'e7411f58-d5e9-4a43-b25d-88ddf2014713', retryAfterSeconds: 2 }) }
    render(<AuthPage api={api} onClose={() => undefined} />)

    expect(await screen.findByRole('heading', { name: 'Сессия восстановлена' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Выйти' }))
    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByText(/Повторите через 2 сек/)).toBeVisible()
  })

  it('clears a password immediately when challenge submission fails', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'password' }])
    api.submitChallenge = async () => { throw new Error('synthetic failure') }
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    const password = screen.getByLabelText('Пароль')
    await user.type(password, 'erase-me')
    await user.click(screen.getByRole('button', { name: 'Продолжить' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось продолжить вход')
    expect(password).toHaveValue('')
  })

  it('renders only an approved HTTPS login.mts.ru browser redirect', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    api.startAuth = async () => ({ status: 'browser_redirect', flow_id: 'browser-flow', authorization_url: 'https://login.mts.ru/amserver/UI/Login?realm=users' })
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByRole('link', { name: 'Продолжить в MTS ID' })).toHaveAttribute('href', 'https://login.mts.ru/amserver/UI/Login?realm=users')
    expect(screen.getByRole('textbox', { name: 'Адрес страницы после входа' })).toHaveAttribute('autocomplete', 'off')
    expect(screen.getByRole('textbox', { name: 'Адрес страницы после входа' })).toHaveAttribute('spellcheck', 'false')
  })

  it('clears the callback before the request settles and never renders callback secrets on failure', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    api.startAuth = async () => ({ status: 'browser_redirect', flow_id: 'browser-flow', authorization_url: 'https://login.mts.ru/amserver/UI/Login?realm=users' })
    let rejectCallback: ((cause: unknown) => void) | undefined
    api.completeCallback = async () => new Promise((_, reject) => { rejectCallback = reject })
    const callbackSecret = 'https://gateway.vdome.mts.ru/user-service/api/auth/callback?code=never-render-this&state=state-secret'
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    const input = screen.getByRole('textbox', { name: 'Адрес страницы после входа' })
    await user.type(input, callbackSecret)
    await user.click(screen.getByRole('button', { name: 'Завершить вход' }))

    expect(input).toHaveValue('')
    rejectCallback?.(new Error(callbackSecret))
    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось завершить вход')
    expect(document.body).not.toHaveTextContent('never-render-this')
    expect(input).toHaveValue('')
  })

  it('rejects an untrusted browser redirect URL', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    api.startAuth = async () => ({ status: 'browser_redirect', flow_id: 'browser-flow', authorization_url: 'https://evil.example/steal' })
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Недопустимый адрес MTS ID')
    expect(screen.queryByRole('link', { name: 'Продолжить в MTS ID' })).not.toBeInTheDocument()
  })
})
