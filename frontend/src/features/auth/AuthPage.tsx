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

type AuthView = 'restoring' | 'choice' | 'challenge' | 'redirect' | 'legacy_sms_consent' | 'legacy_sms_otp' | 'session' | 'consent_rejected' | 'rate_limited'

function normalizeRussianPhone(value: string): string | null {
  const digits = value.replace(/\D/g, '')
  const normalizedDigits = digits.length === 10
    ? `7${digits}`
    : digits.startsWith('8') && digits.length === 11
      ? `7${digits.slice(1)}`
      : digits
  return /^7\d{10}$/.test(normalizedDigits) ? `+${normalizedDigits}` : null
}

function approvedMtsLoginUrl(rawUrl: string): string | null {
  try {
    const url = new URL(rawUrl)
    if (url.protocol !== 'https:' || url.hostname !== 'login.mts.ru' || url.username || url.password) return null
    return url.toString()
  } catch {
    return null
  }
}

export function AuthPage({ api = authApi, onClose, onAuthenticated, onLoggedOut }: {
  api?: AuthApi
  onClose: () => void
  onAuthenticated?: (session: SessionView) => void
  onLoggedOut?: () => void
}) {
  const [view, setView] = useState<AuthView>('restoring')
  const [flowId, setFlowId] = useState<string | null>(null)
  const [challenge, setChallenge] = useState<AuthChallenge | null>(null)
  const [session, setSession] = useState<SessionView | null>(null)
  const [authorizationUrl, setAuthorizationUrl] = useState<string | null>(null)
  const [callbackUrl, setCallbackUrl] = useState('')
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [countdown, setCountdown] = useState(0)
  const [legacyPhone, setLegacyPhone] = useState('')
  const [legacyConsent, setLegacyConsent] = useState(false)
  const [legacyResendCountdown, setLegacyResendCountdown] = useState(0)
  const legacyRequestInFlight = useRef(false)
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

  useEffect(() => {
    if (view !== 'legacy_sms_otp' || legacyResendCountdown <= 0) return
    const timer = window.setTimeout(() => setLegacyResendCountdown((value) => Math.max(0, value - 1)), 1000)
    return () => window.clearTimeout(timer)
  }, [legacyResendCountdown, view])

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
      setFlowId(progress.flow_id)
      setCallbackUrl('')
      const approved = approvedMtsLoginUrl(progress.authorization_url)
      if (!approved) {
        setAuthorizationUrl(null)
        setError('Недопустимый адрес MTS ID')
        setView('choice')
        return
      }
      setAuthorizationUrl(approved)
      setView('redirect')
      return
    }
    if (progress.status === 'challenge') {
      setFlowId(progress.flow_id)
      setCallbackUrl('')
      setChallenge(progress.challenge)
      setView('challenge')
      return
    }
    setFlowId(null)
    setCallbackUrl('')
    setChallenge(null)
    setSession(progress.session)
    setView('session')
    onAuthenticated?.(progress.session)
  }

  async function begin() {
    setBusy(true)
    setError(null)
    setSecret('')
    setCallbackUrl('')
    try {
      applyProgress(await api.startAuth())
    } catch (cause) {
      handleFailure(cause, 'Не удалось начать вход')
    } finally {
      setBusy(false)
    }
  }

  function chooseLegacySms() {
    setError(null)
    setFlowId(null)
    setSecret('')
    setLegacyPhone('')
    setLegacyConsent(false)
    setLegacyResendCountdown(0)
    setView('legacy_sms_consent')
  }

  async function sendLegacySms() {
    const phone = normalizeRussianPhone(legacyPhone)
    if (!phone || !legacyConsent || legacyRequestInFlight.current) return
    legacyRequestInFlight.current = true
    setBusy(true)
    setError(null)
    try {
      const progress = await api.startLegacySms({ phone, accept_legacy_sms: true })
      if (progress.status !== 'challenge' || progress.challenge.type !== 'sms_otp') {
        throw new Error('Unexpected legacy SMS response')
      }
      setFlowId(progress.flow_id)
      setSecret('')
      setLegacyResendCountdown(Math.max(0, progress.challenge.resend_after_seconds))
      setView('legacy_sms_otp')
    } catch (cause) {
      if (cause instanceof AuthApiError && cause.envelope.code === 'rate_limited' && cause.envelope.retryAfterSeconds) {
        setLegacyResendCountdown(Math.max(legacyResendCountdown, cause.envelope.retryAfterSeconds))
        setError(`Слишком много попыток. Повторите через ${cause.envelope.retryAfterSeconds} сек.`)
      } else {
        setError('Не удалось отправить код')
      }
    } finally {
      legacyRequestInFlight.current = false
      setBusy(false)
    }
  }

  async function confirmLegacySms() {
    if (!flowId || !/^\d{6}$/.test(secret) || legacyRequestInFlight.current) return
    const code = secret
    setSecret('')
    legacyRequestInFlight.current = true
    setBusy(true)
    setError(null)
    try {
      const progress = await api.confirmLegacySms(flowId, code)
      if (progress.status !== 'authenticated') throw new Error('Unexpected legacy SMS response')
      applyProgress(progress)
    } catch (cause) {
      if (cause instanceof AuthApiError && cause.envelope.code === 'rate_limited' && cause.envelope.retryAfterSeconds) {
        setLegacyResendCountdown(Math.max(legacyResendCountdown, cause.envelope.retryAfterSeconds))
      }
      setError('Не удалось подтвердить код')
    } finally {
      legacyRequestInFlight.current = false
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

  async function completeRedirect() {
    if (!flowId || !callbackUrl) return
    const submittedCallback = callbackUrl
    setCallbackUrl('')
    setBusy(true)
    setError(null)
    try {
      applyProgress(await api.completeCallback(flowId, submittedCallback))
    } catch (cause) {
      setCallbackUrl('')
      handleFailure(cause, 'Не удалось завершить вход')
    } finally {
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
      setAuthorizationUrl(null)
      setCallbackUrl('')
      setSecret('')
      setLegacyPhone('')
      setLegacyConsent(false)
      setLegacyResendCountdown(0)
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

  const normalizedLegacyPhone = normalizeRussianPhone(legacyPhone)

  return <Modal className="auth-dialog" labelledBy="auth-title" onClose={onClose}>
    <button className="dialog-close" aria-label="Закрыть" onClick={onClose} type="button">×</button>
    {view === 'restoring' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Восстанавливаем сессию</h2><p aria-live="polite">Проверяем локальную сессию…</p></>}
    {view === 'choice' && <><span className="eyebrow">{api.isSyntheticPreview ? 'Режим предпросмотра' : 'Локальное подключение'}</span><h2 id="auth-title">Вход в VDome</h2><p>{api.isSyntheticPreview ? 'Используется синтетический сценарий без запросов к MTS.' : 'Авторизация проходит на официальной странице MTS ID; callback обрабатывает только локальный VDome API.'}</p><div className="stack-actions"><button className="primary-button" data-dialog-initial disabled={busy} onClick={begin} type="button">Войти с MTS ID</button>{!api.isSyntheticPreview && <button className="secondary-button" disabled={busy} onClick={chooseLegacySms} type="button">Войти по SMS</button>}</div></>}
    {view === 'challenge' && challengeForm}
    {view === 'legacy_sms_consent' && <><span className="eyebrow">Режим совместимости</span><h2 id="auth-title">Вход по SMS</h2><p>Этот способ использует устаревший SMS-вход MTS. Код будет отправлен только после вашего явного согласия.</p><label className="field-label">Номер телефона<input autoComplete="tel" aria-label="Номер телефона для SMS" type="tel" value={legacyPhone} onChange={(event) => setLegacyPhone(event.target.value)} placeholder="+7 999 000-00-00" /></label>{legacyPhone && !normalizedLegacyPhone && <p className="field-hint" role="status">Введите российский номер из 11 цифр</p>}<label className="compatibility-consent"><input checked={legacyConsent} onChange={(event) => setLegacyConsent(event.target.checked)} type="checkbox" />Я согласен использовать режим совместимости для входа по SMS</label><div className="stack-actions"><button className="primary-button" disabled={busy || !legacyConsent || !normalizedLegacyPhone} onClick={sendLegacySms} type="button">{busy ? 'Отправляем…' : 'Получить код по SMS'}</button><button className="secondary-button" disabled={busy} onClick={() => setView('choice')} type="button">Назад</button></div></>}
    {view === 'legacy_sms_otp' && <><span className="eyebrow">Режим совместимости</span><h2 id="auth-title">Введите код из SMS</h2><p>{legacyResendCountdown > 0 ? `Повторная отправка будет доступна через ${legacyResendCountdown} сек.` : 'Код можно отправить повторно вручную.'}</p><label className="field-label">Код из SMS<input autoComplete="one-time-code" aria-label="Шестизначный код из SMS" inputMode="numeric" maxLength={6} value={secret} onChange={(event) => setSecret(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><div className="stack-actions"><button className="primary-button" disabled={busy || !/^\d{6}$/.test(secret)} onClick={confirmLegacySms} type="button">{busy ? 'Проверяем…' : 'Подтвердить код'}</button><button className="secondary-button" disabled={busy || legacyResendCountdown > 0} onClick={sendLegacySms} type="button">Отправить код повторно</button></div></>}
    {view === 'redirect' && authorizationUrl && <><span className="eyebrow">Защищённый вход</span><h2 id="auth-title">Продолжите в MTS ID</h2><p>Откройте официальный login.mts.ru. После входа скопируйте полный адрес открывшейся callback-страницы gateway.vdome.mts.ru и вставьте ниже.</p><div className="stack-actions"><a className="primary-button auth-link" href={authorizationUrl} target="_blank" rel="noopener noreferrer">Продолжить в MTS ID</a></div><label className="field-label">Адрес страницы после входа<input aria-label="Адрес страницы после входа" autoComplete="off" inputMode="url" maxLength={8192} spellCheck={false} value={callbackUrl} onChange={(event) => setCallbackUrl(event.target.value)} /></label><button className="secondary-button" disabled={busy || !callbackUrl} onClick={completeRedirect} type="button">Завершить вход</button></>}
    {view === 'session' && session && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Сессия восстановлена</h2><p>Статус: {session.status}. Секреты и токены не отображаются.</p><button className="secondary-button" disabled={busy} onClick={endSession} type="button">{busy ? 'Выходим…' : 'Выйти'}</button></>}
    {view === 'consent_rejected' && <><span className="eyebrow">Вход остановлен</span><h2 id="auth-title">Согласие отклонено</h2><p>Без согласия подключение домофона не выполняется.</p><button className="secondary-button" onClick={() => { setFlowId(null); setView('choice') }} type="button">Назад к способам входа</button></>}
    {view === 'rate_limited' && <><span className="eyebrow">Слишком много попыток</span><h2 id="auth-title">Вход временно ограничен</h2><p aria-live="polite">Повторите через {countdown} сек.</p><button className="secondary-button" disabled={countdown > 0} onClick={begin} type="button">Попробовать снова</button></>}
    {error && <p className="form-error" role="alert">{error}</p>}
  </Modal>
}
