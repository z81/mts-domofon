import { useCallback, useEffect, useEffectEvent, useMemo, useRef, useState, type ReactNode } from 'react'
import { AuthPage } from '../features/auth/AuthPage'
import { AuthApiError, authApi as defaultAuthApi, type AuthApi } from '../features/auth/authApi'
import { IncomingCallOverlay } from '../features/calls/IncomingCallOverlay'
import type { AutoOpenMutationLease } from '../features/calls/AutoOpenPanel'
import {
  callApi,
  startCurrentCallPolling,
  type AutoOpenPolicy,
  type AutoOpenResult,
  type CallApi,
  type CurrentCall,
} from '../features/calls/callApi'
import { EventJournal } from '../features/events/EventJournal'
import { eventApi as defaultEventApi, type EventApi } from '../features/events/eventApi'
import { HomePage } from '../features/home/HomePage'
import { CameraExperience } from '../features/intercom/CameraExperience'
import { DoorGrid, type DoorFeedback, type PreparingLock } from '../features/intercom/DoorGrid'
import { MorePage } from '../features/settings/MorePage'
import {
  IntercomApiError,
  intercomApi,
  type AccessPoint,
  type DeviceInventory,
  type Flat,
  type IntercomApi,
  type Lock,
  type PropertyDashboard,
} from '../features/intercom/intercomApi'
import { Modal } from './Modal'
import { AppShell, type AppView, type Destination } from './AppShell'

type Remote<T> =
  | { status: 'loading' }
  | { status: 'success'; data: T }
  | { status: 'error'; error: unknown }

type DeviceRemote =
  | { status: 'idle' }
  | { status: 'loading'; flatId: string }
  | { status: 'success'; flatId: string; data: DeviceInventory }
  | { status: 'error'; flatId: string; error: unknown }

interface AppProps {
  api?: IntercomApi
  authApi?: AuthApi
  callsApi?: CallApi
  eventsApi?: EventApi
}

interface PreparedDoorIntent {
  intentId: string
  expiresAt: string
  flatLabel: string
  flatAddress: string
  accessPointName: string
  lockName: string
  accessPointId: string
  lockNumber: string
}

interface DoorPreparationLease {
  controller: AbortController
  flatId: string
  sessionEpoch: number
}

interface AppAutoOpenMutationLease extends AutoOpenMutationLease {
  controller: AbortController
  sessionEpoch: number
}


function Glyph({ children }: { children: ReactNode }) {
  return <span className="glyph" aria-hidden="true">{children}</span>
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof IntercomApiError && error.status === 401
}

async function sessionRecovered(authApi: AuthApi, signal: AbortSignal): Promise<boolean> {
  const session = await authApi.getSession(signal)
  if (session.authenticated) return true
  if (session.status !== 'expired') return false
  try {
    // Not tied to the load's signal: an aborted request must not cut a token rotation short.
    return (await authApi.refreshSession()).authenticated
  } catch (error) {
    if (error instanceof AuthApiError && ['session_expired', 'upstream_authentication_failed'].includes(error.envelope.code)) return false
    throw error
  }
}

async function loadWithSessionRecovery<T>(authApi: AuthApi, signal: AbortSignal, load: () => Promise<T>, onRecovered: () => void): Promise<T> {
  try {
    return await load()
  } catch (error) {
    if (!isUnauthorized(error) || !await sessionRecovered(authApi, signal)) throw error
    onRecovered()
    return await load()
  }
}

function preferredFlat(dashboard: PropertyDashboard): string | null {
  if (dashboard.selectedFlatId && dashboard.flats.some((flat) => flat.id === dashboard.selectedFlatId)) {
    return dashboard.selectedFlatId
  }
  return dashboard.flats[0]?.id ?? null
}

function formatDoorExpiry(value: string): string {
  const expiresAt = new Date(value)
  if (Number.isNaN(expiresAt.getTime())) return 'в течение короткого времени'
  return new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(expiresAt)
}

