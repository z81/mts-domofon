export type Capability =
  | { state: 'available' }
  | { state: 'unavailable'; reason?: string | null }

export interface CapabilitySet {
  accessPoints: Capability
  cameras: Capability
  archive: Capability
  events: Capability
  calls: Capability
  callSettings: Capability
}

export interface Flat {
  id: string
  label: string
  address: string
  capabilities: CapabilitySet
}

export type CallSettingsStatus =
  | { state: 'available'; settings: { mode: string; revision: number } }
  | { state: 'unavailable' }

export interface PropertyDashboard {
  flats: Flat[]
  selectedFlatId: string | null
  callSettings: CallSettingsStatus
}

export interface Lock {
  number: string
  name: string
  permanentlyOpen: boolean | null
}

export interface AccessPoint {
  id: string
  flatId: string
  name: string
  kind: 'intercom' | 'barrier' | 'gate' | 'elevator' | 'door' | string
  locks: Lock[]
  cameraIds: string[]
  available: boolean
}

export interface Camera {
  id: string
  flatId: string
  name: string
  accessPointId: string | null
  capabilities: {
    snapshot: Capability
    live: Capability
    archive: Capability
  }
}

export interface DeviceInventory {
  accessPoints: AccessPoint[]
  cameras: Camera[]
}

export interface DoorIntent {
  intentId: string
  expiresAt: string
}

export interface LiveSession {
  sessionId: string
  playlistUrl: string
}

export interface IntercomApi {
  getDashboard(signal?: AbortSignal): Promise<PropertyDashboard>
  getDevices(flatId: string, signal?: AbortSignal): Promise<DeviceInventory>
  getSnapshot(flatId: string, cameraId: string, signal?: AbortSignal): Promise<Blob>
  startLive?(flatId: string, cameraId: string, signal?: AbortSignal): Promise<LiveSession>
  stopLive?(sessionId: string): Promise<void>
  createDoorIntent(flatId: string, accessPointId: string, lockNumber: string, signal?: AbortSignal): Promise<DoorIntent>
  executeDoorIntent(intentId: string, signal?: AbortSignal): Promise<void>
}

interface ErrorBody {
  code?: unknown
}

export class IntercomApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code = 'local_api_error') {
    super('Local intercom API request failed')
    this.name = 'IntercomApiError'
    this.status = status
    this.code = code
  }
}

async function errorFrom(response: Response): Promise<IntercomApiError> {
  let code = 'local_api_error'
  try {
    const body = await response.json() as ErrorBody
    if (typeof body.code === 'string' && body.code.length <= 128) code = body.code
  } catch {
    // The UI only needs a stable local status/code and never renders raw bodies.
  }
  return new IntercomApiError(response.status, code)
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw await errorFrom(response)
  try {
    return await response.json() as T
  } catch {
    throw new IntercomApiError(502, 'invalid_local_response')
  }
}

async function readLiveSession(response: Response): Promise<LiveSession> {
  const value = await readJson<unknown>(response)
  if (!value || typeof value !== 'object') throw new IntercomApiError(502, 'invalid_live_response')
  const { sessionId, playlistUrl } = value as Partial<LiveSession>
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
  if (typeof sessionId !== 'string' || !uuid.test(sessionId)
    || playlistUrl !== `/api/v1/live/${sessionId}/index.m3u8`) {
    throw new IntercomApiError(502, 'invalid_live_response')
  }
  return { sessionId, playlistUrl }
}

const jsonRequest = (signal?: AbortSignal): RequestInit => ({
  credentials: 'same-origin',
  headers: { Accept: 'application/json' },
  method: 'GET',
  signal,
})

export function createProductionIntercomApi(fetcher: typeof fetch = fetch): IntercomApi {
  return {
    getDashboard: async (signal) => readJson<PropertyDashboard>(await fetcher('/api/v1/dashboard', jsonRequest(signal))),
    getDevices: async (flatId, signal) => readJson<DeviceInventory>(await fetcher(`/api/v1/devices?flatId=${encodeURIComponent(flatId)}`, jsonRequest(signal))),
    getSnapshot: async (flatId, cameraId, signal) => {
      const response = await fetcher(`/api/v1/cameras/${encodeURIComponent(cameraId)}/snapshot?flatId=${encodeURIComponent(flatId)}`, {
        credentials: 'same-origin',
        headers: { Accept: 'image/webp, image/jpeg, image/png' },
        method: 'GET',
        signal,
      })
      if (!response.ok) throw await errorFrom(response)

      const contentType = response.headers.get('Content-Type')?.toLowerCase() ?? ''
      if (!['image/webp', 'image/jpeg', 'image/png'].includes(contentType)) {
        throw new IntercomApiError(502, 'invalid_snapshot_response')
      }
      const blob = await response.blob()
      if (blob.size === 0) throw new IntercomApiError(502, 'invalid_snapshot_response')
      return blob
    },
    startLive: async (flatId, cameraId, signal) => readLiveSession(await fetcher(`/api/v1/cameras/${encodeURIComponent(cameraId)}/live?flatId=${encodeURIComponent(flatId)}`, {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
      method: 'POST',
      signal,
    })),
    stopLive: async (sessionId) => {
      const response = await fetcher(`/api/v1/live/${encodeURIComponent(sessionId)}`, {
        credentials: 'same-origin',
        method: 'DELETE',
      })
      if (!response.ok) throw await errorFrom(response)
      if (response.status !== 204) throw new IntercomApiError(502, 'invalid_local_response')
    },
    createDoorIntent: async (flatId, accessPointId, lockNumber, signal) => readJson<DoorIntent>(await fetcher('/api/v1/access/intents', {
      body: JSON.stringify({ flatId, accessPointId, lockNumber }),
      credentials: 'same-origin',
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    })),
    executeDoorIntent: async (intentId, signal) => {
      const response = await fetcher(`/api/v1/access/intents/${encodeURIComponent(intentId)}/execute`, {
        credentials: 'same-origin',
        method: 'POST',
        signal,
      })
      if (!response.ok) throw await errorFrom(response)
      if (response.status !== 204) throw new IntercomApiError(502, 'invalid_local_response')
    },
  }
}

export const intercomApi = createProductionIntercomApi()
