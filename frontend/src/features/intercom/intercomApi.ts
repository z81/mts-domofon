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

export interface IntercomApi {
  getDashboard(signal?: AbortSignal): Promise<PropertyDashboard>
  getDevices(flatId: string, signal?: AbortSignal): Promise<DeviceInventory>
  getSnapshot(flatId: string, cameraId: string, signal?: AbortSignal): Promise<Blob>
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
  }
}

export const intercomApi = createProductionIntercomApi()
