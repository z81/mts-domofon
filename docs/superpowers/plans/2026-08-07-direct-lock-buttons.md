# Direct Lock Buttons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the multi-lock selector with one immediately actionable, named button per MTS lock.

**Architecture:** Keep the backend and confirmation intent flow unchanged. `AccessList` renders every lock as its own button and passes that exact `Lock` to the existing `onPrepare` callback; App state records the point and lock currently preparing so only the pressed button displays progress.

**Tech Stack:** React 19, TypeScript, Testing Library, Vitest, CSS, existing Rust/Axum server.

## Global Constraints

- Every real lock gets its own button; never silently select the first lock.
- Multiple-lock labels use `Открыть: {lock name}` with `Замок {number}` for a blank name.
- A single lock keeps the compact visible label `Открыть`.
- The existing explicit confirmation dialog remains the only action that executes physical opening.
- Do not modify backend routes, provider calls, retries, or dependencies.
- Automated and smoke verification must not create or execute a real door-open intent.

---

### Task 1: Render and track direct lock actions

**Files:**
- Modify: `frontend/src/app/App.test.tsx:386`
- Modify: `frontend/src/app/App.tsx:118-165,330,446,490,512-535,587,618`
- Modify: `frontend/src/app/styles.css:111,256-257,318-320`

**Interfaces:**
- Consumes: existing `AccessPoint`, `Lock`, `onPrepare(point, lock)`, `createDoorIntent(flatId, accessPointId, lockNumber)`, and confirmation modal.
- Produces: direct lock buttons and `PreparingLock = { accessPointId: string; lockNumber: string }` state behind unchanged API calls.

- [ ] **Step 1: Write the failing two-lock UI test**

Replace the current selector-based test setup with literal expectations:

```tsx
it('renders one direct button per lock and executes only after explicit confirmation', async () => {
  const user = userEvent.setup()
  const createDoorIntent = vi.fn<IntercomApi['createDoorIntent']>(async () => ({
    intentId: 'intent / 1',
    expiresAt: new Date(Date.now() + 60_000).toISOString(),
  }))
  const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>(async () => undefined)
  const multipleLocks: DeviceInventory = {
    ...inventory,
    accessPoints: inventory.accessPoints.map((point) => ({
      ...point,
      locks: [
        { number: '1', name: 'Основная дверь', permanentlyOpen: false },
        { number: '2', name: 'Вторая дверь', permanentlyOpen: false },
      ],
    })),
  }
  render(<App api={createApi({ createDoorIntent, executeDoorIntent, getDevices: async () => multipleLocks })} />)

  expect(await screen.findByRole('button', { name: 'Открыть Подъезд 1: Основная дверь' })).toBeEnabled()
  const secondLock = screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
  await user.click(secondLock)

  expect(createDoorIntent).toHaveBeenCalledWith('flat-81', 'entrance-1', '2')
  expect(executeDoorIntent).not.toHaveBeenCalled()
  const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь?' })
  expect(within(dialog).getByText('Вторая дверь')).toBeVisible()
  await user.click(within(dialog).getByRole('button', { name: 'Подтвердить открытие' }))
  expect(executeDoorIntent).toHaveBeenCalledWith('intent / 1')
})
```

- [ ] **Step 2: Run RED**

Run:

```bash
npm --prefix frontend test -- --run frontend/src/app/App.test.tsx
```

If Vitest interprets the root-relative path differently, run `npm --prefix frontend test -- --run src/app/App.test.tsx`. Expected: FAIL because no per-lock buttons exist and the combobox is still rendered.

- [ ] **Step 3: Replace selector state with direct lock buttons**

Add the state shape near the component types:

```tsx
type PreparingLock = { accessPointId: string; lockNumber: string }
```

Change `AccessList` to accept `preparingLock: PreparingLock | null`, remove `selectedLocks`, and render:

```tsx
const unavailable = !point.available || point.locks.length === 0
const actions = unavailable ? [null] : point.locks

{actions.map((lock) => {
  if (!lock) {
    return <button aria-label={`Открытие ${point.name} недоступно`} className="open-button" disabled type="button" key="unavailable">Недоступно</button>
  }
  const lockName = lock.name.trim() || `Замок ${lock.number}`
  const preparing = preparingLock?.accessPointId === point.id && preparingLock.lockNumber === lock.number
  const visibleLabel = point.locks.length === 1 ? 'Открыть' : `Открыть: ${lockName}`
  const accessibleLabel = point.locks.length === 1 ? `Открыть ${point.name}` : `Открыть ${point.name}: ${lockName}`
  return <button
    aria-label={accessibleLabel}
    className="open-button"
    disabled={interactionLocked}
    onClick={() => onPrepare(point, lock)}
    type="button"
    key={lock.number}
  >{preparing ? 'Подготовка…' : visibleLabel}</button>
})}
```

Rename App state and every prop/reset use:

```tsx
const [preparingLock, setPreparingLock] = useState<PreparingLock | null>(null)
const doorInteractionLocked = preparingLock !== null || doorIntent !== null || doorExecuting || doorOutcomeUnknown

setPreparingLock({ accessPointId: point.id, lockNumber: lock.number })
// logout and finally:
setPreparingLock(null)
```

- [ ] **Step 4: Update direct-button layout**

Remove `.lock-select` rules. Allow actions to wrap while staying right-aligned:

```css
.access-actions { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 8px; }

@media (max-width: 700px) {
  .access-actions { grid-column: 1 / -1; width: 100%; }
  .access-actions .open-button { width: 100%; min-width: 110px; margin-bottom: 0; }
}
```

- [ ] **Step 5: Run focused and complete frontend checks**

Run:

```bash
npm --prefix frontend test -- --run src/app/App.test.tsx
npm --prefix frontend test -- --run
npm --prefix frontend run lint
npm --prefix frontend run typecheck
npm --prefix frontend run build
```

Expected: 77 or more tests pass; lint, typecheck, and build exit zero.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/app/App.tsx frontend/src/app/App.test.tsx frontend/src/app/styles.css
git commit -m "feat: add direct lock buttons"
```

### Task 2: Safe local handoff

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: rebuilt `frontend/dist` and the existing local `ServeDir` runtime.
- Produces: the updated UI at `http://127.0.0.1:3000` without any physical door command.

- [ ] **Step 1: Verify the server and static build**

Run an exact loopback listener check and GET `/`; confirm the built HTML references the new hashed frontend asset. Do not launch a browser.

- [ ] **Step 2: Confirm clean state**

```bash
git status --short
git log -3 --oneline
```

Expected: only the design, plan, and implementation commits are new; no secret-bearing files are tracked.
