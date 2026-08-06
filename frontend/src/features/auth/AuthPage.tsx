import { useEffect, useState } from 'react'
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

type AuthView = 'restoring' | 'choice' | 'challenge' | 'redirect' | 'session' | 'consent_rejected' | 'rate_limited'

function approvedMtsLoginUrl(rawUrl: string): string | null {
  try {
    const url = new URL(rawUrl)
    if (url.protocol !== 'https:' || url.hostname !== 'login.mts.ru' || url.username || url.password) return null
    return url.toString()
  } catch {
    return null
  }
}

export function AuthPage({ api = authApi, onClose, onAuthenticated }: {
  api?: AuthApi
  onClose: () => void
  onAuthenticated?: (session: SessionView) => void
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
    await api.logout()
    setSession(null)
    setFlowId(null)
    setChallenge(null)
    setAuthorizationUrl(null)
    setCallbackUrl('')
    setSecret('')
    setError(null)
    setView('choice')
  }

  const challengeForm = challenge && <>
    {challenge.type === 'phone' && <><span className="eyebrow">Номер телефона</span><h2 id="auth-title">Введите номер</h2><p>Номер отправляется только локальному VDome API.</p><label className="field-label">Номер<input autoComplete="tel" aria-label="Номер телефона" type="tel" value={secret} onChange={(event) => setSecret(event.target.value)} placeholder="+7 999 000-00-00" /></label><button className="primary-button" disabled={busy || secret.length < 5} onClick={() => submit({ type: 'phone', phone: secret })} type="button">Получить код</button></>}
    {challenge.type === 'sms_otp' && <><span className="eyebrow">Демо-режим: SMS не отправляется</span><h2 id="auth-title">Подтверждение номера</h2><p>Новый код можно запросить через {challenge.resend_after_seconds} сек.</p><label className="field-label">Код из SMS<input autoComplete="one-time-code" aria-label="Код из SMS" inputMode="numeric" maxLength={6} value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || secret.length < 4} onClick={() => submit({ type: 'sms_otp', code: secret })} type="button">Подтвердить код</button></>}
    {challenge.type === 'captcha' && <><span className="eyebrow">Защита входа</span><h2 id="auth-title">Введите код с картинки</h2><img className="captcha-preview" src={challenge.image_data_url} alt="Проверочный рисунок" /><label className="field-label">Код с картинки<input autoComplete="off" aria-label="Код с картинки" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ type: 'captcha', answer: secret })} type="button">Продолжить</button></>}
    {challenge.type === 'password' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Введите пароль</h2><label className="field-label">Пароль<input autoComplete="current-password" aria-label="Пароль" type="password" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ type: 'password', password: secret })} type="button">Продолжить</button></>}
    {challenge.type === 'account_selection' && <><span className="eyebrow">Объект</span><h2 id="auth-title">Выберите аккаунт</h2><div className="stack-actions">{challenge.options.map((option) => <button className="secondary-button" onClick={() => submit({ type: 'account', id: option.id })} type="button" key={option.id}>{option.label}</button>)}</div></>}
    {challenge.type === 'consent' && <><span className="eyebrow">Согласие</span><h2 id="auth-title">{challenge.title}</h2><p>{challenge.body}</p><div className="stack-actions"><button className="primary-button" onClick={() => submit({ type: 'consent', accepted: true })} type="button">Разрешить</button><button className="secondary-button" onClick={() => submit({ type: 'consent', accepted: false })} type="button">Отклонить согласие</button></div></>}
  </>

  return <Modal className="auth-dialog" labelledBy="auth-title" onClose={onClose}>
    <button className="dialog-close" aria-label="Закрыть" onClick={onClose} type="button">×</button>
    {view === 'restoring' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Восстанавливаем сессию</h2><p aria-live="polite">Проверяем локальную сессию…</p></>}
    {view === 'choice' && <><span className="eyebrow">{api.isSyntheticPreview ? 'Режим предпросмотра' : 'Локальное подключение'}</span><h2 id="auth-title">Вход в VDome</h2><p>{api.isSyntheticPreview ? 'Используется синтетический сценарий без запросов к MTS.' : 'Авторизация проходит на официальной странице MTS ID; callback обрабатывает только локальный VDome API.'}</p><div className="stack-actions"><button className="primary-button" data-dialog-initial disabled={busy} onClick={begin} type="button">Войти с MTS ID</button></div></>}
    {view === 'challenge' && challengeForm}
    {view === 'redirect' && authorizationUrl && <><span className="eyebrow">Защищённый вход</span><h2 id="auth-title">Продолжите в MTS ID</h2><p>Откройте официальный login.mts.ru. После входа скопируйте полный адрес открывшейся callback-страницы gateway.vdome.mts.ru и вставьте ниже.</p><div className="stack-actions"><a className="primary-button auth-link" href={authorizationUrl} target="_blank" rel="noopener noreferrer">Продолжить в MTS ID</a></div><label className="field-label">Адрес страницы после входа<input aria-label="Адрес страницы после входа" autoComplete="off" inputMode="url" maxLength={8192} spellCheck={false} value={callbackUrl} onChange={(event) => setCallbackUrl(event.target.value)} /></label><button className="secondary-button" disabled={busy || !callbackUrl} onClick={completeRedirect} type="button">Завершить вход</button></>}
    {view === 'session' && session && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Сессия восстановлена</h2><p>Статус: {session.status}. Секреты и токены не отображаются.</p><button className="secondary-button" onClick={endSession} type="button">Выйти</button></>}
    {view === 'consent_rejected' && <><span className="eyebrow">Вход остановлен</span><h2 id="auth-title">Согласие отклонено</h2><p>Без согласия подключение домофона не выполняется.</p><button className="secondary-button" onClick={() => { setFlowId(null); setView('choice') }} type="button">Назад к способам входа</button></>}
    {view === 'rate_limited' && <><span className="eyebrow">Слишком много попыток</span><h2 id="auth-title">Вход временно ограничен</h2><p aria-live="polite">Повторите через {countdown} сек.</p><button className="secondary-button" disabled={countdown > 0} onClick={begin} type="button">Попробовать снова</button></>}
    {error && <p className="form-error" role="alert">{error}</p>}
  </Modal>
}
