import { describe, expect, it, vi } from 'vitest'
import { IntercomApiError, createProductionIntercomApi, type DeviceInventory, type PropertyDashboard } from './intercomApi'

const dashboard: PropertyDashboard = {
  flats: [],
  selectedFlatId: null,
  callSettings: { state: 'unavailable' },
}

const inventory: DeviceInventory = {
  accessPoints: [],
  cameras: [],
}

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('production intercom API boundary', () => {
  it('loads the dashboard from the versioned same-origin route', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(dashboard))
    const api = createProductionIntercomApi(fetcher)
    const signal = new AbortController().signal

    await expect(api.getDashboard(signal)).resolves.toEqual(dashboard)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/dashboard', {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
      method: 'GET',
      signal,
    })
  })

  it('encodes an opaque flat id when loading its devices', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(inventory))
    const api = createProductionIntercomApi(fetcher)

    await expect(api.getDevices('flat / №81')).resolves.toEqual(inventory)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/devices?flatId=flat%20%2F%20%E2%84%9681', {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
      method: 'GET',
      signal: undefined,
    })
  })

  it('loads an image snapshot with both opaque path segments encoded', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response('image-bytes', {
      status: 200,
      headers: { 'Content-Type': 'image/webp' },
    }))
    const api = createProductionIntercomApi(fetcher)

    const result = await api.getSnapshot('flat/81', 'camera front/1')

    expect(result.type).toBe('image/webp')
    expect(result.size).toBe(11)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/cameras/camera%20front%2F1/snapshot?flatId=flat%2F81', {
      credentials: 'same-origin',
      headers: { Accept: 'image/webp, image/jpeg, image/png' },
      method: 'GET',
      signal: undefined,
    })
  })

  it('preserves a local 401 status without exposing an untrusted response body', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse({
      code: 'authentication_required',
      message: 'internal upstream details must not reach the UI',
      retryable: false,
      requestId: 'request-1',
    }, 401))
    const api = createProductionIntercomApi(fetcher)
    const request = api.getDashboard()

    await expect(request).rejects.toEqual(expect.objectContaining<Partial<IntercomApiError>>({
      code: 'authentication_required',
      status: 401,
    }))
    await expect(request).rejects.toThrow('Local intercom API request failed')
  })

  it('rejects a successful snapshot response that is not an image', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response('<html>not an image</html>', {
      status: 200,
      headers: { 'Content-Type': 'text/html' },
    }))
    const api = createProductionIntercomApi(fetcher)

    await expect(api.getSnapshot('flat-1', 'camera-1')).rejects.toMatchObject({
      code: 'invalid_snapshot_response',
      status: 502,
    })
  })

  it('creates a door intent without executing the physical command', async () => {
    const intent = { intentId: 'intent / 1', expiresAt: '2026-08-06T12:00:30Z' }
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(intent, 201))
    const api = createProductionIntercomApi(fetcher)

    await expect(api.createDoorIntent('flat / 81', 'entry & 1', 'lock/2')).resolves.toEqual(intent)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/access/intents', {
      body: JSON.stringify({ flatId: 'flat / 81', accessPointId: 'entry & 1', lockNumber: 'lock/2' }),
      credentials: 'same-origin',
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('executes an encoded door intent with an explicitly empty request body', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(new Response(null, { status: 204 }))
    const api = createProductionIntercomApi(fetcher)

    await expect(api.executeDoorIntent('intent / 1')).resolves.toBeUndefined()
    expect(fetcher).toHaveBeenCalledWith('/api/v1/access/intents/intent%20%2F%201/execute', {
      credentials: 'same-origin',
      method: 'POST',
      signal: undefined,
    })
  })
})
