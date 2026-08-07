import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AuthApi } from '../features/auth/authApi'
import type { CallApi, CurrentCall } from '../features/calls/callApi'
import type { EventApi } from '../features/events/eventApi'
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
    getSession: async () => ({ authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null }),
    importWebSsoResponse: async () => ({ status: 'authenticated', session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' } }),
    logout: async () => undefined,
    startAuth: async () => ({
      status: 'browser_redirect',
      flow_id: 'flow-1',
      authorization_url: 'https://login.mts.ru/amserver/UI/Login?realm=users',
    }),
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

function reauthenticatingAuthApi(): AuthApi {
  let sessionReads = 0
  return authenticatedAuthApi({
    getSession: async () => sessionReads++ < 2
      ? { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' }
      : { authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null },
    startAuth: async () => ({
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-07T12:00:00Z', expires_at: null },
    }),
  })
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

afterEach(() => {
  vi.restoreAllMocks()
})

describe('live intercom dashboard', () => {
  it('starts live video with sound only after a button click and stops it', async () => {
    const user = userEvent.setup()
    const startLive = vi.fn(async () => ({
      sessionId: '5d7a5769-adf9-4a41-9bd8-12427e0ae921',
      playlistUrl: '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8',
    }))
    const stopLive = vi.fn(async () => undefined)
    const liveInventory: DeviceInventory = {
      ...inventory,
      cameras: inventory.cameras.map((camera) => ({
        ...camera,
        capabilities: { ...camera.capabilities, live: available },
      })),
    }
    vi.spyOn(HTMLMediaElement.prototype, 'canPlayType').mockReturnValue('probably')
    vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue()
    render(<App api={createApi({ getDevices: async () => liveInventory, startLive, stopLive })} />)

    await user.click(await screen.findByRole('button', { name: 'Смотреть Камера у подъезда' }))
    await user.click(screen.getByRole('button', { name: 'Эфир' }))

    expect(startLive).toHaveBeenCalledWith('flat-81', 'camera-1', expect.any(AbortSignal))
    const video = await screen.findByLabelText('Live-видео с камеры Камера у подъезда')
    expect(video).toHaveAttribute('controls')
    expect(video).not.toHaveAttribute('muted')
    await user.click(screen.getByRole('button', { name: 'Остановить' }))
    await waitFor(() => expect(stopLive).toHaveBeenCalledWith('5d7a5769-adf9-4a41-9bd8-12427e0ae921'))
  })

  it('requires a new user gesture after switching cameras away and back', async () => {
    const user = userEvent.setup()
    const startLive = vi.fn(async () => ({
      sessionId: '5d7a5769-adf9-4a41-9bd8-12427e0ae921',
      playlistUrl: '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8',
    }))
    const stopLive = vi.fn(async () => undefined)
    const liveInventory: DeviceInventory = {
      ...inventory,
      cameras: [
        {
          ...inventory.cameras[0],
          capabilities: { ...inventory.cameras[0].capabilities, live: available },
        },
        {
          ...inventory.cameras[0],
          id: 'camera-2',
          name: 'Камера у калитки',
          capabilities: { ...inventory.cameras[0].capabilities, live: available },
        },
      ],
    }
    vi.spyOn(HTMLMediaElement.prototype, 'canPlayType').mockReturnValue('probably')
    vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue()
    render(<App api={createApi({ getDevices: async () => liveInventory, startLive, stopLive })} />)

    await user.click(await screen.findByRole('button', { name: 'Смотреть Камера у подъезда' }))
    expect(screen.getByRole('button', { name: 'Камера у подъезда' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('button', { name: 'Камера у калитки' })).toHaveAttribute('aria-pressed', 'false')
    await user.click(await screen.findByRole('button', { name: 'Эфир' }))
    expect(await screen.findByLabelText('Live-видео с камеры Камера у подъезда')).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Камера у калитки' }))
    expect(screen.getByRole('button', { name: 'Камера у калитки' })).toHaveAttribute('aria-pressed', 'true')
    await user.click(screen.getByRole('button', { name: 'Камера у подъезда' }))

    expect(await screen.findByRole('button', { name: 'Эфир' })).toBeVisible()
    expect(startLive).toHaveBeenCalledTimes(1)
  })

  it('offers an honest keyboard-operable compact camera switch on Home and cleans up the prior snapshot', async () => {
    const user = userEvent.setup()
    const multiCameraInventory: DeviceInventory = {
      ...inventory,
      cameras: [
        inventory.cameras[0],
        { ...inventory.cameras[0], id: 'camera-2', name: 'Камера у калитки' },
      ],
    }
    render(<App api={createApi({ getDevices: async () => multiCameraInventory })} />)

    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-1')
    const switcher = screen.getByRole('group', { name: 'Камеры на главной' })
    const firstCamera = within(switcher).getByRole('button', { name: 'Камера у подъезда' })
    const secondCamera = within(switcher).getByRole('button', { name: 'Камера у калитки' })
    expect(firstCamera).toHaveAttribute('aria-pressed', 'true')
    expect(secondCamera).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByText('Доступно: кадр')).toBeVisible()
    expect(document.body).not.toHaveTextContent(/Кадр получен от локального API|онлайн/i)

    secondCamera.focus()
    await user.keyboard('{Enter}')

    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у калитки' })).toHaveAttribute('src', 'blob:snapshot-2')
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:snapshot-1')
    expect(firstCamera).toHaveAttribute('aria-pressed', 'false')
    expect(secondCamera).toHaveAttribute('aria-pressed', 'true')
  })

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

    expect(await screen.findByRole('heading', { name: 'Главная' })).toBeVisible()
    expect(await screen.findByText('Подъезд 1')).toBeVisible()
    expect(await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-1')
    expect(screen.getByText('Москва, ул. Примерная, 7')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1' })).toBeEnabled()
    expect(document.body).not.toHaveTextContent(/preview|демо|синтетическ/i)
    expect(screen.queryByText('Поддержка')).not.toBeInTheDocument()
    expect(screen.queryByText('Документы')).not.toBeInTheDocument()
    expect(screen.queryByText('Избранное')).not.toBeInTheDocument()
    const navigationElement = screen.getByRole('navigation', { name: 'Основная навигация' })
    const navigation = within(navigationElement)
    expect(navigation.getAllByRole('button')).toHaveLength(3)
    expect(navigation.getByRole('button', { name: 'Главная' })).toHaveAttribute('aria-current', 'page')
    expect(navigation.getByRole('button', { name: 'События' })).toBeVisible()
    expect(navigation.getByRole('button', { name: 'Ещё' })).toBeVisible()
    expect(navigationElement.querySelectorAll('button')).toHaveLength(3)
    expect(navigation.queryByRole('button', { name: 'Доступ' })).not.toBeInTheDocument()
    expect(navigation.queryByRole('button', { name: 'Камеры' })).not.toBeInTheDocument()
    expect(navigation.queryByRole('button', { name: 'Звонки' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Профиль подключения' })).toHaveAttribute('aria-label', 'Профиль подключения')
  })

  it('opens camera detail and returns to the originating destination', async () => {
    const user = userEvent.setup()
    render(<App api={createApi()} />)

    await user.click(await screen.findByRole('button', { name: 'Смотреть Камера у подъезда' }))
    expect(screen.getByRole('button', { name: 'Назад' })).toBeVisible()
    expect(screen.getByRole('group', { name: 'Режим камеры' })).toBeVisible()
    expect(screen.getByText('Архив этой камеры недоступен')).toBeVisible()

    await user.click(screen.getByRole('button', { name: 'Назад' }))
    expect(await screen.findByRole('heading', { name: 'Главная' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Смотреть Камера у подъезда' })).toBeVisible()
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
    const unavailableDoor = screen.getByRole('button', { name: 'Открытие Калитка недоступно' })
    expect(unavailableDoor).toBeDisabled()
    expect(unavailableDoor).toHaveTextContent('Недоступно')
    expect(getDevices).toHaveBeenLastCalledWith('flat / 22', expect.any(AbortSignal))
    expect(getSnapshot).toHaveBeenLastCalledWith('flat / 22', 'camera / 2', expect.any(AbortSignal))
  })

  it('invalidates a deferred door preparation when the selected flat changes without releasing a newer guard', async () => {
    const user = userEvent.setup()
    const flatAIntent = deferred<{ intentId: string; expiresAt: string }>()
    const flatBIntent = deferred<{ intentId: string; expiresAt: string }>()
    const preparationSignals: AbortSignal[] = []
    const flatBInventory: DeviceInventory = {
      ...secondInventory,
      accessPoints: [{
        ...secondInventory.accessPoints[0],
        available: true,
        locks: [{ number: '1', name: 'Калитка', permanentlyOpen: false }],
      }],
    }
    const createDoorIntent = vi.fn<IntercomApi['createDoorIntent']>((flatId, _accessPointId, _lockNumber, signal) => {
      if (signal) preparationSignals.push(signal)
      return flatId === 'flat-81' ? flatAIntent.promise : flatBIntent.promise
    })
    const executeDoorIntent = vi.fn<IntercomApi['executeDoorIntent']>()
    render(<App api={createApi({
      createDoorIntent,
      executeDoorIntent,
      getDevices: async (flatId) => flatId === 'flat-81' ? inventory : flatBInventory,
    })} />)

    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))
    await user.click(screen.getByRole('button', { name: /Текущий объект/ }))
    await user.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: /Квартира 22/ }))

    const openFlatB = await screen.findByRole('button', { name: 'Открыть Калитка' })
    expect(openFlatB).toBeEnabled()
    expect(preparationSignals[0]?.aborted).toBe(true)
    await user.click(openFlatB)
    expect(createDoorIntent).toHaveBeenCalledTimes(2)

    await act(async () => flatAIntent.resolve({
      intentId: 'stale-flat-a-intent',
      expiresAt: new Date(Date.now() + 60_000).toISOString(),
    }))

    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
    expect(openFlatB).toBeDisabled()
    expect(executeDoorIntent).not.toHaveBeenCalled()

    await act(async () => flatBIntent.resolve({
      intentId: 'current-flat-b-intent',
      expiresAt: new Date(Date.now() + 60_000).toISOString(),
    }))
    const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь?' })
    expect(within(dialog).getByText('Квартира 22')).toBeVisible()
    expect(within(dialog).getAllByText('Калитка')).toHaveLength(2)
    expect(executeDoorIntent).not.toHaveBeenCalled()
  })

  it('renders honest empty and error states for a selected flat', async () => {
    const user = userEvent.setup()
    const getDevices = vi.fn<IntercomApi['getDevices']>()
      .mockResolvedValueOnce({ accessPoints: [], cameras: [] })
      .mockRejectedValueOnce(new IntercomApiError(502, 'upstream_unavailable'))
    render(<App api={createApi({ getDevices })} />)

    expect(await screen.findByText('Дверей для этого объекта нет')).toBeVisible()
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

  it('renders one direct button per lock and executes only after explicit confirmation', async () => {
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
    expect(await screen.findByRole('button', { name: 'Открыть Подъезд 1: Основная дверь' })).toBeEnabled()
    const secondLock = screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })
    expect(secondLock).toBeEnabled()
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument()

    await user.click(secondLock)

    expect(createDoorIntent).toHaveBeenCalledWith('flat-81', 'entrance-1', '2', expect.any(AbortSignal))
    expect(executeDoorIntent).not.toHaveBeenCalled()
    const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь?' })
    expect(within(dialog).getByRole('button', { name: 'Закрыть' })).toHaveAttribute('aria-label', 'Закрыть')
    expect(within(dialog).getByText('Квартира 81')).toBeVisible()
    expect(within(dialog).getByText('Подъезд 1')).toBeVisible()
    expect(within(dialog).getByText('Вторая дверь')).toBeVisible()
    expect(within(dialog).getByText(/Действует до/)).toBeVisible()

    await user.click(within(dialog).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(executeDoorIntent).toHaveBeenCalledWith('intent / 1')
    expect(await screen.findByRole('status')).toHaveTextContent('Подъезд 1 открыт')
    expect(screen.queryByRole('dialog', { name: 'Открыть дверь?' })).not.toBeInTheDocument()
  })

  it('uses the lock-number fallback in the final confirmation when a lock name is blank', async () => {
    const user = userEvent.setup()
    const blankLockNameInventory: DeviceInventory = {
      ...inventory,
      accessPoints: inventory.accessPoints.map((point) => ({
        ...point,
        locks: [{ number: '1', name: '  ', permanentlyOpen: false }],
      })),
    }
    render(<App api={createApi({ getDevices: async () => blankLockNameInventory })} />)

    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1' }))

    expect(within(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).getByText('Замок 1')).toBeVisible()
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

  it('blocks only the lock with a lost execute response', async () => {
    const user = userEvent.setup()
    const twoLocks: DeviceInventory = {
      ...inventory,
      accessPoints: inventory.accessPoints.map((point) => ({
        ...point,
        locks: [
          { number: '1', name: 'Основная дверь', permanentlyOpen: false },
          { number: '2', name: 'Вторая дверь', permanentlyOpen: false },
        ],
      })),
    }
    render(<App api={createApi({
      executeDoorIntent: async () => { throw new TypeError('network response lost') },
      getDevices: async () => twoLocks,
    })} />)

    await user.click(await screen.findByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' }))
    await user.click(within(await screen.findByRole('dialog', { name: 'Открыть дверь?' })).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Результат открытия неизвестен. Не повторяйте команду сразу')
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Основная дверь' })).toBeEnabled()
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

  it('does not let a stale session restore disable calls after authentication succeeds', async () => {
    const user = userEvent.setup()
    const initialSession = deferred<Awaited<ReturnType<AuthApi['getSession']>>>()
    const getSession = vi.fn<AuthApi['getSession']>()
      .mockImplementationOnce(() => initialSession.promise)
      .mockResolvedValueOnce({ authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null })
    const authApi = anonymousAuthApi()
    authApi.getSession = getSession
    authApi.startAuth = async () => ({
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T12:00:00Z', expires_at: null },
    })
    let callSignal: AbortSignal | undefined
    const getCurrent = vi.fn<CallApi['getCurrent']>((signal) => {
      callSignal = signal
      return new Promise<CurrentCall | null>(() => undefined)
    })
    render(<App api={createApi()} authApi={authApi} callsApi={createCallApi({ getCurrent })} />)
    await waitFor(() => expect(getSession).toHaveBeenCalledTimes(1))

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    await waitFor(() => expect(callSignal).toBeDefined())
    await act(async () => initialSession.resolve({
      authenticated: false,
      status: 'anonymous',
      authenticated_at: null,
      expires_at: null,
    }))

    expect(callSignal?.aborted).toBe(false)
  })

  it('shows a ringing call before the dashboard request settles', async () => {
    const pendingDashboard = deferred<PropertyDashboard>()
    render(<App
      api={createApi({ getDashboard: () => pendingDashboard.promise })}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent: async () => ringingCall })}
    />)

    const dialog = await screen.findByRole('dialog', { name: 'Входящий вызов' })
    const shell = screen.getByRole('navigation', { name: 'Основная навигация' }).closest('.app-shell')!
    expect(shell.compareDocumentPosition(dialog) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(dialog).getByRole('heading', { name: 'Домофон у подъезда' })).toBeVisible()
    expect(within(dialog).getByRole('status')).toHaveTextContent('Входящий вызов')
    expect(within(screen.getByRole('main')).getByRole('status')).toHaveTextContent('Загружаем объекты')
  })

  it.each([
    ['2026-08-06T11:59:59Z', null],
    ['2026-08-06T12:00:01Z', 'Дверь открыта автоматически'],
  ] as const)('correlates an auto-open result completed at %s to the current call', async (completedAt, expectedMessage) => {
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getCurrent: async () => ringingCall,
        getAutoOpenPolicy: async () => ({
          mode: 'disabled',
          dismissAfterOpen: false,
          lastResult: { status: 'opened', completedAt },
        }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'disabled', dismissAfterOpen: false }),
        disableAutoOpen: async () => undefined,
      })}
    />)

    const dialog = await screen.findByRole('dialog', { name: 'Входящий вызов' })
    await screen.findByRole('button', { name: 'Автооткрытие' })
    if (expectedMessage) expect(within(dialog).getByText(expectedMessage)).toBeVisible()
    else expect(within(dialog).queryByText(/Дверь открыта автоматически|Автоматического повтора не будет/)).not.toBeInTheDocument()
  })

  it('overlays the saved destination and restores it unchanged when the call disappears', async () => {
    const user = userEvent.setup()
    const firstCall = deferred<CurrentCall | null>()
    const getCurrent = vi.fn<CallApi['getCurrent']>()
      .mockImplementationOnce(() => firstCall.promise)
      .mockResolvedValue(null)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)
    expect(await screen.findByText('Подъезд 1')).toBeVisible()
    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'События' }))
    expect(await screen.findByRole('heading', { name: 'События' })).toBeVisible()

    await act(async () => firstCall.resolve(ringingCall))

    expect(await screen.findByRole('dialog', { name: 'Входящий вызов' })).toBeVisible()
    expect(navigation.getByRole('button', { name: 'События' })).toHaveAttribute('aria-current', 'page')
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Входящий вызов' })).not.toBeInTheDocument(), { timeout: 2_000 })
    expect(screen.getByRole('heading', { name: 'События' })).toBeVisible()
    expect(navigation.getByRole('button', { name: 'События' })).toHaveAttribute('aria-current', 'page')
  })

  it('keeps one browser call overlay mounted while polling through call phases', async () => {
    const connectingCall: CurrentCall = { ...ringingCall, phase: 'connecting', revision: 4 }
    const activeCall: CurrentCall = { ...ringingCall, phase: 'active', revision: 5 }
    const endingCall: CurrentCall = { ...ringingCall, phase: 'ending', revision: 6 }
    const getCurrent = vi.fn<CallApi['getCurrent']>()
      .mockResolvedValueOnce(ringingCall)
      .mockResolvedValueOnce(connectingCall)
      .mockResolvedValueOnce(activeCall)
      .mockResolvedValueOnce(endingCall)
      .mockImplementation(() => new Promise<CurrentCall | null>(() => undefined))
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)
    const dialog = await screen.findByRole('dialog', { name: 'Входящий вызов' })
    expect(within(dialog).getByRole('status')).toHaveTextContent('Входящий вызов')

    await waitFor(() => expect(within(dialog).getByRole('status')).toHaveTextContent('Подключаем звонок'), { timeout: 2_000 })
    expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBe(dialog)
    await waitFor(() => expect(within(dialog).getByRole('status')).toHaveTextContent('Звонок идёт'), { timeout: 2_000 })
    expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBe(dialog)
    expect(within(dialog).getByRole('button', { name: 'Завершить вызов' })).toBeVisible()
    await waitFor(() => expect(within(dialog).getByRole('status')).toHaveTextContent('Завершаем звонок'), { timeout: 2_000 })
    expect(screen.getByRole('dialog', { name: 'Входящий вызов' })).toBe(dialog)
  })

  it('aborts call polling and tears down the call overlay after logout', async () => {
    const user = userEvent.setup()
    let callSignal: AbortSignal | undefined
    let pollCount = 0
    const getCurrent = vi.fn<CallApi['getCurrent']>((signal) => {
      pollCount += 1
      if (pollCount === 1) return Promise.resolve(ringingCall)
      callSignal = signal
      return new Promise<CurrentCall | null>(() => undefined)
    })
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({ getCurrent })}
    />)
    expect(await screen.findByRole('dialog', { name: 'Входящий вызов' })).toBeVisible()
    await waitFor(() => expect(callSignal).toBeDefined(), { timeout: 2_000 })

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))

    expect(callSignal?.aborted).toBe(true)
    expect(await screen.findByRole('heading', { name: 'Войдите в MTS ID' })).toBeVisible()
    expect(screen.queryByRole('dialog', { name: 'Входящий вызов' })).not.toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Домофон у подъезда' })).not.toBeInTheDocument()
  })

  it('does not invent an incoming call when the call endpoint returns no current call', async () => {
    render(<App api={createApi()} authApi={authenticatedAuthApi()} callsApi={createCallApi()} />)
    expect(await screen.findByText('Подъезд 1')).toBeVisible()

    expect(screen.queryByRole('dialog', { name: 'Входящий вызов' })).not.toBeInTheDocument()
    expect(screen.queryByText('Входящий вызов')).not.toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Нет активного звонка' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Принять вызов' })).not.toBeInTheDocument()
  })

  it('opens a camera archive at the timestamp selected from the real event journal', async () => {
    const user = userEvent.setup()
    const getEvents = vi.fn<EventApi['getEvents']>(async () => ({
      page: 0,
      hasMore: false,
      items: [{
        id: 'event-camera-1',
        occurredAt: '2026-08-07T09:30:00Z',
        kind: 'camera',
        title: 'Движение у подъезда',
        cameraId: 'camera-1',
        archiveAt: '2026-08-07T09:30:00Z',
      }],
    }))
    const archiveInventory: DeviceInventory = {
      ...inventory,
      cameras: inventory.cameras.map((camera) => ({
        ...camera,
        capabilities: { ...camera.capabilities, archive: available },
      })),
    }
    const getArchiveDistribution = vi.fn(async () => ({ intervals: [{
      from: '2026-08-07T09:00:00Z', to: '2026-08-07T10:00:00Z',
    }] }))
    render(<App
      api={createApi({ getArchiveDistribution, getDevices: async () => archiveInventory, startArchive: vi.fn(), stopLive: vi.fn() })}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi()}
      eventsApi={{ getEvents }}
    />)
    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'События' }))

    expect(await screen.findByRole('heading', { name: 'События' })).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Открыть запись камеры' }))

    expect(await screen.findByRole('heading', { name: 'Архив' })).toBeVisible()
    expect(screen.getByLabelText('Начало просмотра')).toHaveValue('2026-08-07T12:30')
    expect(getArchiveDistribution).toHaveBeenCalledWith('flat-81', 'camera-1', expect.any(AbortSignal))
    await user.click(screen.getByRole('button', { name: 'Назад' }))
    expect(await screen.findByRole('heading', { name: 'События' })).toBeVisible()
  })

  it('keeps More mounted for policy refresh without exposing hidden settings to accessibility', async () => {
    const user = userEvent.setup()
    const getAutoOpenPolicy = vi.fn(async () => ({ mode: 'disabled', dismissAfterOpen: false } as const))
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy,
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'disabled', dismissAfterOpen: false }),
        disableAutoOpen: async () => undefined,
      })}
    />)

    await waitFor(() => expect(getAutoOpenPolicy).toHaveBeenCalledTimes(1))
    expect(screen.queryByRole('heading', { name: 'Ещё' })).not.toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: /Автооткрытие/ })).not.toBeInTheDocument()

    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'Ещё' }))

    expect(await screen.findByRole('heading', { name: 'Ещё' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()
    expect(getAutoOpenPolicy).toHaveBeenCalledTimes(1)
  })

  it('keeps the armed Home state until direct disable is confirmed', async () => {
    const user = userEvent.setup()
    const pendingDisable = deferred<void>()
    const disableAutoOpen = vi.fn(() => pendingDisable.promise)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen,
      })}
    />)

    const armedStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    const disableButton = within(armedStatus).getByRole('button', { name: 'Выключить' })
    await user.click(disableButton)

    expect(disableAutoOpen).toHaveBeenCalledTimes(1)
    expect(disableButton).toBeDisabled()
    expect(screen.getByRole('status', { name: 'Автооткрытие включено' })).toBeVisible()

    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'Ещё' }))
    const moreDisable = screen.getByRole('button', { name: 'Выключить автооткрытие' })
    expect(moreDisable).toBeDisabled()
    await user.click(moreDisable)
    expect(disableAutoOpen).toHaveBeenCalledTimes(1)

    await act(async () => pendingDisable.resolve())
    expect(screen.queryByRole('status', { name: 'Автооткрытие включено' })).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()
  })

  it('blocks Home disable while More owns the mutation and syncs the result immediately', async () => {
    const user = userEvent.setup()
    const pendingDisable = deferred<void>()
    const disableAutoOpen = vi.fn(() => pendingDisable.promise)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen,
      })}
    />)
    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'Ещё' }))
    await user.click(await screen.findByRole('button', { name: 'Выключить автооткрытие' }))

    await user.click(navigation.getByRole('button', { name: 'Главная' }))
    const homeDisable = within(await screen.findByRole('status', { name: 'Автооткрытие включено' }))
      .getByRole('button', { name: 'Выключить' })
    expect(homeDisable).toBeDisabled()
    await user.click(homeDisable)
    expect(disableAutoOpen).toHaveBeenCalledTimes(1)

    await act(async () => pendingDisable.resolve())
    expect(screen.queryByRole('status', { name: 'Автооткрытие включено' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Автооткрытие' })).toBeVisible()
  })

  it('keeps a session-B Home mutation owned when a logged-out session-A Home mutation settles', async () => {
    const user = userEvent.setup()
    const sessionADisable = deferred<void>()
    const sessionBDisable = deferred<void>()
    const mutationSignals: Array<AbortSignal | undefined> = []
    const disableAutoOpen = vi.fn<NonNullable<CallApi['disableAutoOpen']>>((signal) => {
      mutationSignals.push(signal)
      return mutationSignals.length === 1 ? sessionADisable.promise : sessionBDisable.promise
    })
    render(<App
      api={createApi()}
      authApi={reauthenticatingAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen,
      })}
    />)

    const firstStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    await user.click(within(firstStatus).getByRole('button', { name: 'Выключить' }))
    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))
    expect(mutationSignals[0]?.aborted).toBe(true)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    const authDialog = await screen.findByRole('dialog', { name: 'Вход в VDome' })
    await user.click(within(authDialog).getByRole('button', { name: 'Войти с MTS ID' }))
    const secondStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    const secondDisable = within(secondStatus).getByRole('button', { name: 'Выключить' })
    await user.click(secondDisable)
    expect(disableAutoOpen).toHaveBeenCalledTimes(2)

    await act(async () => sessionADisable.resolve())
    expect(secondDisable).toBeDisabled()
    expect(screen.getByRole('status', { name: 'Автооткрытие включено' })).toBeVisible()

    await act(async () => sessionBDisable.resolve())
    expect(await screen.findByRole('button', { name: 'Автооткрытие' })).toBeVisible()
  })

  it('keeps a session-B More mutation owned when a logged-out session-A More mutation settles', async () => {
    const user = userEvent.setup()
    const sessionADisable = deferred<void>()
    const sessionBDisable = deferred<void>()
    const mutationSignals: AbortSignal[] = []
    const disableAutoOpen = vi.fn<NonNullable<CallApi['disableAutoOpen']>>((signal) => {
      if (signal) mutationSignals.push(signal)
      return mutationSignals.length === 1 ? sessionADisable.promise : sessionBDisable.promise
    })
    render(<App
      api={createApi()}
      authApi={reauthenticatingAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen,
      })}
    />)
    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'Ещё' }))
    await user.click(await screen.findByRole('button', { name: 'Выключить автооткрытие' }))

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))
    expect(mutationSignals[0]?.aborted).toBe(true)

    await user.click(await screen.findByRole('button', { name: 'Войти с MTS ID' }))
    const authDialog = await screen.findByRole('dialog', { name: 'Вход в VDome' })
    await user.click(within(authDialog).getByRole('button', { name: 'Войти с MTS ID' }))
    await user.click(within(screen.getByRole('navigation', { name: 'Основная навигация' })).getByRole('button', { name: 'Ещё' }))
    const secondDisable = await screen.findByRole('button', { name: 'Выключить автооткрытие' })
    await user.click(secondDisable)
    expect(disableAutoOpen).toHaveBeenCalledTimes(2)

    await act(async () => sessionADisable.resolve())
    expect(secondDisable).toBeDisabled()
    expect(screen.getByRole('heading', { name: 'Откроется следующий входящий звонок' })).toBeVisible()

    await act(async () => sessionBDisable.resolve())
    expect(await screen.findByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()
  })

  it('ignores a pre-mutation poll that resolves after confirmed disable', async () => {
    const user = userEvent.setup()
    const armed = { mode: 'next_call', dismissAfterOpen: false } as const
    const stalePolicy = deferred<typeof armed>()
    const policyPolls: Array<() => void> = []
    const originalSetTimeout = window.setTimeout.bind(window)
    const timerSpy = vi.spyOn(window, 'setTimeout').mockImplementation(((handler: TimerHandler, timeout?: number, ...args: unknown[]) => {
      if (timeout === 5_000 && typeof handler === 'function') {
        policyPolls.push(() => handler(...args))
        return 1
      }
      return originalSetTimeout(handler, timeout)
    }) as typeof window.setTimeout)
    const getAutoOpenPolicy = vi.fn<NonNullable<CallApi['getAutoOpenPolicy']>>()
      .mockResolvedValueOnce(armed)
      .mockImplementationOnce(() => stalePolicy.promise)
    const disableAutoOpen = vi.fn(async () => undefined)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy,
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => armed,
        disableAutoOpen,
      })}
    />)

    const armedStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    expect(policyPolls).toHaveLength(1)
    act(() => policyPolls[0]())
    await waitFor(() => expect(getAutoOpenPolicy).toHaveBeenCalledTimes(2))

    await user.click(within(armedStatus).getByRole('button', { name: 'Выключить' }))
    expect(await screen.findByRole('button', { name: 'Автооткрытие' })).toBeVisible()

    const navigation = within(screen.getByRole('navigation', { name: 'Основная навигация' }))
    await user.click(navigation.getByRole('button', { name: 'Ещё' }))
    expect(screen.getByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()

    await act(async () => stalePolicy.resolve(armed))

    expect(screen.getByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Выключить автооткрытие' })).not.toBeInTheDocument()
    await user.click(navigation.getByRole('button', { name: 'Главная' }))
    expect(screen.getByRole('button', { name: 'Автооткрытие' })).toBeVisible()
    expect(screen.queryByRole('status', { name: 'Автооткрытие включено' })).not.toBeInTheDocument()
    expect(disableAutoOpen).toHaveBeenCalledTimes(1)
    timerSpy.mockRestore()
  })

  it('ignores a pre-mutation poll failure after a confirmed policy mutation', async () => {
    const user = userEvent.setup()
    const armed = { mode: 'next_call', dismissAfterOpen: false } as const
    const stalePoll = deferred<typeof armed>()
    const policyPolls: Array<() => void> = []
    const originalSetTimeout = window.setTimeout.bind(window)
    const timerSpy = vi.spyOn(window, 'setTimeout').mockImplementation(((handler: TimerHandler, timeout?: number, ...args: unknown[]) => {
      if (timeout === 5_000 && typeof handler === 'function') {
        policyPolls.push(() => handler(...args))
        return 1
      }
      return originalSetTimeout(handler, timeout)
    }) as typeof window.setTimeout)
    const getAutoOpenPolicy = vi.fn<NonNullable<CallApi['getAutoOpenPolicy']>>()
      .mockResolvedValueOnce(armed)
      .mockImplementationOnce(() => stalePoll.promise)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy,
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => armed,
        disableAutoOpen: async () => undefined,
      })}
    />)

    const armedStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    act(() => policyPolls[0]())
    await waitFor(() => expect(getAutoOpenPolicy).toHaveBeenCalledTimes(2))
    await user.click(within(armedStatus).getByRole('button', { name: 'Выключить' }))
    await user.click(within(screen.getByRole('navigation', { name: 'Основная навигация' })).getByRole('button', { name: 'Ещё' }))

    await act(async () => stalePoll.reject(new Error('stale provider failure')))

    expect(screen.getByRole('heading', { name: 'Автооткрытие выключено' })).toBeVisible()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    timerSpy.mockRestore()
  })

  it('keeps a failed direct disable visible and retryable', async () => {
    const user = userEvent.setup()
    const disableAutoOpen = vi.fn<NonNullable<CallApi['disableAutoOpen']>>()
      .mockRejectedValueOnce(new Error('private provider response'))
      .mockResolvedValueOnce(undefined)
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen,
      })}
    />)

    const armedStatus = await screen.findByRole('status', { name: 'Автооткрытие включено' })
    await user.click(within(armedStatus).getByRole('button', { name: 'Выключить' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось выключить автооткрытие')
    expect(document.body).not.toHaveTextContent('private provider response')
    expect(screen.getByRole('status', { name: 'Автооткрытие включено' })).toBeVisible()

    await user.click(within(armedStatus).getByRole('button', { name: 'Выключить' }))
    await waitFor(() => expect(disableAutoOpen).toHaveBeenCalledTimes(2))
    expect(screen.queryByRole('status', { name: 'Автооткрытие включено' })).not.toBeInTheDocument()
  })

  it('shows one recent-events heading and Home controls for the armed server policy, then clears them on logout', async () => {
    const user = userEvent.setup()
    const eventsApi: EventApi = { getEvents: async () => ({
      page: 0,
      hasMore: false,
      items: [{
        id: 'missed-1', occurredAt: '2026-08-07T08:00:00Z', kind: 'call',
        title: 'Call', callOutcome: 'missed', sourceLabel: 'Подъезд 1',
      }],
    }) }
    render(<App
      api={createApi()}
      authApi={authenticatedAuthApi()}
      callsApi={createCallApi({
        getAutoOpenPolicy: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        createAutoOpenIntent: async () => ({ intentId: 'auto-1', expiresAt: '2026-08-07T10:00:30Z' }),
        executeAutoOpenIntent: async () => ({ mode: 'next_call', dismissAfterOpen: false }),
        disableAutoOpen: async () => undefined,
      })}
      eventsApi={eventsApi}
    />)

    expect(await screen.findByRole('status', { name: 'Автооткрытие включено' })).toHaveTextContent('следующий звонок')
    expect(await screen.findByText(/Пропущенный вызов/)).toBeVisible()
    expect(screen.getAllByRole('heading', { name: 'Последние события' })).toHaveLength(1)

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(await screen.findByRole('button', { name: 'Выйти' }))
    expect(screen.queryByRole('status', { name: 'Автооткрытие включено' })).not.toBeInTheDocument()
  })
})
