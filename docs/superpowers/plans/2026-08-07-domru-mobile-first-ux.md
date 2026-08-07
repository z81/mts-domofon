# Dom.ru Mobile-First UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the five-section dashboard with a responsive three-destination Dom.ru-style intercom UI while preserving every real MTS camera, door, event, call, authentication, and auto-open safety contract.

**Architecture:** Keep `App` as the owner of authentication, provider data, polling, and physical-action state, but extract camera, door, shell, home, settings, and incoming-call presentation into focused components. Navigation becomes a discriminated `AppView`: three persistent destinations plus a camera detail view that remembers where to return. No Rust or local HTTP contract changes are needed.

**Tech Stack:** React 19, TypeScript 6, Vitest, Testing Library, Playwright, hls.js, existing global CSS.

## Global Constraints

- Persistent navigation contains exactly **Главная**, **События**, and **Ещё**.
- Home is the hub for the primary camera, every independently addressable door lock, armed auto-open status, and recent events.
- A physical door command still requires a short-lived intent and final confirmation naming the exact lock.
- Incoming calls appear above the current destination and retain the existing WebRTC and call-scoped unlock safety behavior.
- Camera capability failures stay honest; no mock image, video, archive, event, or call data may be substituted.
- Tap targets are at least 44 by 44 CSS pixels; color is not the only state signal.
- Do not add support, FAQ, requests, documents, theme selection, sharing, favorites, billing, or unrelated profile settings.
- Do not add dependencies or change backend routes.
- Automated and runtime verification must not open a real door, answer/reject a real call, or arm auto-open.

---

### Task 1: Extract the camera experience without changing behavior

**Files:**
- Create: `frontend/src/features/intercom/CameraExperience.tsx`
- Create: `frontend/src/features/intercom/CameraExperience.test.tsx`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: existing `IntercomApi`, `Camera`, `ArchivePlayer`, and hls.js.
- Produces: `CameraExperience`, `CameraExperienceMode`, and the same abort, object-URL, HLS, and local-session cleanup guarantees currently embedded in `App.tsx`.

- [ ] **Step 1: Write failing focused camera tests**

```tsx
render(<CameraExperience api={api} camera={camera} flatId="flat-81" presentation="preview" onOpenDetail={openDetail} />)
expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()
await user.click(screen.getByRole('button', { name: 'Смотреть Камера у подъезда' }))
expect(openDetail).toHaveBeenCalledTimes(1)

render(<CameraExperience api={api} camera={liveCamera} flatId="flat-81" presentation="detail" />)
expect(screen.getByRole('button', { name: 'Эфир' })).toBeVisible()
expect(screen.queryByRole('button', { name: 'Архив' })).not.toBeInTheDocument()
expect(api.startLive).not.toHaveBeenCalled()
```

- [ ] **Step 2: Run the focused test and verify the missing component failure**

Run: `cd frontend && npm test -- CameraExperience.test.tsx`

Expected: FAIL because `CameraExperience.tsx` does not exist.

- [ ] **Step 3: Move snapshot, live, and mode logic into the focused component**

