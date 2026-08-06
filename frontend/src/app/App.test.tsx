import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AuthApi } from '../features/auth/authApi'
import type { CallApi, CurrentCall } from '../features/calls/callApi'
import {
  IntercomApiError,
  type DeviceInventory,
  type IntercomApi,
  type PropertyDashboard,
} from '../features/intercom/intercomApi'
import App from './App'

const available = { state: 'available' } as const
const unavailable = { state: 'unavailable', reason: null } as const

const dashboard: PropertyDashboard = {
  flats: [
    {
      id: 'flat-81',
      label: 'Квартира 81',
      address: 'Москва, ул. Примерная, 7',
      capabilities: {
        accessPoints: available,
        cameras: available,
        archive: unavailable,
        events: unavailable,
        calls: unavailable,
        callSettings: unavailable,
      },
    },
    {
      id: 'flat / 22',
      label: 'Квартира 22',
      address: 'Химки, Лесная, 4',
      capabilities: {
        accessPoints: available,
        cameras: available,
        archive: unavailable,
        events: unavailable,
        calls: unavailable,
        callSettings: unavailable,
      },
    },
  ],
  selectedFlatId: 'flat-81',
  callSettings: { state: 'unavailable' },
}

const inventory: DeviceInventory = {
  accessPoints: [{
    id: 'entrance-1',
    flatId: 'flat-81',
    name: 'Подъезд 1',
    kind: 'intercom',
    locks: [{ number: '1', name: 'Дверь', permanentlyOpen: false }],
    cameraIds: ['camera-1'],
    available: true,
  }],
  cameras: [{
    id: 'camera-1',
    flatId: 'flat-81',
    name: 'Камера у подъезда',
    accessPointId: 'entrance-1',
    capabilities: { snapshot: available, live: unavailable, archive: unavailable },
  }],
}

const secondInventory: DeviceInventory = {
  accessPoints: [{
    id: 'gate-2',
    flatId: 'flat / 22',
    name: 'Калитка',
    kind: 'gate',
    locks: [],
    cameraIds: ['camera / 2'],
    available: false,
  }],
  cameras: [{
    id: 'camera / 2',
    flatId: 'flat / 22',
    name: 'Камера у калитки',
    accessPointId: 'gate-2',
    capabilities: { snapshot: available, live: unavailable, archive: unavailable },
  }],
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve
    reject = promiseReject
  })
  return { promise, reject, resolve }
}

function createApi(overrides: Partial<IntercomApi> = {}): IntercomApi {
  return {
    createDoorIntent: async () => ({ intentId: 'intent-1', expiresAt: new Date(Date.now() + 60_000).toISOString() }),
    executeDoorIntent: async () => undefined,
    getDashboard: async () => dashboard,
    getDevices: async () => inventory,
    getSnapshot: async () => new Blob(['snapshot'], { type: 'image/webp' }),
    ...overrides,
  }
}

const ringingCall: CurrentCall = {
  id: 'call-81',
  revision: 3,
  phase: 'ringing',
  callerName: 'Домофон у подъезда',
  startedAt: '2026-08-06T12:00:00Z',
  remoteSdp: 'v=0\r\na=ice-ufrag:remote\r\n',
  canOpen: true,
}

function createCallApi(overrides: Partial<CallApi> = {}): CallApi {
  return {
    answer: async () => ({ ...ringingCall, phase: 'connecting', revision: 4 }),
    createOpenIntent: async () => ({ intentId: 'call-intent-1', expiresAt: new Date(Date.now() + 60_000).toISOString() }),
    decline: async () => undefined,
    executeOpenIntent: async () => undefined,
    getCurrent: async () => null,
    hangup: async () => undefined,
    ignore: async () => undefined,
    ...overrides,
  }
}

function anonymousAuthApi(): AuthApi {
  return {
    completeCallback: async () => { throw new Error('not used') },
    confirmLegacySms: async () => { throw new Error('not used') },
    getSession: async () => ({ authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null }),
    logout: async () => undefined,
    startAuth: async () => ({
      status: 'browser_redirect',
      flow_id: 'flow-1',
      authorization_url: 'https://login.mts.ru/amserver/UI/Login?realm=users',
    }),
    startLegacySms: async () => { throw new Error('not used') },
    submitChallenge: async () => { throw new Error('not used') },
  }
}

