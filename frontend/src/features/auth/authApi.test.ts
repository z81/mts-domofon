import { describe, expect, it, vi } from 'vitest'
import {
  createProductionAuthApi,
  type AuthProgress,
  type SessionView,
} from './authApi'

const phoneProgress: AuthProgress = {
  status: 'challenge',
  flow_id: 'flow-123',
  challenge: { type: 'phone' },
}

const otpProgress: AuthProgress = {
  status: 'challenge',
  flow_id: 'flow-123',
  challenge: { type: 'sms_otp', resend_after_seconds: 30 },
}

const redirectProgress: AuthProgress = {
  status: 'browser_redirect',
  flow_id: 'flow-browser',
  authorization_url: 'https://login.mts.ru/amserver/UI/Login?realm=users',
}

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('production auth API boundary', () => {
  it('starts auth with the canonical Task 8 route and empty JSON body', async () => {
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(redirectProgress))
      .mockResolvedValueOnce(jsonResponse(phoneProgress))
    const api = createProductionAuthApi(fetcher)

    await expect(api.startAuth()).resolves.toEqual(redirectProgress)
    await expect(api.startAuth()).resolves.toEqual(phoneProgress)
    expect(fetcher).toHaveBeenCalledWith('/api/v1/auth/start', {
      body: '{}',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('submits Task 6 phone and OTP inputs to the flow-specific canonical route', async () => {
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(otpProgress))
      .mockResolvedValueOnce(jsonResponse({ status: 'authenticated', session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' } } satisfies AuthProgress))
    const api = createProductionAuthApi(fetcher)

    await api.submitChallenge('flow-123', { type: 'phone', phone: '+7 (999) 000-00-00' })
    await api.submitChallenge('flow-123', { type: 'sms_otp', code: '1234' })

    expect(fetcher).toHaveBeenNthCalledWith(1, '/api/v1/auth/flows/flow-123/challenge', {
      body: JSON.stringify({ type: 'phone', phone: '+7 (999) 000-00-00' }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
    expect(fetcher).toHaveBeenNthCalledWith(2, '/api/v1/auth/flows/flow-123/challenge', {
      body: JSON.stringify({ type: 'sms_otp', code: '1234' }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('reads and deletes the canonical Task 8 session resource', async () => {
    const session: SessionView = {
      authenticated: true,
      status: 'active',
      authenticated_at: '2026-08-06T09:00:00Z',
      expires_at: '2026-08-06T10:00:00Z',
    }
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(session))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    const api = createProductionAuthApi(fetcher)

    await expect(api.getSession()).resolves.toEqual(session)
    await api.logout()

    expect(fetcher).toHaveBeenNthCalledWith(1, '/api/v1/session', {
      credentials: 'same-origin',
      method: 'GET',
      signal: undefined,
    })
    expect(fetcher).toHaveBeenNthCalledWith(2, '/api/v1/session', {
      credentials: 'same-origin',
      method: 'DELETE',
    })
  })
})
