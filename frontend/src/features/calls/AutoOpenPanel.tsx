import { useCallback, useEffect, useRef, useState } from 'react'
import { Modal } from '../../app/Modal'
import type {
  AutoOpenIntent,
  AutoOpenMode,
  AutoOpenPolicy,
  CallApi,
} from './callApi'

interface AutoOpenPanelProps {
  api: CallApi
  onPolicyChange?: (policy: AutoOpenPolicy) => void
}

interface PreparedPolicy extends AutoOpenIntent {
  mode: Exclude<AutoOpenMode, 'disabled'>
  dismissAfterOpen: boolean
}

function policyText(policy: AutoOpenPolicy): string {
  if (policy.mode === 'disabled') return 'Автооткрытие выключено'
  if (policy.mode === 'next_call') return 'Откроется следующий входящий звонок'
  const deadline = policy.until ? new Date(policy.until) : null
  const formatted = deadline && !Number.isNaN(deadline.getTime())
    ? new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' }).format(deadline)
    : '30 минут'
  return `Автооткрытие включено до ${formatted}`
}

const resultLabels = {
  opened: 'Последняя дверь открыта',
  failed: 'Последняя попытка не удалась и не будет повторена',
  outcome_unknown: 'Результат последней попытки неизвестен; автоматического повтора не будет',
} as const