function authenticatedAuthApi(overrides: Partial<AuthApi> = {}): AuthApi {
  return {
    ...anonymousAuthApi(),
    getSession: async () => ({ authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' }),
    ...overrides,
  }
}

let objectUrlIndex = 0

beforeEach(() => {
  objectUrlIndex = 0
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    value: vi.fn(() => `blob:snapshot-${++objectUrlIndex}`),
  })
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    value: vi.fn(),
  })
})

describe('live intercom dashboard', () => {
  it('keeps a loading state until the real dashboard request settles', async () => {
    const pending = deferred<PropertyDashboard>()
    render(<App api={createApi({ getDashboard: () => pending.promise })} />)

    expect(screen.getByRole('status')).toHaveTextContent('Загружаем объекты')
    expect(screen.queryByText('Квартира 81')).not.toBeInTheDocument()

    await act(async () => pending.resolve(dashboard))
    expect(await screen.findByText('Квартира 81')).toBeVisible()
  })

  it('renders linked flats and discovered devices without synthetic product claims', async () => {
    render(<App api={createApi()} />)

    expect(await screen.findByRole('heading', { name: 'Домофон' })).toBeVisible()
    expect(await screen.findByText('Подъезд 1')).toBeVisible()
    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-1')
    expect(screen.getByText('Москва, ул. Примерная, 7')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1' })).toBeEnabled()
    expect(document.body).not.toHaveTextContent(/preview|демо|синтетическ/i)
    expect(screen.queryByText('Поддержка')).not.toBeInTheDocument()
    expect(screen.queryByText('Документы')).not.toBeInTheDocument()
    expect(screen.queryByText('Избранное')).not.toBeInTheDocument()
  })

  it('offers MTS ID authentication when the dashboard returns 401', async () => {
    const user = userEvent.setup()
    const api = createApi({
      getDashboard: async () => { throw new IntercomApiError(401, 'authentication_required') },
    })
    render(<App api={api} authApi={anonymousAuthApi()} />)

    expect(await screen.findByRole('heading', { name: 'Войдите в MTS ID' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByRole('dialog', { name: 'Вход в VDome' })).toBeVisible()
    expect(screen.getByText('Локальное подключение')).toBeVisible()
  })

  it('reloads devices after authentication even when the selected flat id stays the same', async () => {
    const user = userEvent.setup()
    const getDevices = vi.fn<IntercomApi['getDevices']>()
      .mockResolvedValueOnce(inventory)
      .mockResolvedValueOnce({
        ...inventory,
        accessPoints: inventory.accessPoints.map((point) => ({ ...point, name: 'Обновлённый подъезд' })),
      })
    const authApi = anonymousAuthApi()
    authApi.startAuth = async () => ({
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T11:00:00Z', expires_at: null },
    })
    render(<App api={createApi({ getDevices })} authApi={authApi} />)
    expect(await screen.findByText('Подъезд 1')).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))

    expect(await screen.findByText('Обновлённый подъезд')).toBeVisible()
    expect(getDevices).toHaveBeenCalledTimes(2)
  })

  it('shows an empty account without requesting devices', async () => {
    const getDevices = vi.fn<IntercomApi['getDevices']>()
    render(<App api={createApi({
      getDashboard: async () => ({ flats: [], selectedFlatId: null, callSettings: { state: 'unavailable' } }),
      getDevices,
    })} />)

    expect(await screen.findByRole('heading', { name: 'Нет доступных объектов' })).toBeVisible()
    expect(getDevices).not.toHaveBeenCalled()
  })

  it('shows a retryable dashboard error and retries on demand', async () => {
    const user = userEvent.setup()
    const getDashboard = vi.fn<IntercomApi['getDashboard']>()
      .mockRejectedValueOnce(new IntercomApiError(502, 'upstream_unavailable'))
      .mockResolvedValueOnce(dashboard)
    render(<App api={createApi({ getDashboard })} />)

    expect(await screen.findByRole('heading', { name: 'Не удалось загрузить объекты' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Повторить' }))

    expect(await screen.findByText('Квартира 81')).toBeVisible()
    expect(getDashboard).toHaveBeenCalledTimes(2)
  })

  it('reloads devices and the snapshot when another returned flat is selected', async () => {
    const user = userEvent.setup()
    const getDevices = vi.fn<IntercomApi['getDevices']>(async (flatId) => flatId === 'flat-81' ? inventory : secondInventory)
    const getSnapshot = vi.fn<IntercomApi['getSnapshot']>(async () => new Blob(['snapshot'], { type: 'image/webp' }))
    render(<App api={createApi({ getDevices, getSnapshot })} />)
    expect(await screen.findByText('Подъезд 1')).toBeVisible()

    await user.click(screen.getByRole('button', { name: /Текущий объект/ }))
    await user.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: /Квартира 22/ }))

    expect(await screen.findByText('Калитка')).toBeVisible()
    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у калитки' })).toHaveAttribute('src', 'blob:snapshot-2')
    expect(getDevices).toHaveBeenLastCalledWith('flat / 22', expect.any(AbortSignal))
    expect(getSnapshot).toHaveBeenLastCalledWith('flat / 22', 'camera / 2', expect.any(AbortSignal))
  })

  it('renders honest empty and error states for a selected flat', async () => {
    const user = userEvent.setup()
    const getDevices = vi.fn<IntercomApi['getDevices']>()
      .mockResolvedValueOnce({ accessPoints: [], cameras: [] })
      .mockRejectedValueOnce(new IntercomApiError(502, 'upstream_unavailable'))
    render(<App api={createApi({ getDevices })} />)

    expect(await screen.findByText('Точек доступа для этого объекта нет')).toBeVisible()
    expect(screen.getByText('Камер для этого объекта нет')).toBeVisible()

    await user.click(screen.getByRole('button', { name: /Текущий объект/ }))
    await user.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: /Квартира 22/ }))

    expect(await screen.findByRole('heading', { name: 'Не удалось загрузить устройства' })).toBeVisible()
  })

  it('revokes snapshot object URLs when the camera changes and on unmount', async () => {
    const user = userEvent.setup()
    const api = createApi({
      getDevices: async (flatId) => flatId === 'flat-81' ? inventory : secondInventory,
    })
    const view = render(<App api={api} />)
    expect(await screen.findByRole('img', { name: /Кадр с камеры Камера у подъезда/ })).toHaveAttribute('src', 'blob:snapshot-1')

    await user.click(screen.getByRole('button', { name: /Текущий объект/ }))
    await user.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: /Квартира 22/ }))
    expect(await screen.findByRole('img', { name: /Кадр с камеры Камера у калитки/ })).toHaveAttribute('src', 'blob:snapshot-2')
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:snapshot-1')

    view.unmount()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:snapshot-2')
  })

  it('does not request a snapshot when the camera capability is unavailable', async () => {
    const getSnapshot = vi.fn<IntercomApi['getSnapshot']>()
    const camerasUnavailable: DeviceInventory = {
      ...inventory,
      cameras: inventory.cameras.map((camera) => ({
        ...camera,
        capabilities: { ...camera.capabilities, snapshot: unavailable },
      })),
    }
    render(<App api={createApi({ getDevices: async () => camerasUnavailable, getSnapshot })} />)

    expect(await screen.findByText('Снимки с этой камеры недоступны')).toBeVisible()
    expect(getSnapshot).not.toHaveBeenCalled()
  })

  it('requires lock selection, creates an intent, and executes only after explicit confirmation', async () => {
    const user = userEvent.setup()
    const createDoorIntent = vi.fn<IntercomApi['createDoorIntent']>(async () => ({ intentId: 'intent / 1', expiresAt: new Date(Date.now() + 60_000).toISOString() }))
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
    const lockSelect = await screen.findByRole('combobox', { name: 'Замок Подъезд 1' })
    const prepare = screen.getByRole('button', { name: 'Открыть Подъезд 1' })
    expect(prepare).toBeDisabled()

    await user.selectOptions(lockSelect, '2')
    await user.click(prepare)

    expect(createDoorIntent).toHaveBeenCalledWith('flat-81', 'entrance-1', '2')
    expect(executeDoorIntent).not.toHaveBeenCalled()
    const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь?' })
    expect(within(dialog).getByText('Квартира 81')).toBeVisible()
    expect(within(dialog).getByText('Подъезд 1')).toBeVisible()
    expect(within(dialog).getByText('Вторая дверь')).toBeVisible()
    expect(within(dialog).getByText(/Действует до/)).toBeVisible()

    await user.click(within(dialog).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(executeDoorIntent).toHaveBeenCalledWith('intent / 1')
    expect(await screen.findByRole('status')).toHaveTextContent('Подъезд 1 открыт')
    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
  })

  it('cancels a prepared door intent without executing it', async () => {
    const user = userEvent.setup()
    const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>()
    render(<App api={createApi({ executeDoorIntent })} />)
    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))

    const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь?' })
    await user.click(within(dialog).getByRole('button', { name: 'Отмена' }))

    expect(executeDoorIntent).not.toHaveBeenCalled()
    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
  })

  it('dispatches only one execute request when confirmation is double-clicked', async () => {
    const user = userEvent.setup()
    const pending = deferred<void>()
    const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>(() => pending.promise)
    render(<App api={createApi({ executeDoorIntent })} />)
    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))
    const confirm = within(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).getByRole('button', { name: 'Подтвердить открытие' })

    await user.dblClick(confirm)
    expect(executeDoorIntent).toHaveBeenCalledTimes(1)
    expect(confirm).toBeDisabled()

    await act(async () => pending.resolve())
    expect(await screen.findByRole('status')).toHaveTextContent('Подъезд 1 открыт')
  })

  it('treats an expired-intent execute response as unknown and prevents an immediate repeat', async () => {
    const user = userEvent.setup()
    const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>(async () => {
      throw new IntercomApiError(404, 'door_open_intent_not_found')
    })
    render(<App api={createApi({ executeDoorIntent })} />)
    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))
    await user.click(within(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Результат открытия неизвестен. Не повторяйте команду сразу')
    expect(screen.queryByText(/открыт$/)).not.toBeInTheDocument()
    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1' })).toBeDisabled()
    expect(document.body).not.toHaveTextContent(/подготовьте.*заново/i)
  })

  it('uses the same unknown-result safety state when an execute response is lost', async () => {
    const user = userEvent.setup()
    const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>(async () => {
      throw new TypeError('network response lost')
    })
    render(<App api={createApi({ executeDoorIntent })} />)
    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))
    await user.click(within(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Результат открытия неизвестен. Не повторяйте команду сразу')
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1' })).toBeDisabled()
    expect(executeDoorIntent).toHaveBeenCalledTimes(1)
  })

  it('releases an unexecuted prepared intent only after its local expiry', async () => {
    const user = userEvent.setup()
    render(<App api={createApi({
      createDoorIntent: async () => ({ intentId: 'short-intent', expiresAt: new Date(Date.now() + 120).toISOString() }),
    })} />)
    const open = await screen.findByRole('button', { name: 'Открыть Подъезд 1' })
    await user.click(open)

    expect(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).toBeVisible()
    expect(open).toBeDisabled()
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument())
    expect(await screen.findByRole('alert')).toHaveTextContent('Подтверждение истекло')
    expect(open).toBeEnabled()
  })

  it('clears all intercom state and revokes the camera snapshot after logout', async () => {
    const user = userEvent.setup()
    const logout = vi.fn(async () => undefined)
    const getDashboard = vi.fn<IntercomApi['getDashboard']>(async () => dashboard)
    render(<App
      api={createApi({
        createDoorIntent: async () => ({ intentId: 'intent-to-clear', expiresAt: new Date(Date.now() + 60_000).toISOString() }),
        getDashboard,
      })}
      authApi={authenticatedAuthApi({ logout })}
    />)
    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-1')
    await user.click(screen.getByRole('button', { name: 'Открыть Подъезд 1' }))
    expect(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))

    expect(logout).toHaveBeenCalledTimes(1)
    expect(await screen.findByRole('heading', { name: 'Войдите в MTS ID' })).toBeVisible()
    expect(screen.queryByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).not.toBeInTheDocument()
    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
    expect(screen.queryByText('Подъезд 1')).not.toBeInTheDocument()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:snapshot-1')
    expect(getDashboard).toHaveBeenCalledTimes(1)
  })

  it('keeps loaded intercom state when logout fails and shows a stable error', async () => {
    const user = userEvent.setup()
    const logout = vi.fn(async () => { throw new Error('storage secret') })
    render(<App api={createApi()} authApi={authenticatedAuthApi({ logout })} />)
    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось выйти')
    expect(document.body).not.toHaveTextContent('storage secret')
    expect(screen.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()
    expect(URL.revokeObjectURL).not.toHaveBeenCalled()
    expect(logout).toHaveBeenCalledTimes(1)
  })

  it('polls calls while the unrelated dashboard request is still loading', async () => {
    const pendingDashboard = deferred<PropertyDashboard>()
    const getCurrent = vi.fn<CallApi['getCurrent']>(async () => null)
    render(<App
      api={createApi({ getDashboard: () => pendingDashboard.promise })}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)

    await waitFor(() => expect(getCurrent).toHaveBeenCalledTimes(1))
    await act(async () => pendingDashboard.resolve(dashboard))
    expect(getCurrent).toHaveBeenCalledTimes(1)
  })

  it('does not poll calls for an anonymous session', async () => {
    const getCurrent = vi.fn<CallApi['getCurrent']>(async () => null)
    render(<App
      api={createApi({ getDashboard: async () => { throw new IntercomApiError(401, 'authentication_required') } })}
      authApi={anonymousAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)

    expect(await screen.findByRole('heading', { name: 'Войдите в MTS ID' })).toBeVisible()
    expect(getCurrent).not.toHaveBeenCalled()
  })

  it('shows a ringing call before the dashboard request settles', async () => {
    const pendingDashboard = deferred<PropertyDashboard>()
    render(<App
      api={createApi({ getDashboard: () => pendingDashboard.promise })}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent: async () => ringingCall })}
    />)

    expect(await screen.findByRole('heading', { name: 'Домофон у подъезда' })).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Входящий вызов')
  })

  it('switches to the calls page and marks navigation for a real incoming call', async () => {
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent: async () => ringingCall })}
    />)

    expect(await screen.findByRole('heading', { name: 'Домофон у подъезда' })).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Входящий вызов')
    expect(within(screen.getByRole('complementary', { name: 'Основная навигация' })).getByRole('button', { name: /Звонки.*Входящий/ })).toHaveClass('active')
  })

  it('keeps the browser call session mounted while navigating away and back', async () => {
    const user = userEvent.setup()
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent: async () => ringingCall })}
    />)
    const callHeading = await screen.findByRole('heading', { name: 'Домофон у подъезда' })
    const navigation = within(screen.getByRole('complementary', { name: 'Основная навигация' }))

    await user.click(navigation.getByRole('button', { name: 'Главная' }))
    expect(callHeading).not.toBeVisible()
    await user.click(navigation.getByRole('button', { name: /Звонки.*Входящий/ }))

    expect(screen.getByRole('heading', { name: 'Домофон у подъезда' })).toBe(callHeading)
  })

  it('aborts call polling and clears the call UI after logout', async () => {
    const user = userEvent.setup()
    let callSignal: AbortSignal | undefined
    const getCurrent = vi.fn<CallApi['getCurrent']>((signal) => {
      callSignal = signal
      return new Promise<CurrentCall | null>(() => undefined)
    })
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)
    await waitFor(() => expect(callSignal).toBeDefined())

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))

    expect(callSignal?.aborted).toBe(true)
    expect(await screen.findByRole('heading', { name: 'Войдите в MTS ID' })).toBeVisible()
    expect(screen.queryByRole('heading', { name: 'Домофон у подъезда' })).not.toBeInTheDocument()
  })

  it('does not invent an incoming call when the call endpoint returns no current call', async () => {
    const user = userEvent.setup()
    render(<App api={createApi()} authApi={authenticatedAuthApi()} callsApi={createCallApi()} />)
    expect(await screen.findByText('Подъезд 1')).toBeVisible()

    await user.click(within(screen.getByRole('complementary', { name: 'Основная навигация' })).getByRole('button', { name: 'Звонки' }))
    expect(await screen.findByRole('heading', { name: 'Нет активного звонка' })).toBeVisible()
    expect(screen.queryByText('Входящий вызов')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Принять вызов' })).not.toBeInTheDocument()
  })
})
