export type EventKind = 'call' | 'access' | 'camera' | 'other'
export type CallOutcome = 'accepted' | 'missed' | 'rejected' | 'ignored' | 'unknown'

export interface EventItem {
  id: string
  occurredAt: string
  kind: EventKind
  title: string
  sourceLabel?: string
  callOutcome?: CallOutcome
  durationSeconds?: number
  cameraId?: string
  archiveAt?: string
}

export interface EventPage {
  items: EventItem[]
  page: number
  hasMore: boolean
}

export interface EventQuery {
  flatId: string
  page: number
  size: number
  from: string
  to: string
  kind?: EventKind
}

export interface EventApi {
  getEvents(query: EventQuery, signal?: AbortSignal): Promise<EventPage>
}

export class EventApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code = 'local_api_error') {
    super('Local event API request failed')
    this.name = 'EventApiError'
    this.status = status
    this.code = code
  }
}

async function responseError(response: Response): Promise<EventApiError> {
  let code = 'local_api_error'
  try {
    const body = await response.json() as { code?: unknown }
    if (typeof body.code === 'string' && body.code.length <= 128) code = body.code
  } catch {
    // Raw provider and local response bodies are never rendered.
  }
  return new EventApiError(response.status, code)
}

async function readPage(response: Response): Promise<EventPage> {
  if (!response.ok) throw await responseError(response)
  try {
    return await response.json() as EventPage
  } catch {
    throw new EventApiError(502, 'invalid_local_response')
  }
}

export function createProductionEventApi(fetcher: typeof fetch = fetch): EventApi {
  return {
    getEvents: async (query, signal) => {
      const parameters = new URLSearchParams()
      parameters.set('flatId', query.flatId)
      parameters.set('page', String(query.page))
      parameters.set('size', String(query.size))
      parameters.set('from', query.from)
      parameters.set('to', query.to)
      if (query.kind) parameters.set('kind', query.kind)
      return readPage(await fetcher(`/api/v1/events?${parameters}`, {
        credentials: 'same-origin',
        headers: { Accept: 'application/json' },
        method: 'GET',
        signal,
      }))
    },
  }
}

export const eventApi = createProductionEventApi()
