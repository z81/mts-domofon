import { useCallback, useEffect, useRef, useState } from 'react'
import { Modal } from '../../app/Modal'
import type {
  AutoOpenIntent,
  AutoOpenMode,
  AutoOpenPolicy,
  CallApi,
} from './callApi'

export interface AutoOpenPanelProps {
  api: CallApi
  mutationBusy?: boolean
  getPolicyGeneration?: () => number
  isMutationCurrent?: (lease: AutoOpenMutationLease) => boolean
  onMutationEnd?: (lease: AutoOpenMutationLease) => void
  onMutationStart?: (controller: AbortController) => AutoOpenMutationLease | null
  onPolicyMutationConfirmed?: (lease: AutoOpenMutationLease) => void
  onPolicyChange?: (policy: AutoOpenPolicy) => void
  policy?: AutoOpenPolicy | null
}

export interface AutoOpenMutationLease {
  readonly signal: AbortSignal
}

interface PreparedPolicy extends AutoOpenIntent {
  mode: Exclude<AutoOpenMode, 'disabled'>
  dismissAfterOpen: boolean
}

interface PanelMutationLease {
  controller: AbortController
  shared: AutoOpenMutationLease | null
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

export function AutoOpenPanel({
  api,
  getPolicyGeneration,
  isMutationCurrent,
  mutationBusy = false,
  onMutationEnd,
  onMutationStart,
  onPolicyMutationConfirmed,
  onPolicyChange,
  policy: controlledPolicy,
}: AutoOpenPanelProps) {
  const available = Boolean(
    api.getAutoOpenPolicy
    && api.createAutoOpenIntent
    && api.executeAutoOpenIntent
    && api.disableAutoOpen,
  )
  const [observedPolicy, setObservedPolicy] = useState<AutoOpenPolicy | null>(controlledPolicy ?? null)
  const [loading, setLoading] = useState(available && controlledPolicy == null)
  const [failed, setFailed] = useState(false)
  const [dismissSelection, setDismissSelection] = useState<{
    policy: AutoOpenPolicy | null
    value: boolean
  } | null>(null)
  const [prepared, setPrepared] = useState<PreparedPolicy | null>(null)
  const [busy, setBusy] = useState(false)
  const activeMutation = useRef<PanelMutationLease | null>(null)
  const localPolicyGeneration = useRef(0)
  const readLocalPolicyGeneration = useCallback(() => localPolicyGeneration.current, [])
  const confirmLocalPolicyMutation = useCallback(() => {
    localPolicyGeneration.current += 1
  }, [])
  const readPolicyGeneration = getPolicyGeneration ?? readLocalPolicyGeneration
  const policy = controlledPolicy === undefined ? observedPolicy : controlledPolicy
  const dismissAfterOpen = dismissSelection?.policy === policy
    ? dismissSelection.value
    : policy?.dismissAfterOpen ?? false
  const policyLoading = loading && policy === null

  const publishPolicy = useCallback((next: AutoOpenPolicy) => {
    setObservedPolicy(next)
    onPolicyChange?.(next)
  }, [onPolicyChange])

  useEffect(() => {
    const getPolicy = api.getAutoOpenPolicy
    if (!getPolicy) return
    let active = true
    let timer: number | null = null
    let controller: AbortController | null = null
    const poll = async () => {
      const startedGeneration = readPolicyGeneration()
      controller = new AbortController()
      try {
        const next = await getPolicy(controller.signal)
        if (!active || startedGeneration !== readPolicyGeneration()) return
        publishPolicy(next)
        setFailed(false)
      } catch {
        if (active && !controller.signal.aborted && startedGeneration === readPolicyGeneration()) setFailed(true)
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
  }, [api, publishPolicy, readPolicyGeneration])

  useEffect(() => () => {
    const lease = activeMutation.current
    if (!lease) return
    activeMutation.current = null
    lease.controller.abort()
    if (lease.shared) onMutationEnd?.(lease.shared)
  }, [onMutationEnd])

  function beginMutation(): PanelMutationLease | null {
    if (activeMutation.current || mutationBusy) return null
    const controller = new AbortController()
    const shared = onMutationStart ? onMutationStart(controller) : null
    if (onMutationStart && !shared) {
      controller.abort()
      return null
    }
    const lease = { controller, shared }
    activeMutation.current = lease
    setBusy(true)
    setFailed(false)
    return lease
  }

  function mutationIsCurrent(lease: PanelMutationLease): boolean {
    return activeMutation.current === lease
      && !lease.controller.signal.aborted
      && (!lease.shared || !isMutationCurrent || isMutationCurrent(lease.shared))
  }

  function finishMutation(lease: PanelMutationLease) {
    if (activeMutation.current !== lease) return
    activeMutation.current = null
    setBusy(false)
    if (lease.shared) onMutationEnd?.(lease.shared)
  }

  async function prepare(mode: Exclude<AutoOpenMode, 'disabled'>) {
    const createIntent = api.createAutoOpenIntent
    if (!createIntent) return
    const lease = beginMutation()
    if (!lease) return
    try {
      const intent = await createIntent(mode, dismissAfterOpen, lease.controller.signal)
      if (!mutationIsCurrent(lease)) return
      setPrepared({ ...intent, mode, dismissAfterOpen })
    } catch {
      if (mutationIsCurrent(lease)) setFailed(true)
    } finally {
      finishMutation(lease)
    }
  }

  async function execute() {
    const executeIntent = api.executeAutoOpenIntent
    if (!executeIntent || !prepared) return
    const lease = beginMutation()
    if (!lease) return
    const intent = prepared
    try {
      const next = await executeIntent(intent.intentId, lease.controller.signal)
      if (!mutationIsCurrent(lease)) return
      setPrepared(null)
      if (lease.shared) onPolicyMutationConfirmed?.(lease.shared)
      else confirmLocalPolicyMutation()
      publishPolicy(next)
    } catch {
      if (!mutationIsCurrent(lease)) return
      setPrepared(null)
      setFailed(true)
    } finally {
      finishMutation(lease)
    }
  }

  async function disable() {
    const disablePolicy = api.disableAutoOpen
    if (!disablePolicy) return
    const lease = beginMutation()
    if (!lease) return
    try {
      await disablePolicy(lease.controller.signal)
      if (!mutationIsCurrent(lease)) return
      if (lease.shared) onPolicyMutationConfirmed?.(lease.shared)
      else confirmLocalPolicyMutation()
      publishPolicy({ mode: 'disabled', dismissAfterOpen: false })
    } catch {
      if (mutationIsCurrent(lease)) setFailed(true)
    } finally {
      finishMutation(lease)
    }
  }

  const controlsBusy = busy || mutationBusy

  if (!available) {
    return <section className="content-card auto-open-panel"><p className="eyebrow">Автооткрытие</p><h2>Автооткрытие недоступно</h2><p>Обновите локальный сервис, чтобы использовать ограниченные правила.</p></section>
  }

  return <section className={`content-card auto-open-panel ${policy?.mode !== 'disabled' ? 'armed' : ''}`}>
    <div className="section-title-row"><div><p className="eyebrow">Автооткрытие</p><h2>{policyLoading ? 'Проверяем правило…' : policy ? policyText(policy) : 'Состояние неизвестно'}</h2></div>{policy?.mode !== 'disabled' && <span className="armed-badge">ВКЛЮЧЕНО</span>}</div>
    <p>Правило действует только для звонка домофона и никогда не повторяет неудачную или неоднозначную команду.</p>
    {policy?.lastResult && <p className={`auto-open-result result-${policy.lastResult.status}`}>{resultLabels[policy.lastResult.status]}</p>}
    {failed && <p className="field-error" role="alert">Не удалось обновить автооткрытие</p>}
    <label className="auto-open-checkbox"><input checked={dismissAfterOpen} disabled={controlsBusy || policy?.mode !== 'disabled'} onChange={(event) => setDismissSelection({ policy, value: event.target.checked })} type="checkbox" />Сбрасывать звонок после открытия</label>
    <div className="auto-open-actions">
      {policy?.mode === 'disabled' ? <>
        <button className="primary-button" disabled={controlsBusy || policyLoading} onClick={() => void prepare('next_call')} type="button">Открыть следующий звонок</button>
        <button className="secondary-button" disabled={controlsBusy || policyLoading} onClick={() => void prepare('until')} type="button">Включить на 30 минут</button>
      </> : policy ? <button className="secondary-button danger-outline" disabled={controlsBusy} onClick={() => void disable()} type="button">Выключить автооткрытие</button> : null}
    </div>
    {prepared && <Modal className="door-dialog auto-open-dialog" labelledBy="auto-open-confirm-title" onClose={() => { if (!controlsBusy) setPrepared(null) }} closeOnBackdrop>
      <button aria-label="Закрыть" className="dialog-close" disabled={controlsBusy} onClick={() => setPrepared(null)} type="button">×</button>
      <span className="dialog-icon" aria-hidden="true">⌁</span>
      <h2 id="auto-open-confirm-title">Включить автооткрытие?</h2>
      <p>{prepared.mode === 'next_call' ? 'Будет автоматически открыт только следующий входящий звонок.' : 'Входящие звонки будут автоматически открываться не дольше 30 минут.'}</p>
      <dl className="door-summary">
        <div><dt>Режим</dt><dd>{prepared.mode === 'next_call' ? 'Следующий звонок' : '30 минут'}</dd></div>
        <div><dt>После открытия</dt><dd>{prepared.dismissAfterOpen ? 'Сбросить звонок' : 'Оставить звонок'}</dd></div>
      </dl>
      <div className="dialog-actions">
        <button className="secondary-button" data-dialog-initial disabled={controlsBusy} onClick={() => setPrepared(null)} type="button">Отмена</button>
        <button className="primary-button danger" disabled={controlsBusy} onClick={() => void execute()} type="button">{controlsBusy ? 'Включаем…' : 'Подтвердить включение'}</button>
      </div>
    </Modal>}
  </section>
}
