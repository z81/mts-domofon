export type CallPhase = 'ringing' | 'connecting' | 'active' | 'ending'

export interface CurrentCall {
  id: string
  revision: number
  phase: CallPhase
  callerName: string
  startedAt: string
  remoteSdp: string
  previewUrl?: string
  canOpen: boolean
}

export interface CallOpenIntent {
  intentId: string
  expiresAt: string
}

export type AutoOpenMode = 'disabled' | 'next_call' | 'until'
export type AutoOpenResultStatus = 'opened' | 'failed' | 'outcome_unknown'

export interface AutoOpenResult {
  status: AutoOpenResultStatus
  completedAt: string
}

export interface AutoOpenPolicy {
  mode: AutoOpenMode
  until?: string
  dismissAfterOpen: boolean
  lastResult?: AutoOpenResult
}

export interface AutoOpenIntent {
  intentId: string
  expiresAt: string
}

export interface CallApi {
  getCurrent(signal?: AbortSignal): Promise<CurrentCall | null>
  answer(callId: string, revision: number, sdp: string, signal?: AbortSignal): Promise<CurrentCall>
  decline(callId: string, revision: number, signal?: AbortSignal): Promise<void>
  ignore(callId: string, revision: number, signal?: AbortSignal): Promise<void>
  hangup(callId: string, revision: number, signal?: AbortSignal): Promise<void>
  createOpenIntent(callId: string, revision: number, signal?: AbortSignal): Promise<CallOpenIntent>
  executeOpenIntent(callId: string, intentId: string, revision: number, signal?: AbortSignal): Promise<void>
  getAutoOpenPolicy?(signal?: AbortSignal): Promise<AutoOpenPolicy>
  createAutoOpenIntent?(mode: Exclude<AutoOpenMode, 'disabled'>, dismissAfterOpen: boolean, signal?: AbortSignal): Promise<AutoOpenIntent>
  executeAutoOpenIntent?(intentId: string, signal?: AbortSignal): Promise<AutoOpenPolicy>
  disableAutoOpen?(signal?: AbortSignal): Promise<void>
}

interface ErrorBody {
  code?: unknown
}

export class CallApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code = 'local_api_error') {
    super('Local call API request failed')
    this.name = 'CallApiError'
    this.status = status
    this.code = code
  }
}

async function apiError(response: Response): Promise<CallApiError> {
  let code = 'local_api_error'
  try {
    const body = await response.json() as ErrorBody
    if (typeof body.code === 'string' && body.code.length <= 128) code = body.code
  } catch {
    // Never surface a raw local/upstream response to the browser UI.
  }
  return new CallApiError(response.status, code)
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw await apiError(response)
  try {
    return await response.json() as T
  } catch {
    throw new CallApiError(502, 'invalid_local_response')
  }
}

async function readNoContent(response: Response): Promise<void> {
  if (!response.ok) throw await apiError(response)
  if (response.status !== 204) throw new CallApiError(502, 'invalid_local_response')
}

const getRequest = (signal?: AbortSignal): RequestInit => ({
  credentials: 'same-origin',
  headers: { Accept: 'application/json' },
  method: 'GET',
  signal,
})

const postRequest = (body: unknown, signal?: AbortSignal): RequestInit => ({
  body: JSON.stringify(body),
  credentials: 'same-origin',
  headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
  method: 'POST',
  signal,
})

export function createProductionCallApi(fetcher: typeof fetch = fetch): CallApi {
  const callRoute = (callId: string, action: string) => `/api/v1/calls/${encodeURIComponent(callId)}/${action}`
  const command = async (callId: string, action: 'decline' | 'ignore' | 'hangup', revision: number, signal?: AbortSignal) => {
    await readNoContent(await fetcher(callRoute(callId, action), postRequest({ revision }, signal)))
  }

  return {
    getCurrent: async (signal) => {
      const response = await fetcher('/api/v1/calls/current', getRequest(signal))
      if (response.status === 204) return null
      return readJson<CurrentCall>(response)
    },
    answer: async (callId, revision, sdp, signal) => readJson<CurrentCall>(await fetcher(
      callRoute(callId, 'answer'),
      postRequest({ revision, sdp }, signal),
    )),
    decline: async (callId, revision, signal) => command(callId, 'decline', revision, signal),
    ignore: async (callId, revision, signal) => command(callId, 'ignore', revision, signal),
    hangup: async (callId, revision, signal) => command(callId, 'hangup', revision, signal),
    createOpenIntent: async (callId, revision, signal) => readJson<CallOpenIntent>(await fetcher(
      callRoute(callId, 'open-intents'),
      postRequest({ revision }, signal),
    )),
    executeOpenIntent: async (callId, intentId, revision, signal) => {
      const path = `${callRoute(callId, 'open-intents')}/${encodeURIComponent(intentId)}/execute`
      await readNoContent(await fetcher(path, postRequest({ revision }, signal)))
    },
    getAutoOpenPolicy: async (signal) => readJson<AutoOpenPolicy>(await fetcher(
      '/api/v1/calls/auto-open', getRequest(signal),
    )),
    createAutoOpenIntent: async (mode, dismissAfterOpen, signal) => readJson<AutoOpenIntent>(await fetcher(
      '/api/v1/calls/auto-open/intents', postRequest({ mode, dismissAfterOpen }, signal),
    )),
    executeAutoOpenIntent: async (intentId, signal) => readJson<AutoOpenPolicy>(await fetcher(
      `/api/v1/calls/auto-open/intents/${encodeURIComponent(intentId)}/execute`,
      postRequest({}, signal),
    )),
    disableAutoOpen: async (signal) => readNoContent(await fetcher('/api/v1/calls/auto-open', {
      credentials: 'same-origin',
      method: 'DELETE',
      signal,
    })),
  }
}

export interface CurrentCallPollingCallbacks {
  onUpdate(call: CurrentCall | null): void
  onError?(error: unknown): void
}

export function startCurrentCallPolling(
  api: Pick<CallApi, 'getCurrent'>,
  callbacks: CurrentCallPollingCallbacks,
): () => void {
  let stopped = false
  let controller: AbortController | null = null
  let timer: number | null = null

  const poll = async () => {
    controller = new AbortController()
    try {
      const call = await api.getCurrent(controller.signal)
      if (!stopped) callbacks.onUpdate(call)
    } catch (error) {
      if (!stopped && !controller.signal.aborted) callbacks.onError?.(error)
    } finally {
      controller = null
      if (!stopped) timer = window.setTimeout(() => void poll(), 1_000)
    }
  }

  void poll()
  return () => {
    stopped = true
    if (timer !== null) window.clearTimeout(timer)
    controller?.abort()
  }
}

export const callApi = createProductionCallApi()
