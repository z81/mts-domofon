import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CameraExperience } from './CameraExperience'
import type { Camera, IntercomApi } from './intercomApi'

const available = { state: 'available' } as const
const unavailable = { state: 'unavailable', reason: null } as const

const liveSession = {
  sessionId: '5d7a5769-adf9-4a41-9bd8-12427e0ae921',
  playlistUrl: '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8',
}

const camera: Camera = {
  id: 'camera-1',
  flatId: 'flat-81',
  name: 'Камера у подъезда',
  accessPointId: 'entrance-1',
  capabilities: { snapshot: available, live: unavailable, archive: unavailable },
}

function createApi(overrides: Partial<IntercomApi> = {}): IntercomApi {
  return {
    createDoorIntent: async () => ({ intentId: 'intent-1', expiresAt: '2026-08-07T12:00:00Z' }),
    executeDoorIntent: async () => undefined,
    getDashboard: async () => ({ flats: [], selectedFlatId: null, callSettings: { state: 'unavailable' } }),
    getDevices: async () => ({ accessPoints: [], cameras: [] }),
    getSnapshot: async () => new Blob(['snapshot'], { type: 'image/webp' }),
    ...overrides,
  }
}

function mockMediaElement() {
  vi.spyOn(HTMLMediaElement.prototype, 'canPlayType').mockReturnValue('probably')
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => undefined)
  return vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue()
}

beforeEach(() => {
  let objectUrlIndex = 0
  Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: vi.fn(() => `blob:snapshot-${++objectUrlIndex}`) })
  Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: vi.fn() })
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('CameraExperience', () => {
  it('shows a refreshable preview snapshot and opens camera detail from the picture', async () => {
    const user = userEvent.setup()
    let resolveRefresh!: (blob: Blob) => void
    const getSnapshot = vi.fn<IntercomApi['getSnapshot']>()
      .mockResolvedValueOnce(new Blob(['snapshot'], { type: 'image/webp' }))
      .mockReturnValueOnce(new Promise<Blob>((resolve) => { resolveRefresh = resolve }))
    const api = createApi({ getSnapshot })
    const openDetail = vi.fn()

    render(<CameraExperience api={api} camera={camera} flatId="flat-81" presentation="preview" onOpenDetail={openDetail} />)

    const snapshot = await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })
    expect(snapshot).toBeVisible()
    expect(snapshot).toHaveAttribute('src', 'blob:snapshot-1')
    expect(snapshot.closest('article')).toHaveClass('camera-card', 'camera-preview')
    expect(screen.getByText(/^Кадр \d\d:\d\d:\d\d$/)).toBeVisible()
    expect(screen.queryByText(/Доступно:|СНИМОК/)).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /^Смотреть/ })).not.toBeInTheDocument()
    expect(screen.queryByText('Кадр получен от локального API')).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Обновить кадр' }))
    expect(screen.getByRole('button', { name: 'Обновить кадр' })).toHaveAttribute('aria-busy', 'true')
    expect(screen.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-1')
    await act(async () => resolveRefresh(new Blob(['fresh'], { type: 'image/webp' })))
    expect(screen.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toHaveAttribute('src', 'blob:snapshot-2')
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:snapshot-1')

    await user.click(screen.getByRole('button', { name: 'Открыть камеру Камера у подъезда' }))
    expect(openDetail).toHaveBeenCalledTimes(1)
  })

  it('starts live of a live-only camera at once, with sound attempted and without a snapshot mode', async () => {
    const play = mockMediaElement()
    const getSnapshot = vi.fn<IntercomApi['getSnapshot']>()
    const startLive = vi.fn(async () => liveSession)
    const stopLive = vi.fn(async () => undefined)
    const api = createApi({ getSnapshot, startLive, stopLive })
    const liveCamera: Camera = {
      ...camera,
      capabilities: { ...camera.capabilities, live: available, snapshot: unavailable },
    }

    const view = render(<CameraExperience api={api} camera={liveCamera} flatId="flat-81" presentation="detail" />)

    const video = await screen.findByLabelText('Эфир с камеры Камера у подъезда') as HTMLVideoElement
    await waitFor(() => expect(video).toHaveAttribute('src', liveSession.playlistUrl))
    expect(startLive).toHaveBeenCalledWith('flat-81', 'camera-1', expect.any(AbortSignal))
    expect(play).toHaveBeenCalled()
    expect(video.muted).toBe(false)
    expect(video).not.toHaveAttribute('controls')
    expect(screen.getByRole('button', { name: 'Выключить звук' })).toBeVisible()
    expect(screen.queryByRole('group', { name: 'Режим камеры' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Кадр' })).not.toBeInTheDocument()
    expect(screen.queryByText('Выберите режим камеры')).not.toBeInTheDocument()
    expect(getSnapshot).not.toHaveBeenCalled()

    view.unmount()
    expect(stopLive).not.toHaveBeenCalled()
  })

  it('marks the selected camera mode for the active visual tab', async () => {
    const user = userEvent.setup()
    mockMediaElement()
    const api = createApi({
      getArchiveDistribution: vi.fn(async () => ({ intervals: [] })),
      startArchive: vi.fn(async () => liveSession),
      startLive: vi.fn(async () => liveSession),
      stopLive: vi.fn(async () => undefined),
    })
    const liveCamera: Camera = {
      ...camera,
      capabilities: { ...camera.capabilities, live: available, archive: available },
    }

    render(<CameraExperience api={api} camera={liveCamera} flatId="flat-81" presentation="detail" />)

    expect(await screen.findByLabelText('Эфир с камеры Камера у подъезда')).toBeInTheDocument()
    expect(screen.getByRole('group', { name: 'Режим камеры' })).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Кадр' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Эфир' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('button', { name: 'Архив' })).toHaveAttribute('aria-pressed', 'false')
    await user.click(screen.getByRole('button', { name: 'Архив' }))
    expect(screen.getByRole('button', { name: 'Эфир' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('button', { name: 'Архив' })).toHaveAttribute('aria-pressed', 'true')
    expect(await screen.findByLabelText('Архив камеры Камера у подъезда')).toBeInTheDocument()
    expect(screen.queryByLabelText('Эфир с камеры Камера у подъезда')).not.toBeInTheDocument()
  })
})
