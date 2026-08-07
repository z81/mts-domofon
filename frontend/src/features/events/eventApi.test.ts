import { describe, expect, it, vi } from 'vitest'
import { createProductionEventApi, type EventPage } from './eventApi'

const page: EventPage = { items: [], page: 2, hasMore: false }

describe('production event API boundary', () => {
  it('encodes the complete bounded journal query', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response(JSON.stringify(page), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }))
    const api = createProductionEventApi(fetcher)
    const signal = new AbortController().signal

    await expect(api.getEvents({
      flatId: 'flat / 81',
      page: 2,
      size: 20,
      from: '2026-08-01T00:00:00.000Z',
      to: '2026-08-07T23:59:59.000Z',
      kind: 'call',
    }, signal)).resolves.toEqual(page)

    expect(fetcher).toHaveBeenCalledWith(
      '/api/v1/events?flatId=flat+%2F+81&page=2&size=20&from=2026-08-01T00%3A00%3A00.000Z&to=2026-08-07T23%3A59%3A59.000Z&kind=call',
      {
        credentials: 'same-origin',
        headers: { Accept: 'application/json' },
        method: 'GET',
        signal,
      },
    )
  })

  it('keeps provider details out of the surfaced error', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response(JSON.stringify({
      code: 'upstream_unavailable',
      message: 'secret provider body',
    }), { status: 502 }))
    const api = createProductionEventApi(fetcher)

    const request = api.getEvents({
      flatId: 'flat-1', page: 0, size: 3,
      from: '2026-08-01T00:00:00Z', to: '2026-08-07T00:00:00Z',
    })
    await expect(request).rejects.toMatchObject({ status: 502, code: 'upstream_unavailable' })
    await expect(request).rejects.not.toThrow('secret provider body')
  })
})