export function AutoOpenPanel({ api, onPolicyChange }: AutoOpenPanelProps) {
  const available = Boolean(
    api.getAutoOpenPolicy
    && api.createAutoOpenIntent
    && api.executeAutoOpenIntent
    && api.disableAutoOpen,
  )
  const [policy, setPolicy] = useState<AutoOpenPolicy | null>(null)
  const [loading, setLoading] = useState(available)
  const [failed, setFailed] = useState(false)
  const [dismissAfterOpen, setDismissAfterOpen] = useState(false)
  const [prepared, setPrepared] = useState<PreparedPolicy | null>(null)
  const [busy, setBusy] = useState(false)
  const operationInFlight = useRef(false)

  const publishPolicy = useCallback((next: AutoOpenPolicy) => {
    setPolicy(next)
    setDismissAfterOpen(next.dismissAfterOpen)
    onPolicyChange?.(next)
  }, [onPolicyChange])

  useEffect(() => {
    const getPolicy = api.getAutoOpenPolicy
    if (!getPolicy) return
    let active = true
    let timer: number | null = null
    let controller: AbortController | null = null
    const poll = async () => {
      controller = new AbortController()
      try {
        const next = await getPolicy(controller.signal)
        if (!active) return
        publishPolicy(next)
        setFailed(false)
      } catch {
        if (active && !controller.signal.aborted) setFailed(true)
      } finally {
        if (active) {
          setLoading(false)
          timer = window.setTimeout(() => void poll(), 5_000)
        }
      }
    }
    void poll()
    return () => {
      active = false
      if (timer !== null) window.clearTimeout(timer)
      controller?.abort()
    }
  }, [api, publishPolicy])

  async function prepare(mode: Exclude<AutoOpenMode, 'disabled'>) {
    const createIntent = api.createAutoOpenIntent
    if (!createIntent || operationInFlight.current) return
    operationInFlight.current = true
    setBusy(true)
    setFailed(false)
    const controller = new AbortController()
    try {
      const intent = await createIntent(mode, dismissAfterOpen, controller.signal)
      setPrepared({ ...intent, mode, dismissAfterOpen })
    } catch {
      setFailed(true)
    } finally {
      operationInFlight.current = false
      setBusy(false)
    }
  }

  async function execute() {
    const executeIntent = api.executeAutoOpenIntent
    if (!executeIntent || !prepared || operationInFlight.current) return
    const intent = prepared
    operationInFlight.current = true
    setBusy(true)
    setFailed(false)
    const controller = new AbortController()
    try {
      const next = await executeIntent(intent.intentId, controller.signal)
      setPrepared(null)
      publishPolicy(next)
    } catch {
      setPrepared(null)
      setFailed(true)
    } finally {
      operationInFlight.current = false
      setBusy(false)
    }
  }

  async function disable() {
    const disablePolicy = api.disableAutoOpen
    if (!disablePolicy || operationInFlight.current) return
    operationInFlight.current = true
    setBusy(true)
    setFailed(false)
    const controller = new AbortController()
    try {
      await disablePolicy(controller.signal)
      publishPolicy({ mode: 'disabled', dismissAfterOpen: false })
    } catch {
      setFailed(true)
    } finally {
      operationInFlight.current = false
      setBusy(false)
    }
  }

  if (!available) {
    return <section className="content-card auto-open-panel"><p className="eyebrow">Автооткрытие</p><h2>Автооткрытие недоступно</h2><p>Обновите локальный сервис, чтобы использовать ограниченные правила.</p></section>
  }

  return <section className={`content-card auto-open-panel ${policy?.mode !== 'disabled' ? 'armed' : ''}`}>
    <div className="section-title-row"><div><p className="eyebrow">Автооткрытие</p><h2>{loading ? 'Проверяем правило…' : policy ? policyText(policy) : 'Состояние неизвестно'}</h2></div>{policy?.mode !== 'disabled' && <span className="armed-badge">ВКЛЮЧЕНО</span>}</div>
    <p>Правило действует только для звонка домофона и никогда не повторяет неудачную или неоднозначную команду.</p>
    {policy?.lastResult && <p className={`auto-open-result result-${policy.lastResult.status}`}>{resultLabels[policy.lastResult.status]}</p>}
    {failed && <p className="field-error" role="alert">Не удалось обновить автооткрытие</p>}
    <label className="auto-open-checkbox"><input checked={dismissAfterOpen} disabled={busy || policy?.mode !== 'disabled'} onChange={(event) => setDismissAfterOpen(event.target.checked)} type="checkbox" />Сбрасывать звонок после открытия</label>
    <div className="auto-open-actions">
      {policy?.mode === 'disabled' ? <>
        <button className="primary-button" disabled={busy || loading} onClick={() => void prepare('next_call')} type="button">Открыть следующий звонок</button>
        <button className="secondary-button" disabled={busy || loading} onClick={() => void prepare('until')} type="button">Включить на 30 минут</button>
      </> : policy ? <button className="secondary-button danger-outline" disabled={busy} onClick={() => void disable()} type="button">Выключить автооткрытие</button> : null}
    </div>
    {prepared && <Modal className="door-dialog auto-open-dialog" labelledBy="auto-open-confirm-title" onClose={() => { if (!busy) setPrepared(null) }} closeOnBackdrop>
      <button aria-label="Закрыть" className="dialog-close" disabled={busy} onClick={() => setPrepared(null)} type="button">×</button>
      <span className="dialog-icon" aria-hidden="true">⌁</span>
      <h2 id="auto-open-confirm-title">Включить автооткрытие?</h2>
      <p>{prepared.mode === 'next_call' ? 'Будет автоматически открыт только следующий входящий звонок.' : 'Входящие звонки будут автоматически открываться не дольше 30 минут.'}</p>
      <dl className="door-summary">
        <div><dt>Режим</dt><dd>{prepared.mode === 'next_call' ? 'Следующий звонок' : '30 минут'}</dd></div>
        <div><dt>После открытия</dt><dd>{prepared.dismissAfterOpen ? 'Сбросить звонок' : 'Оставить звонок'}</dd></div>
      </dl>
      <div className="dialog-actions">
        <button className="secondary-button" data-dialog-initial disabled={busy} onClick={() => setPrepared(null)} type="button">Отмена</button>
        <button className="primary-button danger" disabled={busy} onClick={() => void execute()} type="button">{busy ? 'Включаем…' : 'Подтвердить включение'}</button>
      </div>
    </Modal>}
  </section>
}
