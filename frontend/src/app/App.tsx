import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import Hls from 'hls.js'
import { AuthPage } from '../features/auth/AuthPage'
import { authApi as defaultAuthApi, type AuthApi } from '../features/auth/authApi'
import { CallPage } from '../features/calls/CallPage'
import {
  callApi,
  startCurrentCallPolling,
  type CallApi,
  type CurrentCall,
} from '../features/calls/callApi'
import {
  IntercomApiError,
  intercomApi,
  type AccessPoint,
  type Camera,
  type DeviceInventory,
  type Flat,
  type IntercomApi,
  type Lock,
  type PropertyDashboard,
} from '../features/intercom/intercomApi'
import { Modal } from './Modal'

type Page = 'home' | 'access' | 'cameras' | 'calls' | 'events'

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
}

interface PreparedDoorIntent {
  intentId: string
  expiresAt: string
  flatLabel: string
  flatAddress: string
  accessPointName: string
  lockName: string
}

interface PreparingLock {
  accessPointId: string
  lockNumber: string
}

type DoorFeedback = { kind: 'success' | 'error'; message: string }

const navigation: ReadonlyArray<{ page: Page; label: string; glyph: string }> = [
  { page: 'home', label: 'Главная', glyph: '⌂' },
  { page: 'access', label: 'Доступ', glyph: '⌑' },
  { page: 'cameras', label: 'Камеры', glyph: '◉' },
  { page: 'calls', label: 'Звонки', glyph: '☎' },
  { page: 'events', label: 'События', glyph: '✓' },
]

const accessKindLabels: Record<string, string> = {
  barrier: 'Шлагбаум',
  door: 'Дверь',
  elevator: 'Лифт',
  gate: 'Калитка',
  intercom: 'Домофон',
}

const phaseLabelsForDashboard: Record<CurrentCall['phase'], string> = {
  ringing: 'Входящий вызов — откройте экран звонка, чтобы ответить.',
  connecting: 'Соединяем звонок с браузером.',
  active: 'Аудио и видео звонка подключены.',
  ending: 'Звонок завершается.',
}

