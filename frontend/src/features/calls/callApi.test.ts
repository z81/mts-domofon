import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  createProductionCallApi,
  startCurrentCallPolling,
  type CallApi,
  type CurrentCall,
} from './callApi'

const ringingCall: CurrentCall = {
  id: 'call / 81',
  revision: 4,
  phase: 'ringing',
  callerName: 'Подъезд 1',
  startedAt: '2026-08-06T12:00:00Z',
  remoteSdp: 'v=0\r\na=ice-ufrag:test\r\n',
  previewUrl: '/api/v1/calls/call%2081/preview',
  canOpen: true,
}

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve
  })
  return { promise, resolve }
}

function fakeCallApi(getCurrent: CallApi['getCurrent']): CallApi {
  return {
    answer: async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }),
    createOpenIntent: async () => ({ intentId: 'intent-1', expiresAt: '2026-08-06T12:00:30Z' }),
    decline: async () => undefined,
    executeOpenIntent: async () => undefined,
    getCurrent,
    hangup: async () => undefined,
    ignore: async () => undefined,
  }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('production call API boundary', () => {
  it('returns null for a successful no-current-call response', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response(null, { status: 204 }))
    const api = createProductionCallApi(fetcher)
    const signal = new AbortController().signal

    await expect(api.getCurrent(signal)).resolves.toBeNull()
    expect(fetcher).toHaveBeenCalledWith('/api/v1/calls/current', {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
      method: 'GET',
      signal,
    })
  })

  it('returns the real current call without replacing its SDP', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(ringingCall))
    const api = createProductionCallApi(fetcher)

    await expect(api.getCurrent()).resolves.toEqual(ringingCall)
  })

  it('answers an encoded call with its exact revision and candidate-complete SDP', async () => {
    const connectingCall: CurrentCall = { ...ringingCall, phase: 'connecting', revision: 5 }
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(connectingCall))
    const api = createProductionCallApi(fetcher)

    await expect(api.answer('call / 81', 4, 'v=0\r\na=candidate:complete\r\n')).resolves.toEqual(connectingCall)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/calls/call%20%2F%2081/answer', {
      body: JSON.stringify({ revision: 4, sdp: 'v=0\r\na=candidate:complete\r\n' }),
      credentials: 'same-origin',
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('posts each non-media command to its distinct call route', async () => {
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValue(new Response(null, { status: 204 }))
    const api = createProductionCallApi(fetcher)

    await api.decline('call / 81', 4)
    await api.ignore('call / 81', 5)
    await api.hangup('call / 81', 6)

    expect(fetcher.mock.calls.map(([path, request]) => [path, request?.body])).toEqual([
      ['/api/v1/calls/call%20%2F%2081/decline', JSON.stringify({ revision: 4 })],
      ['/api/v1/calls/call%20%2F%2081/ignore', JSON.stringify({ revision: 5 })],
      ['/api/v1/calls/call%20%2F%2081/hangup', JSON.stringify({ revision: 6 })],
    ])
  })

  it('creates and executes a call-bound open intent with encoded opaque ids', async () => {
    const intent = { intentId: 'intent / 2', expiresAt: '2026-08-06T12:00:30Z' }
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(intent, 201))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    const api = createProductionCallApi(fetcher)

    await expect(api.createOpenIntent('call / 81', 4)).resolves.toEqual(intent)
    await expect(api.executeOpenIntent('call / 81', 'intent / 2', 4)).resolves.toBeUndefined()

    expect(fetcher.mock.calls).toEqual([
      ['/api/v1/calls/call%20%2F%2081/open-intents', expect.objectContaining({
        body: JSON.stringify({ revision: 4 }),
        method: 'POST',
      })],
      ['/api/v1/calls/call%20%2F%2081/open-intents/intent%20%2F%202/execute', expect.objectContaining({
        body: JSON.stringify({ revision: 4 }),
        method: 'POST',
      })],
    ])
  })
})

describe('current-call polling', () => {
  it('waits one second after a request settles before starting the next request', async () => {
    vi.useFakeTimers()
    const first = deferred<CurrentCall | null>()
    const getCurrent = vi.fn<CallApi['getCurrent']>()
      .mockImplementationOnce(() => first.promise)
      .mockResolvedValueOnce(null)
    const updates: Array<CurrentCall | null> = []
    const stop = startCurrentCallPolling(fakeCallApi(getCurrent), {
      onUpdate: (call) => updates.push(call),
    })

    expect(getCurrent).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(getCurrent).toHaveBeenCalledTimes(1)

    first.resolve(ringingCall)
    await Promise.resolve()
    expect(updates).toEqual([ringingCall])
    await vi.advanceTimersByTimeAsync(999)
    expect(getCurrent).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(getCurrent).toHaveBeenCalledTimes(2)

    stop()
  })

  it('aborts the in-flight request and publishes nothing after cleanup', async () => {
    const pending = deferred<CurrentCall | null>()
    let capturedSignal: AbortSignal | undefined
    const onUpdate = vi.fn()
    const stop = startCurrentCallPolling(fakeCallApi((_signal) => {
      capturedSignal = _signal
      return pending.promise
    }), { onUpdate })

    stop()
    expect(capturedSignal?.aborted).toBe(true)
    pending.resolve(ringingCall)
    await Promise.resolve()
    expect(onUpdate).not.toHaveBeenCalled()
  })
})
