import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CameraExperience } from './CameraExperience'
import type { Camera, IntercomApi } from './intercomApi'

const available = { state: 'available' } as const
const unavailable = { state: 'unavailable', reason: null } as const

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

beforeEach(() => {
  Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: vi.fn(() => 'blob:snapshot-1') })
  Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: vi.fn() })
})

describe('CameraExperience', () => {
  it('shows a preview snapshot and opens camera detail on demand', async () => {
    const user = userEvent.setup()
    const api = createApi()
    const openDetail = vi.fn()

    render(<CameraExperience api={api} camera={camera} flatId="flat-81" presentation="preview" onOpenDetail={openDetail} />)

    const snapshot = await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })
    expect(snapshot).toBeVisible()
    expect(snapshot.closest('article')).toHaveClass('camera-card', 'camera-preview')
    expect(screen.getByText('Доступно: кадр')).toBeVisible()
    expect(screen.queryByText('Кадр получен от локального API')).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Смотреть Камера у подъезда' }))
    expect(openDetail).toHaveBeenCalledTimes(1)
  })

  it('keeps a live-only camera idle without a dead snapshot mode until a user gesture', () => {
    const getSnapshot = vi.fn<IntercomApi['getSnapshot']>()
    const api = createApi({ getSnapshot, startLive: vi.fn(), stopLive: vi.fn() })
    const liveCamera: Camera = {
      ...camera,
      capabilities: { ...camera.capabilities, live: available, snapshot: unavailable },
    }

    render(<CameraExperience api={api} camera={liveCamera} flatId="flat-81" presentation="detail" />)

    expect(screen.getByRole('button', { name: 'Эфир' })).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Кадр' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Архив' })).not.toBeInTheDocument()
    expect(screen.getByText('Выберите режим камеры')).toBeVisible()
    expect(getSnapshot).not.toHaveBeenCalled()
    expect(api.startLive).not.toHaveBeenCalled()
  })

  it('marks the selected camera mode for the active visual tab', async () => {
    const user = userEvent.setup()
    const api = createApi({
      startLive: vi.fn(async () => ({ sessionId: 'session-1', playlistUrl: '/api/v1/live/session-1/index.m3u8' })),
      stopLive: vi.fn(async () => undefined),
    })
    const liveCamera: Camera = {
      ...camera,
      capabilities: { ...camera.capabilities, live: available },
    }

    render(<CameraExperience api={api} camera={liveCamera} flatId="flat-81" presentation="detail" />)

    await screen.findByRole('img', { name: 'Кадр с камеры Камера у подъезда' })
    expect(screen.getByRole('button', { name: 'Кадр' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('button', { name: 'Эфир' })).toHaveAttribute('aria-pressed', 'false')
    await user.click(screen.getByRole('button', { name: 'Эфир' }))
    expect(screen.getByRole('button', { name: 'Кадр' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('button', { name: 'Эфир' })).toHaveAttribute('aria-pressed', 'true')
  })
})