function Glyph({ children }: { children: ReactNode }) {
  return <span className="glyph" aria-hidden="true">{children}</span>
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof IntercomApiError && error.status === 401
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

function AccessList({ accessPoints, interactionLocked, preparingLock, onPrepare }: {
  accessPoints: readonly AccessPoint[]
  interactionLocked: boolean
  preparingLock: PreparingLock | null
  onPrepare: (point: AccessPoint, lock: Lock) => void
}) {
  if (accessPoints.length === 0) {
    return <p className="empty-copy">Точек доступа для этого объекта нет</p>
  }

  return <div className="access-list">
    {accessPoints.map((point) => {
      const unavailable = !point.available || point.locks.length === 0
      const actions: Array<Lock | null> = unavailable ? [null] : point.locks
      return <div className="access-item" key={point.id}>
        <span className="access-icon"><Glyph>{point.kind === 'barrier' ? '↔' : '⌑'}</Glyph></span>
        <span className="access-copy">
          <b>{point.name}</b>
          <small><i className={point.available ? 'online' : ''} />{point.available ? 'Доступно' : 'Недоступно'} · {accessKindLabels[point.kind] ?? 'Точка входа'}</small>
        </span>
        <span className="access-actions">
          {actions.map((lock) => {
            if (!lock) {
              return <button aria-label={`Открытие ${point.name} недоступно`} className="open-button" disabled type="button" key="unavailable">Недоступно</button>
            }
            const lockName = lock.name.trim() || `Замок ${lock.number}`
            const preparing = preparingLock?.accessPointId === point.id && preparingLock.lockNumber === lock.number
            const label = point.locks.length === 1 ? 'Открыть' : `Открыть: ${lockName}`
            const accessibleLabel = point.locks.length === 1 ? `Открыть ${point.name}` : `Открыть ${point.name}: ${lockName}`
            return <button
              aria-label={accessibleLabel}
              className="open-button"
              disabled={interactionLocked}
              onClick={() => onPrepare(point, lock)}
              type="button"
              key={lock.number}
            >{preparing ? 'Подготовка…' : label}</button>
          })}
        </span>
      </div>
    })}
  </div>
}

type SnapshotRemote =
  | { status: 'loading'; requestKey: string }
  | { status: 'success'; requestKey: string; objectUrl: string }
  | { status: 'error'; requestKey: string }

function CameraSnapshot({ api, camera, flatId }: { api: IntercomApi; camera: Camera; flatId: string }) {
  const [reload, setReload] = useState(0)
  const requestKey = `${flatId}\u0000${camera.id}\u0000${reload}`
  const [snapshot, setSnapshot] = useState<SnapshotRemote>({ status: 'loading', requestKey })

  useEffect(() => {
    if (camera.capabilities.snapshot.state !== 'available') return

    const controller = new AbortController()
    let active = true
    let objectUrl: string | null = null

    api.getSnapshot(flatId, camera.id, controller.signal).then((blob) => {
      if (!active) return
      objectUrl = URL.createObjectURL(blob)
      setSnapshot({ status: 'success', requestKey, objectUrl })
    }).catch(() => {
      if (active && !controller.signal.aborted) setSnapshot({ status: 'error', requestKey })
    })

    return () => {
      active = false
      controller.abort()
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [api, camera.capabilities.snapshot.state, camera.id, flatId, reload, requestKey])

  const currentSnapshot: SnapshotRemote = snapshot.requestKey === requestKey
    ? snapshot
    : { status: 'loading', requestKey }

  if (camera.capabilities.snapshot.state !== 'available') {
    return <div className="camera-placeholder"><p>Снимки с этой камеры недоступны</p></div>
  }
  if (currentSnapshot.status === 'loading') {
    return <div className="camera-placeholder" role="status"><span className="loading-ring" /><p>Загружаем кадр…</p></div>
  }
  if (currentSnapshot.status === 'error') {
    return <div className="camera-placeholder" role="alert">
      <p>Не удалось загрузить кадр</p>
      <button className="secondary-button" onClick={() => setReload((value) => value + 1)} type="button">Повторить</button>
    </div>
  }

  return <div className="camera-snapshot">
    <img alt={`Кадр с камеры ${camera.name}`} src={currentSnapshot.objectUrl} />
    <span className="snapshot-badge">СНИМОК</span>
    <div className="camera-caption">
      <span><b>{camera.name}</b><small>Кадр получен от локального API</small></span>
      <button onClick={() => setReload((value) => value + 1)} type="button">Обновить</button>
    </div>
  </div>
}

function CameraLive({ api, camera, flatId, onStop }: { api: IntercomApi; camera: Camera; flatId: string; onStop: () => void }) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const [status, setStatus] = useState<'starting' | 'ready' | 'error'>(
    api.startLive && api.stopLive ? 'starting' : 'error',
  )

  useEffect(() => {
    const startLive = api.startLive
    const stopLive = api.stopLive
    if (!startLive || !stopLive) return
    const controller = new AbortController()
    let active = true
    let sessionId: string | null = null
    let hls: Hls | null = null

    startLive(flatId, camera.id, controller.signal).then((session) => {
      sessionId = session.sessionId
      if (!active) {
        void stopLive(session.sessionId).catch(() => undefined)
        return
      }
      const video = videoRef.current
      if (!video) return
      if (video.canPlayType('application/vnd.apple.mpegurl')) {
        video.src = session.playlistUrl
      } else if (Hls.isSupported()) {
        hls = new Hls({ enableWorker: true, lowLatencyMode: true })
        hls.loadSource(session.playlistUrl)
        hls.attachMedia(video)
      } else {
        setStatus('error')
        return
      }
      setStatus('ready')
      void video.play().catch(() => undefined)
    }).catch(() => {
      if (active && !controller.signal.aborted) setStatus('error')
    })

    return () => {
      active = false
      controller.abort()
      hls?.destroy()
      if (sessionId) void stopLive(sessionId).catch(() => undefined)
    }
  }, [api, camera.id, flatId])

  return <div className="camera-live">
    <video aria-label={`Live-видео с камеры ${camera.name}`} controls playsInline ref={videoRef} />
    <span className="live-badge"><i />LIVE</span>
    {status === 'starting' && <div className="live-status" role="status"><span className="loading-ring" />Подключаем видео…</div>}
    {status === 'error' && <div className="live-status" role="alert">Не удалось запустить видео</div>}
    <button className="live-stop-button" onClick={onStop} type="button">Остановить</button>
  </div>
}

function CameraPanel({ api, camera, flatId }: { api: IntercomApi; camera: Camera | null; flatId: string }) {
  const [liveCameraKey, setLiveCameraKey] = useState<string | null>(null)
  const cameraKey = camera ? `${flatId}:${camera.id}` : null
  const live = cameraKey !== null && liveCameraKey === cameraKey
  const liveAvailable = camera?.capabilities.live.state === 'available' && api.startLive && api.stopLive

  return <article className="camera-card">
    {camera
      ? live
        ? <CameraLive api={api} camera={camera} flatId={flatId} onStop={() => setLiveCameraKey(null)} />
        : <><CameraSnapshot api={api} camera={camera} flatId={flatId} />
          {liveAvailable && <button className="live-start-button" onClick={() => setLiveCameraKey(cameraKey)} type="button">Смотреть live</button>}
        </>
      : <div className="camera-placeholder"><p>Камер для этого объекта нет</p></div>}
  </article>
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

function UnavailablePage({ eyebrow, title, copy }: { eyebrow: string; title: string; copy: string }) {
  return <>
    <section className="page-heading"><div><p className="eyebrow">{eyebrow}</p><h1>{eyebrow}</h1></div></section>
    <section className="content-card unavailable-card">
      <span className="state-icon" aria-hidden="true">○</span>
      <h2>{title}</h2>
      <p>{copy}</p>
    </section>
  </>
}

function App({ api = intercomApi, authApi = defaultAuthApi, callsApi = callApi }: AppProps) {
  const [page, setPage] = useState<Page>('home')
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
  const [doorOutcomeUnknown, setDoorOutcomeUnknown] = useState(false)
  const [doorFeedback, setDoorFeedback] = useState<DoorFeedback | null>(null)
  const [currentCall, setCurrentCall] = useState<CurrentCall | null>(null)
  const [callLoading, setCallLoading] = useState(true)
  const [callPollingError, setCallPollingError] = useState(false)
  const [callSessionAuthenticated, setCallSessionAuthenticated] = useState<boolean | null>(null)
  const doorPreparationInFlight = useRef(false)
  const doorExecutionInFlight = useRef(false)
  const sessionEpoch = useRef(0)
  const autoOpenedCallId = useRef<string | null>(null)

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
    api.getDashboard(controller.signal).then((result) => {
      if (controller.signal.aborted || requestEpoch !== sessionEpoch.current) return
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
  }, [api, dashboardReload, dataSessionEnabled])

  useEffect(() => {
    if (!dataSessionEnabled || !selectedFlatId) return

    const flatId = selectedFlatId
    const requestEpoch = sessionEpoch.current
    const controller = new AbortController()
    api.getDevices(flatId, controller.signal).then((result) => {
      if (controller.signal.aborted || requestEpoch !== sessionEpoch.current) return
      setDevices({ status: 'success', flatId, data: result })
      setSelectedCameraId((current) => result.cameras.some((camera) => camera.id === current) ? current : (result.cameras[0]?.id ?? null))
    }).catch((error: unknown) => {
      if (!controller.signal.aborted && requestEpoch === sessionEpoch.current) setDevices({ status: 'error', flatId, error })
    })
    return () => controller.abort()
  }, [api, dataSessionEnabled, deviceReload, selectedFlatId])

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
        if (nextCall?.phase === 'ringing' && autoOpenedCallId.current !== nextCall.id) {
          autoOpenedCallId.current = nextCall.id
          setPage('calls')
          setFlatOpen(false)
        }
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

  useEffect(() => {
    if (!doorIntent || doorExecuting) return
    const expiresAt = new Date(doorIntent.expiresAt).getTime()
    if (Number.isNaN(expiresAt)) return
    const delay = Math.max(0, expiresAt - Date.now())
    const timer = window.setTimeout(() => {
      setDoorIntent(null)
      setDoorFeedback({ kind: 'error', message: 'Подтверждение истекло. Подготовьте открытие заново.' })
    }, Math.min(delay, 2_147_483_647))
    return () => window.clearTimeout(timer)
  }, [doorExecuting, doorIntent])

  const selectedFlat = useMemo<Flat | null>(() => {
    if (dashboard.status !== 'success') return null
    return dashboard.data.flats.find((flat) => flat.id === selectedFlatId) ?? null
  }, [dashboard, selectedFlatId])

  const inventory = devices.status === 'success' && devices.flatId === selectedFlatId ? devices.data : null
  const selectedCamera = inventory?.cameras.find((camera) => camera.id === selectedCameraId) ?? inventory?.cameras[0] ?? null
  const doorInteractionLocked = preparingLock !== null || doorIntent !== null || doorExecuting || doorOutcomeUnknown

  function selectPage(next: Page) {
    setPage(next)
    setFlatOpen(false)
  }

  function selectFlat(flatId: string) {
    if (flatId === selectedFlatId) {
      setFlatOpen(false)
      return
    }
    setSelectedFlatId(flatId)
    setSelectedCameraId(null)
    setDevices({ status: 'loading', flatId })
    setFlatOpen(false)
  }

  function finishAuthentication() {
    sessionEpoch.current += 1
    setDataSessionEnabled(true)
    setAuthOpen(false)
    setDoorOutcomeUnknown(false)
    setDoorFeedback(null)
    setCurrentCall(null)
    setCallLoading(true)
    setCallPollingError(false)
    setCallSessionAuthenticated(true)
    setDashboard({ status: 'loading' })
    setDashboardReload((value) => value + 1)
  }

  function finishLogout() {
    sessionEpoch.current += 1
    doorPreparationInFlight.current = false
    doorExecutionInFlight.current = false
    setDataSessionEnabled(false)
    setAuthOpen(false)
    setPage('home')
    setFlatOpen(false)
    setDashboard({ status: 'error', error: new IntercomApiError(401, 'authentication_required') })
    setSelectedFlatId(null)
    setDevices({ status: 'idle' })
    setSelectedCameraId(null)
    setPreparingLock(null)
    setDoorIntent(null)
    setDoorExecuting(false)
    setDoorOutcomeUnknown(false)
    setDoorFeedback(null)
    setCurrentCall(null)
    setCallLoading(false)
    setCallPollingError(false)
    setCallSessionAuthenticated(false)
  }

  function retryDashboard() {
    setDashboard({ status: 'loading' })
    setDashboardReload((value) => value + 1)
  }

  function retryDevices() {
    if (!selectedFlatId) return
    setDevices({ status: 'loading', flatId: selectedFlatId })
    setDeviceReload((value) => value + 1)
  }

  async function prepareDoor(flat: Flat, point: AccessPoint, lock: Lock) {
    if (doorPreparationInFlight.current || doorExecutionInFlight.current || doorIntent) return
    const operationEpoch = sessionEpoch.current
    doorPreparationInFlight.current = true
    setPreparingLock({ accessPointId: point.id, lockNumber: lock.number })
    setDoorFeedback(null)
    try {
      const intent = await api.createDoorIntent(flat.id, point.id, lock.number)
      if (operationEpoch !== sessionEpoch.current) return
      setDoorIntent({
        intentId: intent.intentId,
        expiresAt: intent.expiresAt,
        flatLabel: flat.label,
        flatAddress: flat.address,
        accessPointName: point.name,
        lockName: lock.name,
      })
    } catch {
      if (operationEpoch === sessionEpoch.current) setDoorFeedback({ kind: 'error', message: 'Не удалось подготовить открытие. Повторите попытку.' })
    } finally {
      if (operationEpoch === sessionEpoch.current) {
        doorPreparationInFlight.current = false
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
      setDoorFeedback({ kind: 'success', message: `${intent.accessPointName} открыт` })
    } catch {
      if (operationEpoch !== sessionEpoch.current) return
      setDoorIntent(null)
      setDoorOutcomeUnknown(true)
      setDoorFeedback({ kind: 'error', message: 'Результат открытия неизвестен. Не повторяйте команду сразу — сначала проверьте дверь.' })
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
    const availableCount = deviceInventory.accessPoints.filter((point) => point.available).length
    const cameraPanelKey = `${selectedFlat.id}\u0000${selectedCamera?.id ?? ''}`

    if (page === 'access') {
      return <>
        <section className="page-heading"><div><p className="eyebrow">{selectedFlat.address}</p><h1>Доступ</h1><p>Точки входа, найденные для выбранного объекта.</p></div></section>
        <section className="content-card">
          <div className="section-title-row"><div><h2>Точки доступа</h2><p>Каждое открытие требует отдельного подтверждения.</p></div><span className="count-badge">{deviceInventory.accessPoints.length}</span></div>
          <AccessList accessPoints={deviceInventory.accessPoints} interactionLocked={doorInteractionLocked} preparingLock={preparingLock} onPrepare={(point, lock) => void prepareDoor(selectedFlat, point, lock)} />
        </section>
      </>
    }

    if (page === 'cameras') {
      return <>
        <section className="page-heading"><div><p className="eyebrow">{selectedFlat.address}</p><h1>Камеры</h1><p>Снимки с камер выбранного объекта.</p></div></section>
        {deviceInventory.cameras.length > 1 && <div className="camera-tabs" aria-label="Выбор камеры" role="group">
          {deviceInventory.cameras.map((camera) => <button className={camera.id === selectedCamera?.id ? 'selected' : ''} onClick={() => setSelectedCameraId(camera.id)} type="button" key={camera.id}>{camera.name}</button>)}
        </div>}
        <section className="camera-layout">
          <CameraPanel api={api} camera={selectedCamera} flatId={selectedFlat.id} key={cameraPanelKey} />
          <aside className="content-card camera-info"><p className="eyebrow">Возможности</p><h2>{selectedCamera?.name ?? 'Камеры не найдены'}</h2><p>{selectedCamera ? 'Сейчас доступен отдельный снимок. Прямой эфир и архив не отображаются, пока их transport не подключён.' : 'Для выбранного объекта MTS не вернул камер.'}</p></aside>
        </section>
      </>
    }

    if (page === 'events') {
      return <UnavailablePage eyebrow="События" title="События пока не подключены" copy="Журнал останется пустым, пока локальный API не начнёт отдавать реальные события домофона." />
    }

    return <>
      <section className="page-heading">
        <div><p className="eyebrow">{selectedFlat.address}</p><h1>Домофон</h1><p>Реальные устройства выбранного объекта.</p></div>
        <span className="device-summary"><i className={availableCount > 0 ? 'online' : ''} />{availableCount} из {deviceInventory.accessPoints.length} точек доступны</span>
      </section>
      <section className="hero-grid" aria-label="Камера и быстрый доступ">
        <CameraPanel api={api} camera={selectedCamera} flatId={selectedFlat.id} key={cameraPanelKey} />
        <aside className="quick-panel">
          <div className="section-title-row"><div><p className="eyebrow">Выбранный объект</p><h2>Точки входа</h2></div><span className="count-badge">{deviceInventory.accessPoints.length}</span></div>
          <AccessList accessPoints={deviceInventory.accessPoints} interactionLocked={doorInteractionLocked} preparingLock={preparingLock} onPrepare={(point, lock) => void prepareDoor(selectedFlat, point, lock)} />
        </aside>
      </section>
      <section className="lower-grid">
        <article className="panel honest-status"><span className="state-icon" aria-hidden="true">☎</span><p className="eyebrow">Звонки</p><h2>{activeCurrentCall ? activeCurrentCall.callerName : 'Ожидаем входящий вызов'}</h2><p>{activeCurrentCall ? phaseLabelsForDashboard[activeCurrentCall.phase] : 'Локальный сервис проверяет только реальные входящие звонки.'}</p><button className="text-button" onClick={() => selectPage('calls')} type="button">Подробнее ›</button></article>
        <article className="panel honest-status"><span className="state-icon" aria-hidden="true">✓</span><p className="eyebrow">События</p><h2>Журнал ещё не подключён</h2><p>Экран начнёт заполняться только реальными событиями локального API.</p><button className="text-button" onClick={() => selectPage('events')} type="button">Подробнее ›</button></article>
      </section>
    </>
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
  const incomingCall = activeCurrentCall?.phase === 'ringing'

  return <div className="app-frame">
    <a className="skip-link" href="#main-content">К основному содержимому</a>
    <aside className="sidebar" aria-label="Основная навигация">
      <div className="brand"><span className="brand-mark">V</span><span className="brand-name">VDome <b>MTS</b></span></div>
      <nav className="nav-list">{navigation.map((item) => <button className={`nav-item ${page === item.page ? 'active' : ''}`} onClick={() => selectPage(item.page)} key={item.page} type="button"><Glyph>{item.glyph}</Glyph><span>{item.label}</span>{item.page === 'calls' && incomingCall && <span className="call-nav-badge">Входящий</span>}</button>)}</nav>
      <div className="sidebar-foot"><small><span className="status-dot" />Локальный API</small></div>
    </aside>
    <div className="app-body">
      <header className="topbar">
        <div className="flat-wrap">
          <button className="flat-switcher" aria-expanded={flatOpen} aria-haspopup="menu" disabled={flatOptions.length === 0} onClick={() => setFlatOpen((open) => !open)} type="button">
            <span className="flat-icon"><Glyph>⌂</Glyph></span>
            <span><small>Текущий объект</small><b>{selectedFlat?.label ?? (dashboard.status === 'loading' ? 'Загрузка…' : 'Не выбран')}</b></span>
            <Glyph>⌄</Glyph>
          </button>
          {flatOpen && <div className="flat-menu" role="menu">{flatOptions.map((flat) => <button aria-current={flat.id === selectedFlatId ? 'true' : undefined} onClick={() => selectFlat(flat.id)} role="menuitem" type="button" key={flat.id}><b>{flat.label}</b><small>{flat.address}</small></button>)}</div>}
        </div>
        <div className="top-actions"><span className="api-pill"><span />Локальный API</span><button className="avatar" aria-label="Профиль подключения" onClick={() => setAuthOpen(true)} type="button">М</button></div>
      </header>
      <main id="main-content" className="main-content">
        {callPollingEnabled && <div className="persistent-call-page" hidden={page !== 'calls'}>
          <CallPage api={callsApi} call={activeCurrentCall} loading={callLoading} pollingError={callPollingError} />
        </div>}
        {(page !== 'calls' || !callPollingEnabled) && mainContent}
      </main>
    </div>
    <nav className="mobile-nav" aria-label="Мобильная навигация">{navigation.slice(0, 4).map((item) => <button className={page === item.page ? 'active' : ''} onClick={() => selectPage(item.page)} key={item.page} type="button"><Glyph>{item.glyph}</Glyph><span>{item.label}</span>{item.page === 'calls' && incomingCall && <span className="call-nav-badge mobile-call-badge">Входящий</span>}</button>)}</nav>
    {authOpen && <AuthPage api={authApi} onClose={() => setAuthOpen(false)} onAuthenticated={finishAuthentication} onLoggedOut={finishLogout} />}
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
    {doorFeedback && <div className={`toast door-feedback ${doorFeedback.kind}`} role={doorFeedback.kind === 'success' ? 'status' : 'alert'}><span>{doorFeedback.kind === 'success' ? '✓' : '!'}</span>{doorFeedback.message}</div>}
  </div>
}

export default App