```tsx
export type CameraExperienceMode = 'snapshot' | 'live' | 'archive'

export interface CameraExperienceProps {
  api: IntercomApi
  camera: Camera | null
  flatId: string
  presentation: 'preview' | 'detail'
  initialArchiveAt?: string
  onOpenDetail?: () => void
}

export function CameraExperience(props: CameraExperienceProps) {
  const liveAvailable = props.camera?.capabilities.live.state === 'available' && Boolean(props.api.startLive && props.api.stopLive)
  const archiveAvailable = props.camera?.capabilities.archive.state === 'available' && Boolean(props.api.getArchiveDistribution && props.api.startArchive && props.api.stopLive)
  const initialMode: CameraExperienceMode = props.initialArchiveAt && archiveAvailable ? 'archive' : 'snapshot'
  const [mode, setMode] = useState<CameraExperienceMode>(initialMode)

  if (!props.camera) return <div className="camera-empty">Камер для этого объекта нет</div>
  if (props.presentation === 'preview') return <article className="camera-preview">
    <CameraSnapshot api={props.api} camera={props.camera} flatId={props.flatId} />
    <button aria-label={`Смотреть ${props.camera.name}`} onClick={props.onOpenDetail} type="button">Смотреть</button>
  </article>

  return <article className="camera-detail">
    <div aria-label="Режим камеры" className="camera-mode-tabs" role="group">
      {liveAvailable && <button onClick={() => setMode('live')} type="button">Эфир</button>}
      <button onClick={() => setMode('snapshot')} type="button">Кадр</button>
      {archiveAvailable && <button onClick={() => setMode('archive')} type="button">Архив</button>}
    </div>
    {mode === 'live' && liveAvailable
      ? <CameraLive api={props.api} camera={props.camera} flatId={props.flatId} onStop={() => setMode('snapshot')} />
      : mode === 'archive' && archiveAvailable
        ? <ArchivePlayer api={props.api} cameraId={props.camera.id} cameraName={props.camera.name} flatId={props.flatId} initialAt={props.initialArchiveAt} />
        : <CameraSnapshot api={props.api} camera={props.camera} flatId={props.flatId} />}
  </article>
}
```

Move the current private `SnapshotRemote`, `CameraSnapshot`, and `CameraLive` implementations from `App.tsx` into this file unchanged apart from CSS class names. This retains user-gesture-only live start, audio-capable native video controls, abort on unmount, `URL.revokeObjectURL`, `hls.destroy()`, and `api.stopLive(sessionId)`.

- [ ] **Step 4: Replace the private camera functions in App and migrate their tests**

```tsx
<CameraExperience
  api={api}
  camera={selectedCamera}
  flatId={selectedFlat.id}
  presentation="preview"
  onOpenDetail={() => setPage('cameras')}
/>
```

Keep the existing live-start, camera-switch, unavailable-capability, object-URL-revocation, and archive tests; move component-specific assertions to `CameraExperience.test.tsx`.

- [ ] **Step 5: Run focused and regression tests**

Run: `cd frontend && npm test -- CameraExperience.test.tsx App.test.tsx ArchivePlayer.test.tsx`

Expected: PASS.

- [ ] **Step 6: Commit the extraction**

```bash
git add frontend/src/features/intercom/CameraExperience.tsx frontend/src/features/intercom/CameraExperience.test.tsx frontend/src/app/App.tsx frontend/src/app/App.test.tsx
git commit -m "refactor: extract camera experience"
```

### Task 2: Extract independent door actions

**Files:**
- Create: `frontend/src/features/intercom/DoorGrid.tsx`
- Create: `frontend/src/features/intercom/DoorGrid.test.tsx`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: `readonly AccessPoint[]`, the current in-flight lock identity, an optional lock-specific blocked/result state, and an App-owned prepare callback.
- Produces: one direct 44-pixel-minimum button per `Lock`, with no selector, stable accessible names, and feedback attached to the affected door.

- [ ] **Step 1: Write failing door-grid tests**

```tsx
const { rerender } = render(<DoorGrid accessPoints={[pointWithTwoLocks]} interactionLocked={false} preparingLock={null} onPrepare={onPrepare} />)
await user.click(screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' }))
expect(onPrepare).toHaveBeenCalledWith(pointWithTwoLocks, pointWithTwoLocks.locks[1])
expect(screen.queryByRole('combobox')).not.toBeInTheDocument()

rerender(<DoorGrid accessPoints={[pointWithTwoLocks]} interactionLocked={false} preparingLock={null} blockedLock={{ accessPointId: 'entrance-1', lockNumber: '2' }} feedback={{ accessPointId: 'entrance-1', lockNumber: '2', kind: 'error', message: 'Результат открытия неизвестен' }} onPrepare={onPrepare} />)
expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })).toBeDisabled()
expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Основная дверь' })).toBeEnabled()
expect(screen.getByRole('alert')).toHaveTextContent('Результат открытия неизвестен')
```

