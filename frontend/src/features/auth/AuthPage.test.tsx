import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { vi } from 'vitest'
import { AuthPage } from './AuthPage'
import { AuthApiError, createDefaultAuthApi, type AuthApi, type AuthChallenge, type AuthProgress, type SessionView } from './authApi'

const authenticated: AuthProgress = {
  status: 'authenticated',
  session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' },
}

function sequenceApi(challenges: readonly AuthChallenge[], session: SessionView = { authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null }): AuthApi {
  let index = 0
  return {
    getSession: async () => session,
    refreshSession: async () => session,
    importWebSsoResponse: async () => authenticated,
    startAuth: async () => ({ status: 'challenge', flow_id: 'memory-only-flow', challenge: challenges[0] }),
    submitChallenge: async () => {
      const next = challenges[++index]
      return next ? { status: 'challenge', flow_id: 'memory-only-flow', challenge: next } : authenticated
    },
    logout: async () => undefined,
  }
}

describe('AuthPage', () => {
  it('offers no production legacy SMS entry point', async () => {
    const api = sequenceApi([{ type: 'phone' }])
    render(<AuthPage api={api} onClose={() => undefined} />)

    await screen.findByRole('button', { name: 'Войти с MTS ID' })

    expect(screen.getByRole('heading', { name: 'Вход через MTS ID' })).toBeVisible()
    expect(document.body).not.toHaveTextContent(/VDome|локальн/i)
    expect(screen.queryByRole('button', { name: 'Войти по SMS' })).not.toBeInTheDocument()
  })

  it('navigates to the exact valid MTS authorization URL in the current tab', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    const authorizationUrl = 'https://login.mts.ru/amserver/oauth2/authorize?client_id=vdome&state=opaque'
    api.startAuth = async () => ({ status: 'browser_redirect', flow_id: 'browser / opaque', authorization_url: authorizationUrl })
    const navigate = vi.fn()
    render(<AuthPage api={api} navigate={navigate} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    expect(navigate).toHaveBeenCalledTimes(1)
    expect(navigate).toHaveBeenCalledWith(authorizationUrl)
    expect(screen.queryByRole('link')).not.toBeInTheDocument()
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
    expect(document.body).not.toHaveTextContent(/callback|chrome|открыть mts id/i)
  })

  it.each([
    'http://login.mts.ru/amserver/oauth2/authorize',
    'https://evil.example/amserver/oauth2/authorize',
    'https://token@login.mts.ru/amserver/oauth2/authorize',
    'https://login.mts.ru:444/amserver/oauth2/authorize',
    'https://login.mts.ru/amserver/UI/Login',
  ])('rejects an invalid MTS authorization URL without navigation', async (authorizationUrl) => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    api.startAuth = async () => ({ status: 'browser_redirect', flow_id: 'browser-flow', authorization_url: authorizationUrl })
    const navigate = vi.fn()
    render(<AuthPage api={api} navigate={navigate} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(navigate).not.toHaveBeenCalled()
    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось начать вход')
    expect(screen.getByRole('button', { name: 'Войти с MTS ID' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Вставить ответ MTS' })).toBeVisible()
    expect(screen.queryByRole('heading', { name: 'Переходим в MTS ID' })).not.toBeInTheDocument()
    expect(document.body).not.toHaveTextContent('provider-secret')
  })

  it('keeps MTS JSON import hidden until explicitly selected', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    render(<AuthPage api={api} onClose={() => undefined} />)

    await screen.findByRole('button', { name: 'Войти с MTS ID' })
    expect(screen.queryByRole('textbox', { name: 'JSON от MTS' })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Вставить ответ MTS' }))
    const response = screen.getByRole('textbox', { name: 'JSON от MTS' })
    expect(response).toHaveAttribute('autocomplete', 'off')
    expect(response).toHaveAttribute('spellcheck', 'false')
    expect(screen.getByRole('button', { name: 'Подключить' })).toBeDisabled()
  })

  it('clears imported MTS JSON before its request settles and authenticates once', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    let finishImport: ((progress: AuthProgress) => void) | undefined
    const importWebSsoResponse = vi.fn(() => new Promise<AuthProgress>((resolve) => { finishImport = resolve }))
    api.importWebSsoResponse = importWebSsoResponse
    const onAuthenticated = vi.fn()
    render(<AuthPage api={api} onAuthenticated={onAuthenticated} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Вставить ответ MTS' }))
    const response = screen.getByRole('textbox', { name: 'JSON от MTS' })
    await user.click(response)
    await user.paste('{"response":"erase-me"}')
    const submit = screen.getByRole('button', { name: 'Подключить' })
    await user.dblClick(submit)

    expect(importWebSsoResponse).toHaveBeenCalledTimes(1)
    expect(importWebSsoResponse).toHaveBeenCalledWith('{"response":"erase-me"}')
    expect(response).toHaveValue('')

    await act(async () => {
      finishImport?.(authenticated)
    })
    expect(onAuthenticated).toHaveBeenCalledWith(authenticated.session)
  })

  it('shows only a generic error when importing MTS JSON fails', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }])
    api.importWebSsoResponse = async () => { throw new Error('provider response: never-render-this-secret') }
    render(<AuthPage api={api} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Вставить ответ MTS' }))
    await user.click(screen.getByRole('textbox', { name: 'JSON от MTS' }))
    await user.paste('{"secret":"never-render-this-secret"}')
    await user.click(screen.getByRole('button', { name: 'Подключить' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось импортировать ответ MTS')
    expect(document.body).not.toHaveTextContent('never-render-this-secret')
  })

  it('keeps the explicit preview flow in-page and advances its first challenge', async () => {
    const user = userEvent.setup()
    const fetcher = vi.fn<typeof fetch>()
    const api = createDefaultAuthApi({ dev: true, syntheticPreviewEnabled: true, fetcher })
    const navigate = vi.fn()
    render(<AuthPage api={api} navigate={navigate} onClose={() => undefined} />)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    await user.type(screen.getByRole('textbox', { name: 'Номер телефона' }), '+7 999 000-00-00')
    await user.click(screen.getByRole('button', { name: 'Получить код' }))

    const otp = await screen.findByRole('textbox', { name: 'Код из SMS' })
    expect(otp).toHaveAttribute('autocomplete', 'one-time-code')
    expect(screen.queryByDisplayValue('+7 999 000-00-00')).not.toBeInTheDocument()
    expect(navigate).not.toHaveBeenCalled()
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('starts only one auth request while the first request is pending', async () => {
    const api = sequenceApi([{ type: 'phone' }])
    let finishStart: ((progress: AuthProgress) => void) | undefined
    const startAuth = vi.fn(() => new Promise<AuthProgress>((resolve) => { finishStart = resolve }))
    api.startAuth = startAuth
    render(<AuthPage api={api} onClose={() => undefined} />)

    const start = await screen.findByRole('button', { name: 'Войти с MTS ID' })
    act(() => {
      start.click()
      start.click()
    })

    expect(startAuth).toHaveBeenCalledTimes(1)

    await act(async () => {
      finishStart?.({ status: 'challenge', flow_id: 'single-flow', challenge: { type: 'phone' } })
    })
    expect(await screen.findByRole('textbox', { name: 'Номер телефона' })).toBeVisible()
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

  it('restores and logs out a session, and renders a rate-limit countdown', async () => {
    const user = userEvent.setup()
    const api = sequenceApi([{ type: 'phone' }], authenticated.session)
    api.startAuth = async () => { throw new AuthApiError({ code: 'rate_limited', message: 'Слишком много попыток', retryable: true, requestId: 'e7411f58-d5e9-4a43-b25d-88ddf2014713', retryAfterSeconds: 2 }) }
    render(<AuthPage api={api} onClose={() => undefined} />)

    expect(await screen.findByRole('heading', { name: 'Вы вошли в MTS ID' })).toBeVisible()
    expect(document.body).not.toHaveTextContent(/active|Статус/)
    await user.click(screen.getByRole('button', { name: 'Выйти' }))
    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByText(/Повторите через 2 сек/)).toBeVisible()
  })
})