function formatAutoOpenBanner(policy: AutoOpenPolicy | null): string {
  if (policy?.mode === 'next_call') return 'Автооткрытие: следующий звонок'
  if (policy?.mode !== 'until' || !policy.until) return 'Автооткрытие включено'

  const until = new Date(policy.until)
  if (Number.isNaN(until.getTime())) return 'Автооткрытие включено'
  return `Автооткрытие до ${new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit',
    minute: '2-digit',
  }).format(until)}`
}

function autoOpenResultForCall(call: CurrentCall | null, policy: AutoOpenPolicy | null): AutoOpenResult | undefined {
  const result = policy?.lastResult
  if (!call || !result) return undefined
  const callStartedAt = new Date(call.startedAt).getTime()
  const resultCompletedAt = new Date(result.completedAt).getTime()
  if (Number.isNaN(callStartedAt) || Number.isNaN(resultCompletedAt) || resultCompletedAt < callStartedAt) return undefined
  return result
}

function StateCard({ eyebrow, title, children, action }: {
  eyebrow: string
  title: string
  children: ReactNode
  action?: ReactNode
}) {
  return <section className="state-card">
    <span className="state-icon" aria-hidden="true">⌂</span>
    <p className="eyebrow">{eyebrow}</p>
    <h1>{title}</h1>
    <div className="state-copy">{children}</div>
    {action && <div className="state-action">{action}</div>}
  </section>
}

function DeviceError({ unauthorized, onAuth, onRetry }: { unauthorized: boolean; onAuth: () => void; onRetry: () => void }) {
  return <StateCard
    eyebrow="Устройства"
    title={unauthorized ? 'Сессия MTS ID истекла' : 'Не удалось загрузить устройства'}
    action={<button className="primary-button" onClick={unauthorized ? onAuth : onRetry} type="button">{unauthorized ? 'Войти с MTS ID' : 'Повторить'}</button>}
  >
    <p>{unauthorized ? 'Войдите снова, чтобы получить список домофонов и камер.' : 'Проверьте локальный сервис и повторите запрос.'}</p>
  </StateCard>
}

