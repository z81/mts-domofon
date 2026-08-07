import { render, screen, waitFor } from '@testing-library/react'
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

describe('archive player', () => {
  it('selects a real interval, starts HLS only by gesture, keeps sound, and stops on cleanup', async () => {
    const user = userEvent.setup()
    const getArchiveDistribution = vi.fn(async () => ({ intervals: [{
      from: '2026-08-07T09:00:00Z',
      to: '2026-08-07T10:00:00Z',
    }] }))
    const startArchive = vi.fn(async () => ({
      sessionId: '5d7a5769-adf9-4a41-9bd8-12427e0ae921',
      playlistUrl: '/api/v1/live/5d7a5769-adf9-4a41-9bd8-12427e0ae921/index.m3u8',
    }))
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

    expect(await screen.findByText(/07\.08.*—.*07\.08/)).toBeVisible()
    expect(startArchive).not.toHaveBeenCalled()
    await user.click(screen.getByRole('button', { name: 'Запустить архив' }))
    expect(startArchive).toHaveBeenCalledWith(
      'flat / 81', 'camera / 1', '2026-08-07T09:30:00.000Z', expect.any(AbortSignal),
    )
    const video = await screen.findByLabelText('Архив камеры Камера у подъезда')
    expect(video).toHaveAttribute('controls')
    expect(video).not.toHaveAttribute('muted')

    view.unmount()
    await waitFor(() => expect(stopLive).toHaveBeenCalledWith('5d7a5769-adf9-4a41-9bd8-12427e0ae921'))
  })
})
