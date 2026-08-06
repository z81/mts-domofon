export interface AuthAccountOption {
  id: string
  label: string
}

// Task 8 deliberately keeps SessionStatus provider-neutral; the UI displays it
// but does not branch on undocumented values.
export type SessionStatus = string

export interface SessionView {
  authenticated: boolean
  status: SessionStatus
  authenticated_at: string | null
  expires_at: string | null
}

export type AuthChallenge =
  | { type: 'phone' }
  | { type: 'sms_otp'; resend_after_seconds: number }
  | { type: 'captcha'; image_data_url: string }
  | { type: 'password' }
  | { type: 'account_selection'; options: readonly AuthAccountOption[] }
  | { type: 'consent'; title: string; body: string }

export type ChallengeInput =
  | { type: 'phone'; phone: string }
  | { type: 'sms_otp'; code: string }
  | { type: 'captcha'; answer: string }
  | { type: 'password'; password: string }
  | { type: 'account'; id: string }
  | { type: 'consent'; accepted: boolean }

export type AuthProgress =
  | { status: 'browser_redirect'; flow_id: string; authorization_url: string }
  | { status: 'challenge'; flow_id: string; challenge: AuthChallenge }
  | { status: 'authenticated'; session: SessionView }

export interface ErrorEnvelope {
  code: string
  message: string
  retryable: boolean
  requestId: string
  retryAfterSeconds?: number
}

export class AuthApiError extends Error {
  readonly envelope: ErrorEnvelope

  constructor(envelope: ErrorEnvelope) {
    super(envelope.message)
    this.name = 'AuthApiError'
    this.envelope = envelope
  }
}

export interface AuthApi {
  startAuth(signal?: AbortSignal): Promise<AuthProgress>
  submitChallenge(flowId: string, input: ChallengeInput, signal?: AbortSignal): Promise<AuthProgress>
  getSession(signal?: AbortSignal): Promise<SessionView>
  logout(): Promise<void>
}

async function readLocalResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let envelope: ErrorEnvelope
    try {
      envelope = await response.json() as ErrorEnvelope
    } catch {
      envelope = {
        code: 'invalid_local_response',
        message: 'Локальный сервис вернул некорректный ответ',
        retryable: false,
        requestId: response.headers.get('X-Request-Id') ?? 'unknown',
      }
    }
    throw new AuthApiError(envelope)
  }
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

export function createProductionAuthApi(fetcher: typeof fetch = fetch): AuthApi {
  return {
    startAuth: async (signal) => readLocalResponse<AuthProgress>(await fetcher('/api/v1/auth/start', {
      body: JSON.stringify({}),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    })),
    submitChallenge: async (flowId, input, signal) => readLocalResponse<AuthProgress>(await fetcher(`/api/v1/auth/flows/${encodeURIComponent(flowId)}/challenge`, {
      body: JSON.stringify(input),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    })),
    getSession: async (signal) => readLocalResponse<SessionView>(await fetcher('/api/v1/session', {
      credentials: 'same-origin',
      method: 'GET',
      signal,
    })),
    logout: async () => readLocalResponse<void>(await fetcher('/api/v1/session', {
      credentials: 'same-origin',
      method: 'DELETE',
    })),
  }
}

function nextSyntheticProgress(flowId: string, input: ChallengeInput): AuthProgress {
  if (input.type === 'phone') return { status: 'challenge', flow_id: flowId, challenge: { type: 'sms_otp', resend_after_seconds: 30 } }
  if (input.type === 'captcha') return { status: 'challenge', flow_id: flowId, challenge: { type: 'password' } }
  if (input.type === 'password') return { status: 'challenge', flow_id: flowId, challenge: { type: 'account_selection', options: [{ id: 'demo-flat', label: 'Демо-квартира' }] } }
  if (input.type === 'account') return { status: 'challenge', flow_id: flowId, challenge: { type: 'consent', title: 'Доступ к домофону', body: 'Разрешить безопасный демонстрационный доступ' } }
  if (input.type === 'consent' && !input.accepted) return { status: 'challenge', flow_id: flowId, challenge: { type: 'consent', title: 'Доступ отклонён', body: 'Для входа необходимо согласие' } }
  return { status: 'authenticated', session: { authenticated: true, status: 'active', authenticated_at: new Date().toISOString(), expires_at: null } }
}

const syntheticAuthApi: AuthApi = {
  startAuth: async () => ({ status: 'challenge', flow_id: `preview-${crypto.randomUUID()}`, challenge: { type: 'phone' } }),
  submitChallenge: async (flowId, input) => nextSyntheticProgress(flowId, input),
  getSession: async () => ({ authenticated: false, status: 'anonymous', authenticated_at: null, expires_at: null }),
  logout: async () => undefined,
}

const defaultAuthApi = import.meta.env.DEV ? syntheticAuthApi : createProductionAuthApi()

export function startAuth(signal?: AbortSignal): Promise<AuthProgress> {
  return defaultAuthApi.startAuth(signal)
}

export function submitChallenge(flowId: string, input: ChallengeInput, signal?: AbortSignal): Promise<AuthProgress> {
  return defaultAuthApi.submitChallenge(flowId, input, signal)
}

export function getSession(signal?: AbortSignal): Promise<SessionView> {
  return defaultAuthApi.getSession(signal)
}

export function logout(): Promise<void> {
  return defaultAuthApi.logout()
}

export const authApi: AuthApi = { startAuth, submitChallenge, getSession, logout }