Also assert that an unavailable access point exposes a disabled button named `Открытие Подъезд 1 недоступно`.

- [ ] **Step 2: Run the focused test and verify failure**

Run: `cd frontend && npm test -- DoorGrid.test.tsx`

Expected: FAIL because `DoorGrid.tsx` does not exist.

- [ ] **Step 3: Implement the pure door grid**

```tsx
export interface PreparingLock {
  accessPointId: string
  lockNumber: string
}

export interface DoorFeedback extends PreparingLock {
  kind: 'success' | 'error'
  message: string
}

export interface DoorGridProps {
  accessPoints: readonly AccessPoint[]
  interactionLocked: boolean
  preparingLock: PreparingLock | null
  blockedLock?: PreparingLock | null
  feedback?: DoorFeedback | null
  onPrepare(point: AccessPoint, lock: Lock): void
}

export function DoorGrid({ accessPoints, interactionLocked, preparingLock, blockedLock, feedback, onPrepare }: DoorGridProps) {
  if (accessPoints.length === 0) return <p className="empty-copy">Дверей для этого объекта нет</p>
  return <div className="door-grid">{accessPoints.flatMap((point) => {
    if (!point.available || point.locks.length === 0) return [<button
      aria-label={`Открытие ${point.name} недоступно`}
      className="door-card"
      disabled
      type="button"
      key={`${point.id}:unavailable`}
    ><span>{point.name}</span><b>Недоступно</b></button>]
    return point.locks.map((lock) => {
      const lockName = lock.name.trim() || `Замок ${lock.number}`
      const label = point.locks.length === 1 ? `Открыть ${point.name}` : `Открыть ${point.name}: ${lockName}`
      const preparing = preparingLock?.accessPointId === point.id && preparingLock.lockNumber === lock.number
      const blocked = blockedLock?.accessPointId === point.id && blockedLock.lockNumber === lock.number
      const ownFeedback = feedback?.accessPointId === point.id && feedback.lockNumber === lock.number ? feedback : null
      return <div className="door-action" key={`${point.id}:${lock.number}`}>
        <button aria-label={label} className="door-card" disabled={interactionLocked || blocked} onClick={() => onPrepare(point, lock)} type="button">
          <span>{point.name}</span><b>{preparing ? 'Подготовка…' : point.locks.length === 1 ? 'Открыть' : lockName}</b>
        </button>
        {ownFeedback && <p role={ownFeedback.kind === 'error' ? 'alert' : 'status'}>{ownFeedback.message}</p>}
      </div>
    })
  })}</div>
}
```

- [ ] **Step 4: Replace `AccessList` in App while preserving intent orchestration**

```tsx
<DoorGrid
  accessPoints={inventory.accessPoints}
  interactionLocked={doorInteractionLocked}
  preparingLock={preparingLock}
  blockedLock={blockedLock}
  feedback={doorFeedback}
  onPrepare={(point, lock) => void prepareDoor(selectedFlat, point, lock)}
/>
```

Keep `prepareDoor`, `executeDoor`, epoch guards, expiry handling, and the exact-lock confirmation dialog in App. Replace the global `doorOutcomeUnknown` boolean with `blockedLock: PreparingLock | null`, and include `accessPointId` plus `lockNumber` in `DoorFeedback`. A lost/ambiguous execute blocks only that exact lock until flat/session reset; other owned locks stay usable. Remove the detached global door toast after feedback renders beside its door.

- [ ] **Step 5: Run door safety regressions**

Run: `cd frontend && npm test -- DoorGrid.test.tsx App.test.tsx`