function App({ api = intercomApi, authApi = defaultAuthApi, callsApi = callApi, eventsApi = defaultEventApi }: AppProps) {
  const [view, setView] = useState<AppView>({ kind: 'destination', destination: 'home' })
  const [flatOpen, setFlatOpen] = useState(false)
  const [authOpen, setAuthOpen] = useState(false)
  const [dataSessionEnabled, setDataSessionEnabled] = useState(true)
  const [dashboardReload, setDashboardReload] = useState(0)
  const [deviceReload, setDeviceReload] = useState(0)
  const [dashboard, setDashboard] = useState<Remote<PropertyDashboard>>({ status: 'loading' })
  const [selectedFlatId, setSelectedFlatId] = useState<string | null>(null)
  const [devices, setDevices] = useState<DeviceRemote>({ status: 'idle' })
  const [selectedCameraId, setSelectedCameraId] = useState<string | null>(null)
  const [preparingLock, setPreparingLock] = useState<PreparingLock | null>(null)
  const [doorIntent, setDoorIntent] = useState<PreparedDoorIntent | null>(null)
  const [doorExecuting, setDoorExecuting] = useState(false)
  const [blockedLock, setBlockedLock] = useState<PreparingLock | null>(null)
  const [doorFeedback, setDoorFeedback] = useState<DoorFeedback | null>(null)
  const [currentCall, setCurrentCall] = useState<CurrentCall | null>(null)
  const [callLoading, setCallLoading] = useState(true)
  const [callPollingError, setCallPollingError] = useState(false)
  const [callSessionAuthenticated, setCallSessionAuthenticated] = useState<boolean | null>(null)
  const [autoOpenPolicy, setAutoOpenPolicy] = useState<AutoOpenPolicy | null>(null)
  const [autoOpenDisableFailed, setAutoOpenDisableFailed] = useState(false)
  const [autoOpenMutationBusy, setAutoOpenMutationBusy] = useState(false)
  const doorPreparationLease = useRef<DoorPreparationLease | null>(null)
  const doorExecutionInFlight = useRef(false)
  const autoOpenMutationLease = useRef<AppAutoOpenMutationLease | null>(null)
  const autoOpenPolicyGeneration = useRef(0)
  const sessionEpoch = useRef(0)
  const autoResumeEpoch = useRef<number | null>(null)

  useEffect(() => {
    const requestEpoch = sessionEpoch.current
    const controller = new AbortController()
    authApi.getSession(controller.signal).then((session) => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) {
        setCallSessionAuthenticated(session.authenticated)
      }
    }).catch(() => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) {
        setCallSessionAuthenticated(false)
      }
    })
    return () => controller.abort()
  }, [authApi])

  useEffect(() => {
    if (!dataSessionEnabled) return
    const requestEpoch = sessionEpoch.current
    const controller = new AbortController()
    loadWithSessionRecovery(authApi, controller.signal, () => api.getDashboard(controller.signal), () => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) setCallSessionAuthenticated(true)
    }).then((result) => {
      if (controller.signal.aborted || requestEpoch !== sessionEpoch.current) return
      autoResumeEpoch.current = null
      const flatId = preferredFlat(result)
      setDashboard({ status: 'success', data: result })
      setSelectedFlatId(flatId)
      setSelectedCameraId(null)
      setDevices(flatId ? { status: 'loading', flatId } : { status: 'idle' })
      setDeviceReload((value) => value + 1)
    }).catch((error: unknown) => {
      if (controller.signal.aborted || requestEpoch !== sessionEpoch.current) return
      setDashboard({ status: 'error', error })
      if (isUnauthorized(error)) setCallSessionAuthenticated(false)
    })
    return () => controller.abort()
  }, [api, authApi, dashboardReload, dataSessionEnabled])

  useEffect(() => {
    if (!dataSessionEnabled || !selectedFlatId) return

    const flatId = selectedFlatId
    const requestEpoch = sessionEpoch.current
    const controller = new AbortController()
    loadWithSessionRecovery(authApi, controller.signal, () => api.getDevices(flatId, controller.signal), () => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) setCallSessionAuthenticated(true)
    }).then((result) => {
      if (controller.signal.aborted || requestEpoch !== sessionEpoch.current) return
      autoResumeEpoch.current = null
      setDevices({ status: 'success', flatId, data: result })
      setSelectedCameraId((current) => result.cameras.some((camera) => camera.id === current) ? current : (result.cameras[0]?.id ?? null))
    }).catch((error: unknown) => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) setDevices({ status: 'error', flatId, error })
    })
    return () => controller.abort()
  }, [api, authApi, dataSessionEnabled, deviceReload, selectedFlatId])

  const callPollingEnabled = dataSessionEnabled && callSessionAuthenticated === true

  useEffect(() => {
    if (!callPollingEnabled) return

    let active = true
    const stop = startCurrentCallPolling(callsApi, {
      onUpdate: (nextCall) => {
        if (!active) return
        setCurrentCall(nextCall)
        setCallLoading(false)
        setCallPollingError(false)
      },
      onError: () => {
        if (!active) return
        setCallLoading(false)
        setCallPollingError(true)
      },
    })
    return () => {
      active = false
      stop()
    }
  }, [callPollingEnabled, callsApi])

  const activeCurrentCall = callPollingEnabled ? currentCall : null
  const activeCallAutoOpenResult = autoOpenResultForCall(activeCurrentCall, autoOpenPolicy)

  useEffect(() => {
    if (!doorIntent || doorExecuting) return
    const expiresAt = new Date(doorIntent.expiresAt).getTime()
    if (Number.isNaN(expiresAt)) return
    const delay = Math.max(0, expiresAt - Date.now())
    const timer = window.setTimeout(() => {
      setDoorIntent(null)
      setDoorFeedback({
        accessPointId: doorIntent.accessPointId,
        lockNumber: doorIntent.lockNumber,
        kind: 'error',
        message: 'Подтверждение истекло. Подготовьте открытие заново.',
      })
    }, Math.min(delay, 2_147_483_647))
    return () => window.clearTimeout(timer)
  }, [doorExecuting, doorIntent])

  const selectedFlat = useMemo<Flat | null>(() => {
    if (dashboard.status !== 'success') return null
    return dashboard.data.flats.find((flat) => flat.id === selectedFlatId) ?? null
  }, [dashboard, selectedFlatId])

  const inventory = devices.status === 'success' && devices.flatId === selectedFlatId ? devices.data : null
  const selectedCamera = inventory?.cameras.find((camera) => camera.id === selectedCameraId) ?? inventory?.cameras[0] ?? null
  const doorInteractionLocked = preparingLock !== null || doorIntent !== null || doorExecuting
  const unauthenticated = (dashboard.status === 'error' && isUnauthorized(dashboard.error))
    || (dashboard.status === 'success' && devices.status === 'error' && isUnauthorized(devices.error))

  const destination = view.kind === 'destination' ? view.destination : view.returnTo

  function selectDestination(next: Destination) {
    setView({ kind: 'destination', destination: next })
    setFlatOpen(false)
  }

  function selectFlat(flatId: string) {
    if (flatId === selectedFlatId) {
      setFlatOpen(false)
      return
    }
    const preparation = doorPreparationLease.current
    doorPreparationLease.current = null
    preparation?.controller.abort()
    setPreparingLock(null)
    setDoorIntent(null)
    setSelectedFlatId(flatId)
    setSelectedCameraId(null)
    setView((current) => current.kind === 'camera'
      ? { kind: 'camera', returnTo: current.returnTo }
      : current)
    setBlockedLock(null)
    setDoorFeedback(null)
    setDevices({ status: 'loading', flatId })
    setFlatOpen(false)
  }

  function finishAuthentication() {
    sessionEpoch.current += 1
    doorPreparationLease.current?.controller.abort()
    doorPreparationLease.current = null
    autoOpenMutationLease.current?.controller.abort()
    autoOpenMutationLease.current = null
    setDataSessionEnabled(true)
    setAuthOpen(false)
    setBlockedLock(null)
    setDoorFeedback(null)
    setPreparingLock(null)
    setDoorIntent(null)
    setCurrentCall(null)
    setCallLoading(true)
    setCallPollingError(false)
    setCallSessionAuthenticated(true)
    autoOpenPolicyGeneration.current += 1
    setAutoOpenPolicy(null)
    setAutoOpenDisableFailed(false)
    setAutoOpenMutationBusy(false)
    setDashboard({ status: 'loading' })
    setDashboardReload((value) => value + 1)
  }

  function finishLogout() {
    sessionEpoch.current += 1
    doorPreparationLease.current?.controller.abort()
    doorPreparationLease.current = null
    doorExecutionInFlight.current = false
    autoOpenMutationLease.current?.controller.abort()
    autoOpenMutationLease.current = null
    autoOpenPolicyGeneration.current += 1
    setDataSessionEnabled(false)
    setAuthOpen(false)
    setView({ kind: 'destination', destination: 'home' })
    setFlatOpen(false)
    setDashboard({ status: 'error', error: new IntercomApiError(401, 'authentication_required') })
    setSelectedFlatId(null)
    setDevices({ status: 'idle' })
    setSelectedCameraId(null)
    setPreparingLock(null)
    setDoorIntent(null)
    setDoorExecuting(false)
    setBlockedLock(null)
    setDoorFeedback(null)
    setCurrentCall(null)
    setCallLoading(false)
    setCallPollingError(false)
    setCallSessionAuthenticated(false)
    setAutoOpenPolicy(null)
    setAutoOpenDisableFailed(false)
    setAutoOpenMutationBusy(false)
  }

  function resumeSession() {
    finishAuthentication()
    autoResumeEpoch.current = sessionEpoch.current
  }

  function resumeRestoredSession() {
    // Resume from the dialog once until a load succeeds: if that did not stick, the dialog must still offer logout.
    if (unauthenticated && autoResumeEpoch.current !== sessionEpoch.current) resumeSession()
  }

  const resumeVisibleSession = useEffectEvent(() => {
    if (unauthenticated) resumeSession()
  })

  useEffect(() => {
    if (!unauthenticated) return
    const controller = new AbortController()
    function recheckSession() {
      if (document.visibilityState !== 'visible') return
      const requestEpoch = sessionEpoch.current
      authApi.getSession(controller.signal).then((session) => {
        if (session.authenticated && !controller.signal.aborted && requestEpoch === sessionEpoch.current) resumeVisibleSession()
      }).catch(() => undefined)
    }
    document.addEventListener('visibilitychange', recheckSession)
    return () => {
      document.removeEventListener('visibilitychange', recheckSession)
      controller.abort()
    }
  }, [authApi, unauthenticated])

  function retryDashboard() {
    setDashboard({ status: 'loading' })
    setDashboardReload((value) => value + 1)
  }

  function retryDevices() {
    if (!selectedFlatId) return
    setDevices({ status: 'loading', flatId: selectedFlatId })
    setDeviceReload((value) => value + 1)
  }

  function openEventArchive(cameraId: string, archiveAt: string) {
    if (!inventory?.cameras.some((camera) => camera.id === cameraId)) return
    setSelectedCameraId(cameraId)
    setView({ kind: 'camera', returnTo: 'events', archiveTarget: { cameraId, archiveAt } })
    setFlatOpen(false)
  }

  const beginAutoOpenMutation = useCallback((controller = new AbortController()): AppAutoOpenMutationLease | null => {
    if (autoOpenMutationLease.current) return null
    const lease: AppAutoOpenMutationLease = {
      controller,
      sessionEpoch: sessionEpoch.current,
      signal: controller.signal,
    }
    autoOpenMutationLease.current = lease
    setAutoOpenMutationBusy(true)
    setAutoOpenDisableFailed(false)
    return lease
  }, [])

  const isAutoOpenMutationCurrent = useCallback((lease: AutoOpenMutationLease): boolean => {
    return autoOpenMutationLease.current === lease
      && !lease.signal.aborted
      && autoOpenMutationLease.current.sessionEpoch === sessionEpoch.current
  }, [])

  const endAutoOpenMutation = useCallback((lease: AutoOpenMutationLease) => {
    if (autoOpenMutationLease.current !== lease) return
    autoOpenMutationLease.current = null
    setAutoOpenMutationBusy(false)
  }, [])

  const getAutoOpenPolicyGeneration = useCallback(() => autoOpenPolicyGeneration.current, [])
  const confirmAutoOpenPolicyMutation = useCallback((lease: AutoOpenMutationLease) => {
    if (!isAutoOpenMutationCurrent(lease)) return
    autoOpenPolicyGeneration.current += 1
  }, [isAutoOpenMutationCurrent])

  async function disableAutoOpenFromHome() {
    if (!callsApi.disableAutoOpen) return
    const lease = beginAutoOpenMutation()
    if (!lease) return
    try {
      await callsApi.disableAutoOpen(lease.signal)
      if (!isAutoOpenMutationCurrent(lease)) return
      confirmAutoOpenPolicyMutation(lease)
      setAutoOpenPolicy({ mode: 'disabled', dismissAfterOpen: false })
    } catch {
      if (isAutoOpenMutationCurrent(lease)) setAutoOpenDisableFailed(true)
    } finally {
      endAutoOpenMutation(lease)
    }
  }

  async function prepareDoor(flat: Flat, point: AccessPoint, lock: Lock) {
    if (doorPreparationLease.current || doorExecutionInFlight.current || doorIntent) return
    const lease: DoorPreparationLease = {
      controller: new AbortController(),
      flatId: flat.id,
      sessionEpoch: sessionEpoch.current,
    }
    doorPreparationLease.current = lease
    setPreparingLock({ accessPointId: point.id, lockNumber: lock.number })
    setDoorFeedback(null)
    try {
      const intent = await api.createDoorIntent(flat.id, point.id, lock.number, lease.controller.signal)
      if (doorPreparationLease.current !== lease || lease.controller.signal.aborted || lease.sessionEpoch !== sessionEpoch.current) return
      setDoorIntent({
        intentId: intent.intentId,
        expiresAt: intent.expiresAt,
        flatLabel: flat.label,
        flatAddress: flat.address,
        accessPointName: point.name,
        lockName: lock.name.trim() || `Замок ${lock.number}`,
        accessPointId: point.id,
        lockNumber: lock.number,
      })
    } catch {
      if (doorPreparationLease.current === lease && !lease.controller.signal.aborted && lease.sessionEpoch === sessionEpoch.current) setDoorFeedback({
        accessPointId: point.id,
        lockNumber: lock.number,
        kind: 'error',
        message: 'Не удалось подготовить открытие. Повторите попытку.',
      })
    } finally {
      if (doorPreparationLease.current === lease) {
        doorPreparationLease.current = null
        setPreparingLock(null)
      }
    }
  }

  const closeDoorIntent = useCallback(() => {
    if (doorExecutionInFlight.current) return
    setDoorIntent(null)
  }, [])

  async function executeDoor() {
    if (!doorIntent || doorExecutionInFlight.current) return
    const intent = doorIntent
    const operationEpoch = sessionEpoch.current
    doorExecutionInFlight.current = true
    setDoorExecuting(true)
    setDoorFeedback(null)
    try {
      await api.executeDoorIntent(intent.intentId)
      if (operationEpoch !== sessionEpoch.current) return
      setDoorIntent(null)
      setDoorFeedback({
        accessPointId: intent.accessPointId,
        lockNumber: intent.lockNumber,
        kind: 'success',
        message: `${intent.accessPointName} открыт`,
      })
    } catch {
      if (operationEpoch !== sessionEpoch.current) return
      setDoorIntent(null)
      setBlockedLock({ accessPointId: intent.accessPointId, lockNumber: intent.lockNumber })
      setDoorFeedback({
        accessPointId: intent.accessPointId,
        lockNumber: intent.lockNumber,
        kind: 'error',
        message: 'Результат открытия неизвестен. Не повторяйте команду сразу — сначала проверьте дверь.',
      })
    } finally {
      if (operationEpoch === sessionEpoch.current) {
        doorExecutionInFlight.current = false
        setDoorExecuting(false)
      }
    }
  }

  function renderDeviceContent() {
    if (!selectedFlat) return null
    if (devices.status === 'idle' || devices.status === 'loading' || devices.flatId !== selectedFlat.id) {
      return <div className="loading-card" role="status"><span className="loading-ring" /><p>Загружаем устройства…</p></div>
    }
    if (devices.status === 'error') {
      return <DeviceError unauthorized={isUnauthorized(devices.error)} onAuth={() => setAuthOpen(true)} onRetry={retryDevices} />
    }

    const deviceInventory = devices.data
    const activeArchiveAt = view.kind === 'camera' && view.archiveTarget?.cameraId === selectedCamera?.id
      ? view.archiveTarget?.archiveAt
      : undefined
    const cameraPanelKey = `${selectedFlat.id}\u0000${selectedCamera?.id ?? ''}\u0000${activeArchiveAt ?? ''}`

    if (view.kind === 'camera') {
      return <>
        <section className="page-heading camera-detail-heading"><div><p className="eyebrow">{selectedFlat.address}</p><h1>Камеры</h1><p>Снимки и доступные режимы выбранной камеры.</p></div><button className="outline-button" onClick={() => selectDestination(view.returnTo)} type="button">Назад</button></section>
        {deviceInventory.cameras.length > 1 && <div className="camera-tabs" aria-label="Выбор камеры" role="group">
          {deviceInventory.cameras.map((camera) => <button aria-pressed={camera.id === selectedCamera?.id} className={camera.id === selectedCamera?.id ? 'selected' : ''} onClick={() => { setSelectedCameraId(camera.id); setView({ kind: 'camera', returnTo: view.returnTo }) }} type="button" key={camera.id}>{camera.name}</button>)}
        </div>}
        <section className="camera-layout">
          <CameraExperience api={api} camera={selectedCamera} flatId={selectedFlat.id} initialArchiveAt={activeArchiveAt} key={cameraPanelKey} presentation="detail" />
          <aside className="content-card camera-info"><p className="eyebrow">Возможности</p><h2>{selectedCamera?.name ?? 'Камеры не найдены'}</h2><p>{selectedCamera ? [
            selectedCamera.capabilities.snapshot.state === 'available' ? 'снимок' : null,
            selectedCamera.capabilities.live.state === 'available' ? 'прямой эфир со звуком' : null,
            selectedCamera.capabilities.archive.state === 'available' ? 'архив со звуком' : null,
          ].filter(Boolean).join(' · ') || 'Медиа недоступно' : 'Для выбранного объекта MTS не вернул камер.'}</p>
          {selectedCamera && selectedCamera.capabilities.archive.state !== 'available' && <p className="capability-note">Архив этой камеры недоступен</p>}
          </aside>
        </section>
      </>
    }

    if (destination === 'events') {
      return <EventJournal api={eventsApi} flatId={selectedFlat.id} onOpenArchive={openEventArchive} variant="full" />
    }

    if (destination === 'more') {
      return null
    }

    const autoOpenStrip = autoOpenPolicy === null ? null : autoOpenPolicy.mode !== 'disabled'
      ? <section aria-label="Автооткрытие включено" className="auto-open-strip" role="status">
          <span>{formatAutoOpenBanner(autoOpenPolicy)}</span>
          <button disabled={autoOpenMutationBusy} onClick={() => void disableAutoOpenFromHome()} type="button">Выключить</button>
          {autoOpenDisableFailed && <span role="alert">Не удалось выключить автооткрытие</span>}
        </section>
      : <button className="auto-open-entry" onClick={() => selectDestination('more')} type="button">Автооткрытие</button>

    return <HomePage
      activity={<EventJournal api={eventsApi} flatId={selectedFlat.id} onOpenArchive={openEventArchive} variant="compact" />}
      autoOpen={autoOpenStrip}
      camera={<>
        {deviceInventory.cameras.length > 1 && <div aria-label="Камеры на главной" className="home-camera-switch" role="group">
          {deviceInventory.cameras.map((camera) => <button
            aria-pressed={camera.id === selectedCamera?.id}
            className={camera.id === selectedCamera?.id ? 'selected' : ''}
            key={camera.id}
            onClick={() => setSelectedCameraId(camera.id)}
            type="button"
          >{camera.name}</button>)}
        </div>}
        <CameraExperience
            api={api}
            camera={selectedCamera}
            flatId={selectedFlat.id}
            key={cameraPanelKey}
            onOpenDetail={() => setView({ kind: 'camera', returnTo: destination })}
            presentation="preview"
          />
      </>}
      doors={<DoorGrid
            accessPoints={deviceInventory.accessPoints}
            interactionLocked={doorInteractionLocked}
            preparingLock={preparingLock}
            blockedLock={blockedLock}
            feedback={doorFeedback}
            onPrepare={(point, lock) => void prepareDoor(selectedFlat, point, lock)}
          />}
      onOpenAllEvents={() => selectDestination('events')}
    />
  }

  let mainContent: ReactNode
  if (dashboard.status === 'loading') {
    mainContent = <div className="loading-card" role="status"><span className="loading-ring" /><p>Загружаем объекты…</p></div>
  } else if (dashboard.status === 'error' && isUnauthorized(dashboard.error)) {
    mainContent = <StateCard eyebrow="MTS ID" title="Войдите в MTS ID" action={<button className="primary-button" onClick={() => setAuthOpen(true)} type="button">Войти с MTS ID</button>}><p>Авторизация нужна, чтобы загрузить ваши квартиры и домофоны.</p></StateCard>
  } else if (dashboard.status === 'error') {
    mainContent = <StateCard eyebrow="Локальный API" title="Не удалось загрузить объекты" action={<button className="primary-button" onClick={retryDashboard} type="button">Повторить</button>}><p>Проверьте, что локальный сервис запущен, и повторите запрос.</p></StateCard>
  } else if (dashboard.data.flats.length === 0) {
    mainContent = <StateCard eyebrow="Объекты MTS" title="Нет доступных объектов"><p>MTS ID подключён, но сервис не вернул ни одной квартиры.</p></StateCard>
  } else {
    mainContent = renderDeviceContent()
  }

  const flatOptions = dashboard.status === 'success' ? dashboard.data.flats : []
  const moreVisible = view.kind === 'destination' && destination === 'more'
  const shellHeader = <div className="shell-header-row">
    <div className="flat-wrap">
      <button className="flat-switcher" aria-expanded={flatOpen} aria-haspopup="menu" disabled={flatOptions.length === 0} onClick={() => setFlatOpen((open) => !open)} type="button">
        <span className="flat-icon"><Glyph>⌂</Glyph></span>
        <span><small>Текущий объект</small><b>{selectedFlat?.label ?? (dashboard.status === 'loading' ? 'Загрузка…' : 'Не выбран')}</b></span>
        <Glyph>⌄</Glyph>
      </button>
      {flatOpen && <div className="flat-menu" role="menu">{flatOptions.map((flat) => <button aria-current={flat.id === selectedFlatId ? 'true' : undefined} onClick={() => selectFlat(flat.id)} role="menuitem" type="button" key={flat.id}><b>{flat.label}</b><small>{flat.address}</small></button>)}</div>}
    </div>
    {selectedFlat && <span className="shell-property-address">{selectedFlat.address}</span>}
    <button className="avatar" aria-label="Профиль подключения" onClick={() => setAuthOpen(true)} type="button">М</button>
  </div>
  return <div className="app-frame">
    <AppShell destination={destination} header={shellHeader} onNavigate={selectDestination}>
      <div hidden={moreVisible}>{mainContent}</div>
      {callPollingEnabled && <div hidden={!moreVisible}>
        <MorePage
          autoOpenMutationBusy={autoOpenMutationBusy}
          autoOpenPolicy={autoOpenPolicy}
          callsApi={callsApi}
          connectionIssue={callPollingError ? 'Не удалось проверить входящие звонки' : undefined}
          getAutoOpenPolicyGeneration={getAutoOpenPolicyGeneration}
          isAutoOpenMutationCurrent={isAutoOpenMutationCurrent}
          onAutoOpenMutationEnd={endAutoOpenMutation}
          onAutoOpenMutationStart={beginAutoOpenMutation}
          onAutoOpenPolicyMutationConfirmed={confirmAutoOpenPolicyMutation}
          onOpenAuth={() => setAuthOpen(true)}
          onPolicyChange={setAutoOpenPolicy}
        />
      </div>}
      {!callPollingEnabled && moreVisible && <div className="more-page">
        <header className="screen-heading"><h1>Ещё</h1></header>
        <button className="settings-row" onClick={() => setAuthOpen(true)} type="button">MTS ID и подключение</button>
      </div>}
    </AppShell>
    <IncomingCallOverlay api={callsApi} autoOpenResult={activeCallAutoOpenResult} call={activeCurrentCall} loading={callLoading} pollingError={callPollingError} />
    {authOpen && <AuthPage api={authApi} onClose={() => setAuthOpen(false)} onAuthenticated={finishAuthentication} onLoggedOut={finishLogout} onSessionRestored={resumeRestoredSession} />}
    {doorIntent && <Modal className="door-dialog" labelledBy="door-confirm-title" onClose={closeDoorIntent} closeOnBackdrop>
      <button aria-label="Закрыть" className="dialog-close" disabled={doorExecuting} onClick={closeDoorIntent} type="button">×</button>
      <span className="dialog-icon"><Glyph>⌑</Glyph></span>
      <h2 id="door-confirm-title">Открыть дверь?</h2>
      <p>Проверьте объект и замок. Следующее нажатие отправит физическую команду домофону.</p>
      <dl className="door-summary">
        <div><dt>Объект</dt><dd><b>{doorIntent.flatLabel}</b><small>{doorIntent.flatAddress}</small></dd></div>
        <div><dt>Точка входа</dt><dd>{doorIntent.accessPointName}</dd></div>
        <div><dt>Замок</dt><dd>{doorIntent.lockName}</dd></div>
      </dl>
      <p className="door-expiry">Действует до {formatDoorExpiry(doorIntent.expiresAt)}</p>
      <div className="dialog-actions">
        <button className="secondary-button" data-dialog-initial disabled={doorExecuting} onClick={closeDoorIntent} type="button">Отмена</button>
        <button className="primary-button danger" disabled={doorExecuting} onClick={() => void executeDoor()} type="button">{doorExecuting ? 'Открываем…' : 'Подтвердить открытие'}</button>
      </div>
    </Modal>}
  </div>
}

export default App
