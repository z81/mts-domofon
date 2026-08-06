import { useEffect, useRef, useState } from 'react'
import { Modal } from '../../app/Modal'
import {
  AuthApiError,
  authApi,
  type AuthApi,
  type AuthChallenge,
  type AuthProgress,
  type ChallengeInput,
  type SessionView,
} from './authApi'

type AuthView = 'restoring' | 'choice' | 'challenge' | 'import' | 'redirecting' | 'session' | 'consent_rejected' | 'rate_limited'

function navigateSameTab(url: string) {
  window.location.assign(url)
}

function directMtsLoginUrl(raw: string): string {
  const url = new URL(raw)
  if (url.protocol !== 'https:' || url.hostname !== 'login.mts.ru' ||
      (url.port !== '' && url.port !== '443') ||
      url.username !== '' || url.password !== '' ||
      url.pathname !== '/amserver/oauth2/authorize') {
    throw new Error('invalid MTS authorization URL')
  }
  return url.href
}

export function AuthPage({ api = authApi, navigate = navigateSameTab, onClose, onAuthenticated, onLoggedOut }: {
  api?: AuthApi
  navigate?: (url: string) => void
  onClose: () => void
  onAuthenticated?: (session: SessionView) => void
  onLoggedOut?: () => void
}) {
  const [view, setView] = useState<AuthView>('restoring')
  const [flowId, setFlowId] = useState<string | null>(null)
  const [challenge, setChallenge] = useState<AuthChallenge | null>(null)
  const [session, setSession] = useState<SessionView | null>(null)
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [countdown, setCountdown] = useState(0)
  const startRequestInFlight = useRef(false)
  const importRequestInFlight = useRef(false)
  const logoutRequestInFlight = useRef(false)

  useEffect(() => {
    const controller = new AbortController()
    api.getSession(controller.signal).then((restored) => {
      if (restored.authenticated) {
        setSession(restored)
        setView('session')
      } else {
        setView('choice')
      }
    }).catch((cause: unknown) => {
      if (!(cause instanceof DOMException && cause.name === 'AbortError')) setView('choice')
    })
    return () => controller.abort()
  }, [api])

  useEffect(() => {
    if (view !== 'rate_limited') return
    const timer = window.setInterval(() => setCountdown((value) => Math.max(0, value - 1)), 1000)
    return () => window.clearInterval(timer)
  }, [view])

  function handleFailure(cause: unknown, fallback: string) {
    if (cause instanceof AuthApiError && cause.envelope.code === 'rate_limited' && cause.envelope.retryAfterSeconds) {
      setCountdown(cause.envelope.retryAfterSeconds)
      setView('rate_limited')
      return
    }
    setError(fallback)
  }

  function applyProgress(progress: AuthProgress) {
    if (progress.status === 'browser_redirect') {
      const authorizationUrl = directMtsLoginUrl(progress.authorization_url)
      setView('redirecting')
      navigate(authorizationUrl)
      return
    }
    if (progress.status === 'challenge') {
      setFlowId(progress.flow_id)
      setChallenge(progress.challenge)
      setView('challenge')
      return
    }
    setFlowId(null)
    setChallenge(null)
    setSession(progress.session)
    setView('session')
    onAuthenticated?.(progress.session)
  }

  async function begin() {
    if (startRequestInFlight.current) return
    startRequestInFlight.current = true
    setBusy(true)
    setError(null)
    setSecret('')
    try {
      applyProgress(await api.startAuth())
    } catch (cause) {
      handleFailure(cause, 'Не удалось начать вход')
    } finally {
      startRequestInFlight.current = false
      setBusy(false)
    }
  }

  async function submit(input: ChallengeInput) {
    if (!flowId) return
    setSecret('')
    setBusy(true)
    setError(null)
    try {
      const progress = await api.submitChallenge(flowId, input)
      if (input.type === 'consent' && !input.accepted) {
        setChallenge(null)
        setView('consent_rejected')
      } else {
        applyProgress(progress)
      }
    } catch (cause) {
      handleFailure(cause, 'Не удалось продолжить вход')
    } finally {
      setBusy(false)
    }
  }

  async function importMtsResponse() {
    if (!secret || importRequestInFlight.current) return
    importRequestInFlight.current = true
    const response = secret
    setSecret('')
    setBusy(true)
    setError(null)
    try {
      applyProgress(await api.importWebSsoResponse(response))
    } catch {
      setError('Не удалось импортировать ответ MTS')
    } finally {
      importRequestInFlight.current = false
      setBusy(false)
    }
  }

  async function endSession() {
    if (logoutRequestInFlight.current) return
    logoutRequestInFlight.current = true
    setBusy(true)
    setError(null)
    let loggedOut = false
    try {
      await api.logout()
      setSession(null)
      setFlowId(null)
      setChallenge(null)
      setSecret('')
      setView('choice')
      loggedOut = true
    } catch {
      setError('Не удалось выйти')
    } finally {
      logoutRequestInFlight.current = false
      setBusy(false)
    }
    if (loggedOut) onLoggedOut?.()
  }

  const challengeForm = challenge && <>
    {challenge.type === 'phone' && <><span className="eyebrow">Номер телефона</span><h2 id="auth-title">Введите номер</h2><p>Номер отправляется только локальному VDome API.</p><label className="field-label">Номер<input autoComplete="tel" aria-label="Номер телефона" type="tel" value={secret} onChange={(event) => setSecret(event.target.value)} placeholder="+7 999 000-00-00" /></label><button className="primary-button" disabled={busy || secret.length < 5} onClick={() => submit({ type: 'phone', phone: secret })} type="button">Получить код</button></>}
    {challenge.type === 'sms_otp' && <><span className="eyebrow">Код из SMS</span><h2 id="auth-title">Подтверждение номера</h2><p>Новый код можно запросить через {challenge.resend_after_seconds} сек.</p><label className="field-label">Код из SMS<input autoComplete="one-time-code" aria-label="Код из SMS" inputMode="numeric" maxLength={6} value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || secret.length < 4} onClick={() => submit({ type: 'sms_otp', code: secret })} type="button">Подтвердить код</button></>}
    {challenge.type === 'captcha' && <><span className="eyebrow">Защита входа</span><h2 id="auth-title">Введите код с картинки</h2><img className="captcha-preview" src={challenge.image_data_url} alt="Проверочный рисунок" /><label className="field-label">Код с картинки<input autoComplete="off" aria-label="Код с картинки" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ type: 'captcha', answer: secret })} type="button">Продолжить</button></>}
    {challenge.type === 'password' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Введите пароль</h2><label className="field-label">Пароль<input autoComplete="current-password" aria-label="Пароль" type="password" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ type: 'password', password: secret })} type="button">Продолжить</button></>}
    {challenge.type === 'account_selection' && <><span className="eyebrow">Объект</span><h2 id="auth-title">Выберите аккаунт</h2><div className="stack-actions">{challenge.options.map((option) => <button className="secondary-button" onClick={() => submit({ type: 'account', id: option.id })} type="button" key={option.id}>{option.label}</button>)}</div></>}
    {challenge.type === 'consent' && <><span className="eyebrow">Согласие</span><h2 id="auth-title">{challenge.title}</h2><p>{challenge.body}</p><div className="stack-actions"><button className="primary-button" onClick={() => submit({ type: 'consent', accepted: true })} type="button">Разрешить</button><button className="secondary-button" onClick={() => submit({ type: 'consent', accepted: false })} type="button">Отклонить согласие</button></div></>}
  </>

  return <Modal className="auth-dialog" labelledBy="auth-title" onClose={onClose}>
    <button className="dialog-close" aria-label="Закрыть" onClick={onClose} type="button">×</button>
    {view === 'restoring' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Восстанавливаем сессию</h2><p aria-live="polite">Проверяем локальную сессию…</p></>}
    {view === 'choice' && <><span className="eyebrow">{api.isSyntheticPreview ? 'Режим предпросмотра' : 'Локальное подключение'}</span><h2 id="auth-title">Вход в VDome</h2><p>{api.isSyntheticPreview ? 'Используется синтетический сценарий без запросов к MTS.' : 'Продолжите вход на защищённой странице MTS ID.'}</p><div className="stack-actions"><button className="primary-button" data-dialog-initial disabled={busy} onClick={begin} type="button">Войти с MTS ID</button><button className="secondary-button" disabled={busy} onClick={() => { setError(null); setSecret(''); setView('import') }} type="button">Вставить ответ MTS</button></div></>}
    {view === 'challenge' && challengeForm}
    {view === 'import' && <><span className="eyebrow">Локальное подключение</span><h2 id="auth-title">Импорт ответа MTS</h2><p>Вставьте JSON в это локальное окно приложения, а не в чат.</p><label className="field-label">JSON от MTS<textarea aria-label="JSON от MTS" autoComplete="off" spellCheck={false} value={secret} onChange={(event) => setSecret(event.target.value)} /></label><div className="stack-actions"><button className="primary-button" disabled={busy || !secret} onClick={importMtsResponse} type="button">Подключить</button><button className="secondary-button" disabled={busy} onClick={() => { setError(null); setSecret(''); setView('choice') }} type="button">Назад</button></div></>}
    {view === 'redirecting' && <><span className="eyebrow">Защищённый вход</span><h2 id="auth-title">Переходим в MTS ID</h2><p aria-live="polite">Открываем защищённую страницу входа…</p></>}
    {view === 'session' && session && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Сессия восстановлена</h2><p>Статус: {session.status}. Секреты и токены не отображаются.</p><button className="secondary-button" disabled={busy} onClick={endSession} type="button">{busy ? 'Выходим…' : 'Выйти'}</button></>}
    {view === 'consent_rejected' && <><span className="eyebrow">Вход остановлен</span><h2 id="auth-title">Согласие отклонено</h2><p>Без согласия подключение домофона не выполняется.</p><button className="secondary-button" onClick={() => { setFlowId(null); setView('choice') }} type="button">Назад к способам входа</button></>}
    {view === 'rate_limited' && <><span className="eyebrow">Слишком много попыток</span><h2 id="auth-title">Вход временно ограничен</h2><p aria-live="polite">Повторите через {countdown} сек.</p><button className="secondary-button" disabled={countdown > 0} onClick={begin} type="button">Попробовать снова</button></>}
    {error && <p className="form-error" role="alert">{error}</p>}
  </Modal>
}