Expected: PASS, including double-click suppression, cancel, expiry, exact-lock lost-response lockout, another-lock availability, and logout cleanup.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/features/intercom/DoorGrid.tsx frontend/src/features/intercom/DoorGrid.test.tsx frontend/src/app/App.tsx frontend/src/app/App.test.tsx
git commit -m "refactor: extract direct door actions"
```

### Task 3: Introduce the three-destination shell and home hub

**Files:**
- Create: `frontend/src/app/AppShell.tsx`
- Create: `frontend/src/app/AppShell.test.tsx`
- Create: `frontend/src/features/home/HomePage.tsx`
- Create: `frontend/src/features/home/HomePage.test.tsx`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: App-owned header, current destination, optional armed banner, and route content.
- Produces: `Destination`, `AppView`, three persistent nav actions, and a home layout composed from real feature nodes.

- [ ] **Step 1: Write shell and home tests**

```tsx
render(<AppShell destination="home" onNavigate={onNavigate} header={<span>Квартира 81</span>}><p>Контент</p></AppShell>)
expect(screen.getAllByRole('button').filter((button) => ['Главная', 'События', 'Ещё'].includes(button.textContent ?? ''))).toHaveLength(3)
expect(screen.queryByText('Доступ')).not.toBeInTheDocument()
expect(screen.queryByText('Звонки')).not.toBeInTheDocument()

render(<HomePage camera={<div>Камера</div>} doors={<div>Двери</div>} activity={<div>События</div>} autoOpen={null} />)
expect(screen.getByRole('heading', { name: 'Главная' })).toBeVisible()
expect(screen.getByRole('heading', { name: 'Камеры' })).toBeVisible()
expect(screen.getByRole('heading', { name: 'Двери' })).toBeVisible()
```

- [ ] **Step 2: Run tests and verify failure**

Run: `cd frontend && npm test -- AppShell.test.tsx HomePage.test.tsx`

Expected: FAIL because the two components do not exist.

- [ ] **Step 3: Implement route and shell types**

```tsx
export type Destination = 'home' | 'events' | 'more'
export type AppView =
  | { kind: 'destination'; destination: Destination }
  | { kind: 'camera'; returnTo: Destination; archiveTarget?: { cameraId: string; archiveAt: string } }

const destinations = [
  { id: 'home', label: 'Главная' },
  { id: 'events', label: 'События' },
  { id: 'more', label: 'Ещё' },
] as const
```

`AppShell` renders a compact property header, centered content, optional status strip, and the same three controls in responsive desktop/mobile navigation. Use inline SVG with `aria-hidden="true"`; the visible Russian labels remain the accessible names.

- [ ] **Step 4: Implement the compositional home page**

```tsx
export interface HomePageProps {
  camera: ReactNode
  doors: ReactNode
  autoOpen: ReactNode | null
  activity: ReactNode
  onOpenAllEvents(): void
}

export function HomePage(props: HomePageProps) {
  return <div className="home-page">
    <header className="screen-heading"><h1>Главная</h1></header>
    <section className="home-section home-camera"><header><h2>Камеры</h2></header>{props.camera}</section>
    <section className="home-section home-doors"><header><h2>Двери</h2></header>{props.doors}</section>
    {props.autoOpen}
    <section className="home-section home-events"><header><h2>Последние события</h2><button onClick={props.onOpenAllEvents}>Все события</button></header>{props.activity}</section>
  </div>
}
```

- [ ] **Step 5: Replace old page state and dashboard sections in App**

```tsx
const [view, setView] = useState<AppView>({ kind: 'destination', destination: 'home' })
const destination = view.kind === 'destination' ? view.destination : view.returnTo

function selectDestination(next: Destination) {
  setView({ kind: 'destination', destination: next })
  setFlatOpen(false)
}
```

Remove the persistent `Доступ`, `Камеры`, and `Звонки` routes, sidebar, local-API pills, duplicate call-status card, and five-item mobile nav. Pass the existing compact flat switcher as the `AppShell` header. Compose preview camera, `DoorGrid`, armed strip, and compact `EventJournal` into `HomePage`. A camera detail view renders `CameraExperience presentation="detail"` with a `Назад` action and camera switcher.

- [ ] **Step 6: Update App integration assertions**

Add assertions for exactly three persistent destinations, home-first camera and doors, navigation to camera detail, Back returning to the originating destination, and flat switching retaining safe reload behavior. Remove assertions tied only to the old complementary sidebar and Calls page.

- [ ] **Step 7: Run the shell/home regression set**

Run: `cd frontend && npm test -- AppShell.test.tsx HomePage.test.tsx App.test.tsx`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/app/AppShell.tsx frontend/src/app/AppShell.test.tsx frontend/src/features/home/HomePage.tsx frontend/src/features/home/HomePage.test.tsx frontend/src/app/App.tsx frontend/src/app/App.test.tsx
git commit -m "feat: add Dom.ru-style home navigation"
```

