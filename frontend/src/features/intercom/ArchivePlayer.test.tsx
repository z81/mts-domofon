import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ArchivePlayer } from './ArchivePlayer'
import type { IntercomApi } from './intercomApi'

function archiveApi(overrides: Partial<IntercomApi> = {}): IntercomApi {
  return {
    createDoorIntent: async () => ({ intentId: 'intent', expiresAt: '2026-08-07T12:00:00Z' }),
    executeDoorIntent: async () => undefined,
    getDashboard: async () => ({ flats: [], selectedFlatId: null, callSettings: { state: 'unavailable' } }),
    getDevices: async () => ({ accessPoints: [], cameras: [] }),
    getSnapshot: async () => new Blob(['image'], { type: 'image/webp' }),
    ...overrides,
  }
}

function localInput(value: string): string {
  const date = new Date(value)
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 19)
}

describe('archive player', () => {
  it('starts at the event without a click, keeps sound, restarts only on demand, and stops on cleanup', async () => {
    const user = userEvent.setup()
    const getArchiveDistribution = vi.fn(async () => ({ intervals: [{
      from: '2026-08-07T09:00:00Z',
      to: '2026-08-07T10:00:00Z',
    }] }))
    const startArchive = vi.fn<NonNullable<IntercomApi['startArchive']>>()
      .mockResolvedValueOnce({
        sessionId: '5d7a5769-adf9-4a41-9bd8-12427e0ae921',
        playlistUrl: '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8',
      })
      .mockResolvedValueOnce({
        sessionId: '0b3f7c2e-6a51-4c8d-9e47-2f1d8a6b5c93',
        playlistUrl: '/api/v1/live/0b3f7c2e-6a51-4c8d-9e47-2f1d8a6b5c93/index.m3u8',
      })
    const stopLive = vi.fn(async () => undefined)
    vi.spyOn(HTMLMediaElement.prototype, 'canPlayType').mockReturnValue('probably')
    vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue()
    vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined)
    vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => undefined)
    const view = render(<ArchivePlayer
      api={archiveApi({ getArchiveDistribution, startArchive, stopLive })}
      cameraId="camera / 1"
      cameraName="Камера у подъезда"
      flatId="flat / 81"
      initialAt="2026-08-07T09:30:00Z"
    />)

    const interval = await screen.findByRole('button', { name: /^\d\d:\d\d–\d\d:\d\d$/ })
    expect(screen.getByRole('group', { name: 'Доступные интервалы' })).toContainElement(interval)
    expect(interval).toHaveAttribute('aria-pressed', 'true')
    await waitFor(() => expect(startArchive).toHaveBeenCalledWith('flat / 81', 'camera / 1', '2026-08-07T09:29:50.000Z'))
    const start = screen.getByLabelText('Начало просмотра') as HTMLInputElement
    expect(new Date(start.value).toISOString()).toBe('2026-08-07T09:29:50.000Z')
    expect(start).toHaveAttribute('step', '1')
    const video = screen.getByLabelText('Архив камеры Камера у подъезда') as HTMLVideoElement
    await waitFor(() => expect(video).toHaveAttribute('src', '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8'))
    expect(video.muted).toBe(false)
    act(() => {
      video.dispatchEvent(new Event('playing'))
      video.currentTime = 1
      video.dispatchEvent(new Event('timeupdate'))
    })
    expect(video).toHaveAttribute('controls')
    expect(screen.getByText(/^Запись: \d\d\.\d\d \d\d:\d\d:51$/)).toBeVisible()

    fireEvent.change(start, { target: { value: localInput('2026-08-07T09:45:30Z') } })
    expect(screen.getByRole('button', { name: /^Смотреть с \d\d:\d\d:30$/ })).toBeVisible()
    expect(startArchive).toHaveBeenCalledTimes(1)

    await user.click(interval)
    await waitFor(() => expect(startArchive).toHaveBeenLastCalledWith('flat / 81', 'camera / 1', '2026-08-07T09:00:00.000Z'))
    expect(stopLive).toHaveBeenCalledWith('5d7a5769-adf9-4a41-9bd8-12427e0ae921')

    view.unmount()
    await waitFor(() => expect(stopLive).toHaveBeenCalledWith('0b3f7c2e-6a51-4c8d-9e47-2f1d8a6b5c93'))
  })
})
