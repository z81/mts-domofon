export interface AuthAccount {
  id: string
  label: string
}

export interface AuthSession {
  userId: string
  displayName: string
}

export type AuthChallenge =
  | { kind: 'phone' }
  | { kind: 'otp'; destination: string }
  | { kind: 'captcha'; imageAlt: string }
  | { kind: 'password' }
  | { kind: 'account_selection'; accounts: readonly AuthAccount[] }
  | { kind: 'consent'; text: string }
  | { kind: 'consent_rejected' }
  | { kind: 'rate_limited'; retryAfterSeconds: number }

export type AuthChallengeInput =
  | { kind: 'phone'; value: string }
  | { kind: 'otp'; value: string }
  | { kind: 'captcha'; value: string }
  | { kind: 'password'; value: string }
  | { kind: 'account_selection'; accountId: string }
  | { kind: 'consent'; accepted: boolean }

export interface AuthFlowResult {
  flowId: string
  challenge?: AuthChallenge
  session?: AuthSession
}

export interface AuthApi {
  startAuth(signal?: AbortSignal): Promise<AuthFlowResult>
  submitChallenge(flowId: string, input: AuthChallengeInput, signal?: AbortSignal): Promise<AuthFlowResult>
  getSession(signal?: AbortSignal): Promise<AuthSession | null>
  logout(): Promise<void>
}

async function localRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api/auth/${path}`, {
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
  if (!response.ok) throw new Error(`Auth request failed: ${response.status}`)
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

const productionAuthApi: AuthApi = {
  startAuth: (signal) => localRequest<AuthFlowResult>('start', { method: 'POST', signal }),
  submitChallenge: (flowId, input, signal) => localRequest<AuthFlowResult>('challenge', { method: 'POST', signal, body: JSON.stringify({ flowId, input }) }),
  getSession: (signal) => localRequest<AuthSession | null>('session', { signal }),
  logout: () => localRequest<void>('logout', { method: 'POST' }),
}

function nextSyntheticChallenge(input: AuthChallengeInput): AuthChallenge | undefined {
  if (input.kind === 'phone') return { kind: 'otp', destination: '+7 ••• •••-00-00' }
  if (input.kind === 'otp') return { kind: 'account_selection', accounts: [{ id: 'demo-flat', label: 'Демо-квартира' }] }
  if (input.kind === 'captcha') return { kind: 'password' }
  if (input.kind === 'password') return { kind: 'account_selection', accounts: [{ id: 'demo-flat', label: 'Демо-квартира' }] }
  if (input.kind === 'account_selection') return { kind: 'consent', text: 'Разрешить доступ к домофону' }
  if (input.kind === 'consent' && !input.accepted) return { kind: 'consent_rejected' }
  return undefined
}

const syntheticAuthApi: AuthApi = {
  startAuth: async () => ({ flowId: `preview-${crypto.randomUUID()}`, challenge: { kind: 'phone' } }),
  submitChallenge: async (flowId, input) => {
    const challenge = nextSyntheticChallenge(input)
    if (challenge) return { flowId, challenge }
    return { flowId, session: { userId: 'preview-user', displayName: 'Мария · demo' } }
  },
  getSession: async () => null,
  logout: async () => undefined,
}

const defaultAuthApi = import.meta.env.DEV ? syntheticAuthApi : productionAuthApi

export const startAuth = (signal?: AbortSignal) => defaultAuthApi.startAuth(signal)
export const submitChallenge = (flowId: string, input: AuthChallengeInput, signal?: AbortSignal) => defaultAuthApi.submitChallenge(flowId, input, signal)
export const getSession = (signal?: AbortSignal) => defaultAuthApi.getSession(signal)
export const logout = () => defaultAuthApi.logout()

export const authApi: AuthApi = { startAuth, submitChallenge, getSession, logout }
