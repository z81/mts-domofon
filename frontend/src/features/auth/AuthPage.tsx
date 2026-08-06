import { useEffect, useState } from 'react'
import { Modal } from '../../app/Modal'
import { authApi, type AuthApi, type AuthChallenge, type AuthChallengeInput, type AuthSession } from './authApi'

export function AuthPage({ api = authApi, onClose, onAuthenticated }: {
  api?: AuthApi
  onClose: () => void
  onAuthenticated?: (session: AuthSession) => void
}) {
  const [view, setView] = useState<'restoring' | 'choice' | 'challenge' | 'session'>('restoring')
  const [flowId, setFlowId] = useState<string | null>(null)
  const [challenge, setChallenge] = useState<AuthChallenge | null>(null)
  const [session, setSession] = useState<AuthSession | null>(null)
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [countdown, setCountdown] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    api.getSession(controller.signal).then((restored) => {
      if (restored) { setSession(restored); setView('session') } else setView('choice')
    }).catch((cause: unknown) => {
      if (!(cause instanceof DOMException && cause.name === 'AbortError')) setView('choice')
    })
    return () => controller.abort()
  }, [api])

  useEffect(() => {
    if (challenge?.kind !== 'rate_limited') return
    const timer = window.setInterval(() => setCountdown((value) => Math.max(0, value - 1)), 1000)
    return () => window.clearInterval(timer)
  }, [challenge])

  function applyResult(result: Awaited<ReturnType<AuthApi['startAuth']>>) {
    setFlowId(result.flowId)
    if (result.challenge) {
      setChallenge(result.challenge)
      if (result.challenge.kind === 'rate_limited') setCountdown(result.challenge.retryAfterSeconds)
      setView('challenge')
    }
    if (result.session) {
      setSession(result.session)
      setView('session')
      onAuthenticated?.(result.session)
    }
  }

  async function begin() {
    setBusy(true); setError(null); setSecret('')
    try {
      const result = await api.startAuth()
      applyResult(result)
    } catch {
      setError('Не удалось начать вход')
    } finally { setBusy(false) }
  }

  async function submit(input: AuthChallengeInput) {
    if (!flowId) return
    setSecret('')
    setBusy(true)
    setError(null)
    try {
      const result = await api.submitChallenge(flowId, input)
      applyResult(result)
    } catch {
      setError('Не удалось продолжить вход')
    } finally { setBusy(false) }
  }

  async function endSession() {
    await api.logout()
    setSession(null); setFlowId(null); setChallenge(null); setSecret(''); setView('choice')
  }

  const challengeForm = challenge && <>
    {challenge.kind === 'phone' && <><span className="eyebrow">Номер телефона</span><h2 id="auth-title">Введите номер</h2><p>Демо-номер используется только для перехода к экрану кода.</p><label className="field-label">Номер<input autoComplete="tel" aria-label="Номер телефона" type="tel" value={secret} onChange={(event) => setSecret(event.target.value)} placeholder="+7 999 000-00-00" /></label><button className="primary-button" disabled={busy || secret.length < 5} onClick={() => submit({ kind: 'phone', value: secret })} type="button">Получить код</button></>}
    {challenge.kind === 'otp' && <><span className="eyebrow">Демо-режим: SMS не отправляется</span><h2 id="auth-title">Подтверждение номера</h2><p>Код для {challenge.destination} хранится только до отправки.</p><label className="field-label">Код из SMS<input autoComplete="one-time-code" aria-label="Код из SMS" inputMode="numeric" maxLength={6} value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || secret.length < 4} onClick={() => submit({ kind: 'otp', value: secret })} type="button">Подтвердить код</button></>}
    {challenge.kind === 'captcha' && <><span className="eyebrow">Защита входа</span><h2 id="auth-title">Введите код с картинки</h2><div className="captcha-preview" role="img" aria-label={challenge.imageAlt}>A7K2</div><label className="field-label">Код с картинки<input autoComplete="off" aria-label="Код с картинки" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ kind: 'captcha', value: secret })} type="button">Продолжить</button></>}
    {challenge.kind === 'password' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Введите пароль</h2><label className="field-label">Пароль<input autoComplete="current-password" aria-label="Пароль" type="password" value={secret} onChange={(event) => setSecret(event.target.value)} /></label><button className="primary-button" disabled={busy || !secret} onClick={() => submit({ kind: 'password', value: secret })} type="button">Продолжить</button></>}
    {challenge.kind === 'account_selection' && <><span className="eyebrow">Объект</span><h2 id="auth-title">Выберите аккаунт</h2><div className="stack-actions">{challenge.accounts.map((account) => <button className="secondary-button" onClick={() => submit({ kind: 'account_selection', accountId: account.id })} type="button" key={account.id}>{account.label}</button>)}</div></>}
    {challenge.kind === 'consent' && <><span className="eyebrow">Согласие</span><h2 id="auth-title">Разрешение доступа</h2><p>{challenge.text}</p><div className="stack-actions"><button className="primary-button" onClick={() => submit({ kind: 'consent', accepted: true })} type="button">Разрешить</button><button className="secondary-button" onClick={() => submit({ kind: 'consent', accepted: false })} type="button">Отклонить согласие</button></div></>}
    {challenge.kind === 'consent_rejected' && <><span className="eyebrow">Вход остановлен</span><h2 id="auth-title">Согласие отклонено</h2><p>Без согласия подключение домофона не выполняется.</p><button className="secondary-button" onClick={() => { setFlowId(null); setChallenge(null); setView('choice') }} type="button">Назад к способам входа</button></>}
    {challenge.kind === 'rate_limited' && <><span className="eyebrow">Слишком много попыток</span><h2 id="auth-title">Вход временно ограничен</h2><p aria-live="polite">Повторите через {countdown} сек.</p><button className="secondary-button" disabled={countdown > 0} onClick={begin} type="button">Попробовать снова</button></>}
  </>

  return <Modal className="auth-dialog" labelledBy="auth-title" onClose={onClose}>
    <button className="dialog-close" aria-label="Закрыть" onClick={onClose} type="button">×</button>
    {view === 'restoring' && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Восстанавливаем сессию</h2><p aria-live="polite">Проверяем локальную сессию…</p></>}
    {view === 'choice' && <><span className="eyebrow">Безопасный preview</span><h2 id="auth-title">Вход в VDome</h2><p>Секреты существуют только до отправки текущего шага и не сохраняются.</p><div className="stack-actions"><button className="primary-button" data-dialog-initial onClick={begin} type="button">Войти с MTS ID</button><button className="secondary-button" onClick={begin} type="button">Войти по номеру телефона</button></div></>}
    {view === 'challenge' && challengeForm}
    {view === 'session' && session && <><span className="eyebrow">MTS ID</span><h2 id="auth-title">Сессия восстановлена</h2><p>{session.displayName}, подключение доступно в этом окне.</p><button className="secondary-button" onClick={endSession} type="button">Выйти</button></>}
    {error && <p className="form-error" role="alert">{error}</p>}
  </Modal>
}