### Task 4: Consolidate Events and More

**Files:**
- Create: `frontend/src/features/settings/MorePage.tsx`
- Create: `frontend/src/features/settings/MorePage.test.tsx`
- Modify: `frontend/src/features/events/EventJournal.tsx`
- Modify: `frontend/src/features/events/EventJournal.test.tsx`
- Modify: `frontend/src/features/calls/AutoOpenPanel.tsx`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: `AuthApi` modal trigger, `CallApi`, `AutoOpenPolicy`, `EventApi`, selected flat, and camera archive navigation callback.
- Produces: one chronological Events destination, one narrowly scoped More destination, and a direct armed-policy disable action.

- [ ] **Step 1: Write failing More and consolidated Events tests**

```tsx
render(<MorePage callsApi={callsApi} onOpenAuth={onOpenAuth} onPolicyChange={onPolicyChange} />)
expect(screen.getByRole('heading', { name: 'Ещё' })).toBeVisible()
expect(screen.getByRole('button', { name: 'MTS ID и подключение' })).toBeVisible()
expect(screen.getByRole('heading', { name: /Автооткрытие/ })).toBeVisible()
expect(screen.queryByText(/FAQ|Документы|Тема|Избранное/)).not.toBeInTheDocument()
```

Extend `EventJournal.test.tsx` to assert visible filters **Все**, **Звонки**, **Доступ**, **Камеры**, readable outcome text, `Показать ещё`, and archive navigation only for an event with both `cameraId` and `archiveAt`.

- [ ] **Step 2: Run the focused tests and verify failure**

Run: `cd frontend && npm test -- MorePage.test.tsx EventJournal.test.tsx`

Expected: FAIL because `MorePage` is missing and event copy/roles do not yet match.

- [ ] **Step 3: Implement More with existing safe auto-open controls**

```tsx
export interface MorePageProps {
  callsApi: CallApi
  onOpenAuth(): void
  onPolicyChange(policy: AutoOpenPolicy): void
  connectionIssue?: string
}

export function MorePage({ callsApi, onOpenAuth, onPolicyChange, connectionIssue }: MorePageProps) {
  return <div className="more-page">
    <header className="screen-heading"><h1>Ещё</h1></header>
    <button className="settings-row" onClick={onOpenAuth} type="button">MTS ID и подключение</button>
    {connectionIssue && <p className="connection-issue" role="status">{connectionIssue}</p>}
    <AutoOpenPanel api={callsApi} onPolicyChange={onPolicyChange} />
  </div>
}
```

Keep `MorePage` mounted in App while visually hidden so `AutoOpenPanel` continues its existing five-second bounded policy refresh and the global armed state remains current.

- [ ] **Step 4: Simplify full and compact EventJournal presentation**

Use a screen heading **События**, filter labels from the specification, 30-day owned data, date grouping, title/source/outcome/time, and the existing bounded `Показать ещё`. Remove the standalone call-history variant from App; call events live under the **Звонки** filter.

- [ ] **Step 5: Wire direct armed-policy disable and archive return routing**

