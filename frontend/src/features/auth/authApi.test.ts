import { describe, expect, it, vi } from 'vitest'
import {
  createDefaultAuthApi,
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
  it('uses the explicit legacy SMS routes with their exact canonical JSON bodies', async () => {
    const authenticated: AuthProgress = {
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' },
    }
    const fetcher = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(otpProgress))
      .mockResolvedValueOnce(jsonResponse(authenticated))
    const api = createProductionAuthApi(fetcher)

    await expect(api.startLegacySms({ phone: '+79990000000', accept_legacy_sms: true })).resolves.toEqual(otpProgress)
    await expect(api.confirmLegacySms('flow / opaque', '123456')).resolves.toEqual(authenticated)

    expect(fetcher).toHaveBeenNthCalledWith(1, '/api/v1/auth/legacy-sms/start', {
      body: JSON.stringify({ phone: '+79990000000', accept_legacy_sms: true }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
    expect(fetcher).toHaveBeenNthCalledWith(2, '/api/v1/auth/legacy-sms/flows/flow%20%2F%20opaque/confirm', {
      body: JSON.stringify({ code: '123456' }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

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

  it('submits the opaque callback only in the canonical JSON request body', async () => {
    const authenticated: AuthProgress = {
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' },
    }
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(authenticated))
    const api = createProductionAuthApi(fetcher)
    const signal = new AbortController().signal
    const callbackUrl = 'https://gateway.vdome.mts.ru/user-service/api/auth/callback?code=callback-secret&state=state-secret'

    await expect(api.completeCallback('flow / opaque', callbackUrl, signal)).resolves.toEqual(authenticated)

    expect(fetcher).toHaveBeenCalledWith('/api/v1/auth/flows/flow%20%2F%20opaque/callback', {
      body: JSON.stringify({ callback_url: callbackUrl }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    })
    expect(fetcher.mock.calls[0]?.[0]).not.toContain('callback-secret')
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

  it('uses the production API in normal development and synthetic preview only by explicit opt-in', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(redirectProgress))
    const normalDevelopment = createDefaultAuthApi({ dev: true, syntheticPreviewEnabled: false, fetcher })

    await expect(normalDevelopment.startAuth()).resolves.toEqual(redirectProgress)
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(normalDevelopment.isSyntheticPreview).toBe(false)

    const explicitPreview = createDefaultAuthApi({ dev: true, syntheticPreviewEnabled: true, fetcher })
    await expect(explicitPreview.startAuth()).resolves.toMatchObject({ status: 'challenge' })
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(explicitPreview.isSyntheticPreview).toBe(true)

    const production = createDefaultAuthApi({ dev: false, syntheticPreviewEnabled: true, fetcher })
    expect(production.isSyntheticPreview).toBe(false)
  })
})