```tsx
const autoOpenDisabling = useRef(false)
const [autoOpenDisableFailed, setAutoOpenDisableFailed] = useState(false)
const [autoOpenDisableBusy, setAutoOpenDisableBusy] = useState(false)

async function disableAutoOpenFromHome() {
  if (!callsApi.disableAutoOpen || autoOpenDisabling.current) return
  autoOpenDisabling.current = true
  setAutoOpenDisableBusy(true)
  setAutoOpenDisableFailed(false)
  try {
    await callsApi.disableAutoOpen()
    setAutoOpenPolicy({ mode: 'disabled', dismissAfterOpen: false })
  } catch {
    setAutoOpenDisableFailed(true)
  } finally {
    autoOpenDisabling.current = false
    setAutoOpenDisableBusy(false)
  }
}

function openEventArchive(cameraId: string, archiveAt: string) {
  if (!inventory?.cameras.some((camera) => camera.id === cameraId)) return
  setSelectedCameraId(cameraId)
  setView({ kind: 'camera', returnTo: 'events', archiveTarget: { cameraId, archiveAt } })
}

const autoOpenStrip = autoOpenPolicy?.mode !== 'disabled'
  ? <section aria-label="Автооткрытие включено" className="auto-open-strip" role="status">
      <span>{formatAutoOpenBanner(autoOpenPolicy)}</span>
      <button disabled={autoOpenDisableBusy} onClick={() => void disableAutoOpenFromHome()} type="button">Выключить</button>
      {autoOpenDisableFailed && <span role="alert">Не удалось выключить автооткрытие</span>}
    </section>
  : <button className="auto-open-entry" onClick={() => selectDestination('more')} type="button">Автооткрытие</button>
```

Display a stable retryable error if direct disable fails; never imply the rule was disabled before the local API confirms it.

- [ ] **Step 6: Run Events, More, auto-open, and App tests**

Run: `cd frontend && npm test -- MorePage.test.tsx EventJournal.test.tsx AutoOpenPanel.test.tsx App.test.tsx`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/features/settings/MorePage.tsx frontend/src/features/settings/MorePage.test.tsx frontend/src/features/events/EventJournal.tsx frontend/src/features/events/EventJournal.test.tsx frontend/src/features/calls/AutoOpenPanel.tsx frontend/src/app/App.tsx frontend/src/app/App.test.tsx
git commit -m "feat: consolidate events and intercom settings"
```

### Task 5: Present incoming calls above navigation

**Files:**
- Create: `frontend/src/features/calls/IncomingCallOverlay.tsx`
- Create: `frontend/src/features/calls/IncomingCallOverlay.test.tsx`
- Modify: `frontend/src/features/calls/CallPage.tsx`
- Modify: `frontend/src/features/calls/CallPage.test.tsx`
- Modify: `frontend/src/app/Modal.tsx`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: the current owned `CurrentCall`, existing `CallApi`, polling error/loading state, and `CallPage` WebRTC lifecycle.
- Produces: a non-dismissible call overlay, `CallPage` embedded presentation, and explicit microphone mute state.

- [ ] **Step 1: Write failing overlay and microphone tests**

```tsx
render(<IncomingCallOverlay api={api} call={ringingCall} />)
expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBeVisible()
expect(screen.getByRole('button', { name: 'Принять вызов' })).toBeVisible()
expect(screen.queryByRole('navigation')).not.toBeInTheDocument()

render(<CallPage api={api} call={ringingCall} presentation="embedded" getUserMedia={getUserMedia} createPeerConnection={createPeerConnection} />)
await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
expect(await screen.findByRole('button', { name: 'Выключить микрофон' })).toBeVisible()
await user.click(screen.getByRole('button', { name: 'Выключить микрофон' }))
expect(localAudioTrack.enabled).toBe(false)
expect(screen.getByRole('button', { name: 'Включить микрофон' })).toBeVisible()
```

- [ ] **Step 2: Run tests and verify failure**

Run: `cd frontend && npm test -- IncomingCallOverlay.test.tsx CallPage.test.tsx`

Expected: FAIL because overlay and `presentation` do not exist.

- [ ] **Step 3: Make Modal optionally non-dismissible**

```tsx
interface ModalProps {
  children: ReactNode
  labelledBy: string
  onClose: () => void
  className?: string
  closeOnBackdrop?: boolean
  dismissible?: boolean
}

export function Modal({ children, labelledBy, onClose, className = '', closeOnBackdrop = false, dismissible = true }: ModalProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const restoreFocusRef = useRef<HTMLElement | null>(null)
  useEffect(() => {
    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const dialog = dialogRef.current
    const initial = dialog?.querySelector<HTMLElement>('[data-dialog-initial]') ?? dialog?.querySelector<HTMLElement>(focusableSelector)
    initial?.focus()
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        if (!dismissible) return
        event.preventDefault()
        onClose()
        return
      }
      if (event.key !== 'Tab' || !dialog) return
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector))
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      restoreFocusRef.current?.focus()
    }
  }, [dismissible, onClose])
  return <div className="dialog-backdrop" onMouseDown={(event) => {
    if (dismissible && closeOnBackdrop && event.currentTarget === event.target) onClose()
  }}>
    <section ref={dialogRef} className={`confirm-dialog ${className}`.trim()} role="dialog" aria-modal="true" aria-labelledby={labelledBy}>{children}</section>
  </div>
}
```

Keep focus trapping and focus restoration. Existing door and auto-open confirmations retain `dismissible=true`.

- [ ] **Step 4: Add embedded CallPage and microphone control**

```tsx
export interface CallPageProps {
  api?: CallApi
  call: CurrentCall | null
  loading?: boolean
  pollingError?: boolean
  createPeerConnection?: () => RTCPeerConnection
  getUserMedia?: (constraints: MediaStreamConstraints) => Promise<MediaStream>
  iceGatheringTimeoutMs?: number
  presentation?: 'page' | 'embedded'
}

function toggleMicrophone() {
  setMicrophoneMuted((muted) => {
    localStreamRef.current?.getAudioTracks().forEach((track) => { track.enabled = muted })
    return !muted
  })
}
```

Set `microphoneReady` after `getUserMedia` returns an audio track, reset it in `cleanupMedia`, and render the mute button only while ready. `presentation="embedded"` removes only the redundant page heading; it preserves video, answer/decline/ignore/hangup, call-scoped open intent confirmation, DTMF best effort, unknown-outcome lockout, and media cleanup.

- [ ] **Step 5: Implement the overlay and wire it outside destination content**

```tsx
export function IncomingCallOverlay(props: Pick<CallPageProps, 'api' | 'call' | 'loading' | 'pollingError'>) {
  if (!props.call) return null
  return <Modal labelledBy="incoming-call-title" onClose={ignoreDismiss} dismissible={false} className="incoming-call-dialog">
    <h1 id="incoming-call-title" className="visually-hidden">Входящий вызов</h1>
    <CallPage {...props} presentation="embedded" />
  </Modal>
}
```

Define `const ignoreDismiss = () => undefined` at module scope. Render this sibling after `AppShell`, so an incoming/running call is visible above Home, Events, More, or camera detail without mutating the saved destination. When the call disappears, the overlay unmounts and the prior destination is unchanged.

- [ ] **Step 6: Update call integration tests**

Replace old “switches to Calls page” assertions with overlay priority, retained destination, overlay persistence through call phases, teardown after logout, and no invented UI when `getCurrent` returns `null`.

- [ ] **Step 7: Run call and App regressions**

Run: `cd frontend && npm test -- IncomingCallOverlay.test.tsx CallPage.test.tsx App.test.tsx`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/features/calls/IncomingCallOverlay.tsx frontend/src/features/calls/IncomingCallOverlay.test.tsx frontend/src/features/calls/CallPage.tsx frontend/src/features/calls/CallPage.test.tsx frontend/src/app/Modal.tsx frontend/src/app/App.tsx frontend/src/app/App.test.tsx
git commit -m "feat: surface incoming calls above navigation"
```

### Task 6: Apply the responsive Dom.ru visual system and verify the finished UX

**Files:**
- Modify: `frontend/src/app/styles.css`
- Modify: `frontend/e2e/core-live.spec.ts`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: the semantic classes and state attributes introduced by Tasks 1–5.
- Produces: a white/coral mobile-first UI, stable media geometry, desktop two-column home layout, safe-area-aware navigation, and end-to-end evidence for both desktop and mobile projects.

- [ ] **Step 1: Extend E2E assertions before styling**

```ts
await expect(page.getByRole('navigation', { name: 'Основная навигация' })).toBeVisible()
await expect(page.getByRole('button', { name: 'Главная' })).toBeVisible()
await expect(page.getByRole('button', { name: 'События' })).toBeVisible()
await expect(page.getByRole('button', { name: 'Ещё' })).toBeVisible()
await expect(page.getByRole('button', { name: 'Доступ' })).toHaveCount(0)
await expect(page.getByRole('button', { name: 'Открыть Подъезд 1' })).toHaveCSS('min-height', '56px')
```

Keep the mocked intent/execute route and exact assertion that execute count remains zero until the final confirmation.
Change the mocked intent expiry to `new Date(Date.now() + 60_000).toISOString()` and update the old **Домофон** page-heading assertion to **Главная** so the E2E test does not depend on wall-clock date or removed copy.

- [ ] **Step 2: Run E2E and verify the visual-contract assertion fails**

Run: `cd frontend && npm run e2e -- --project=chromium --project=mobile-chromium`

Expected: FAIL until the new shell styling and minimum door target size are present.

- [ ] **Step 3: Replace dashboard CSS with the mobile-first token system**

```css
:root {
  color: #171a2b;
  background: #f5f5f8;
  font-family: Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --accent: #e30613;
  --accent-soft: #fff0f1;
  --surface: #ffffff;
  --line: #e8e7ee;
  --muted: #686a78;
  --radius-card: 16px;
  --shadow-card: 0 8px 28px rgb(30 24 55 / 8%);
}

.bottom-nav button,
.door-card,
.camera-action,
.settings-row { min-height: 56px; }

.media-frame { aspect-ratio: 16 / 9; background: #171a2b; overflow: hidden; }

@media (min-width: 900px) {
  .home-page { display: grid; grid-template-columns: minmax(0, 1.45fr) minmax(320px, .75fr); gap: 24px; }
  .home-camera { grid-row: span 2; }
}

@media (max-width: 899px) {
  .app-content { padding: 16px 16px calc(88px + env(safe-area-inset-bottom)); }
  .bottom-nav { padding-bottom: env(safe-area-inset-bottom); }
}
```

Style selected nav with text plus coral color, cards with pale borders and restrained shadow, 16-pixel mobile spacing, readable 14-pixel secondary text, 20–28-pixel headings, stable loading/media dimensions, horizontal camera/filter scrollers, full-screen mobile call overlay, and a bounded desktop call dialog. Remove all unused `.sidebar`, five-route, old hero-dashboard, and microcopy styles after `rg` confirms no consumers.

- [ ] **Step 4: Add DOM-level layout invariants**

In `App.test.tsx`, assert no hidden fourth destination, the call dialog comes after the application shell, icon-only controls have Russian `aria-label`s, and unavailable/error states retain text labels rather than color-only dots.

- [ ] **Step 5: Run all frontend quality gates**

Run:

```bash
cd frontend
npm test
npm run typecheck
npm run lint
npm run build
npm run e2e -- --project=chromium --project=mobile-chromium
```

Expected: every command exits 0.

- [ ] **Step 6: Run repository-wide regression checks**

Run from repository root:

```bash
cargo fmt --check
cargo test --workspace
git diff --check
git status --short
```

Expected: format check and tests exit 0; status lists only the intended UX changes before commit.

- [ ] **Step 7: Perform read-only runtime smoke**

Start or reuse the local service, then load only dashboard, device, snapshot, event, archive-availability, and media-manifest reads. Confirm the UI contains three destinations and the real provider-unavailable archive message when MTS reports no archive. Do not click a door confirmation, answer/reject a call, or enable auto-open.

- [ ] **Step 8: Commit the visual system and final checks**

```bash
git add frontend/src/app/styles.css frontend/e2e/core-live.spec.ts frontend/src/app/App.test.tsx
git commit -m "feat: finish responsive Dom.ru intercom UX"
```
