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
  it('imports the MTS JSON response through the canonical local endpoint', async () => {
    const authenticated: AuthProgress = {
      status: 'authenticated',
      session: { authenticated: true, status: 'active', authenticated_at: '2026-08-06T09:00:00Z', expires_at: '2026-08-06T10:00:00Z' },
    }
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(authenticated))
    const api = createProductionAuthApi(fetcher)
    const callbackJson = '{"access_token":"import-secret"}'

    await expect(api.importWebSsoResponse(callbackJson)).resolves.toEqual(authenticated)

    expect(fetcher).toHaveBeenCalledWith('/api/v1/auth/web-sso/import', {
      body: JSON.stringify({ response: callbackJson }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('starts auth with the canonical route and empty JSON body', async () => {
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

  it('submits challenge inputs to the flow-specific canonical route', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(phoneProgress))
    const api = createProductionAuthApi(fetcher)

    await api.submitChallenge('flow-123', { type: 'phone', phone: '+7 (999) 000-00-00' })

    expect(fetcher).toHaveBeenCalledWith('/api/v1/auth/flows/flow-123/challenge', {
      body: JSON.stringify({ type: 'phone', phone: '+7 (999) 000-00-00' }),
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal: undefined,
    })
  })

  it('reads and deletes the canonical session resource', async () => {
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

  it('uses production in normal development and keeps synthetic preview opt-in functional', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(jsonResponse(redirectProgress))
    const normalDevelopment = createDefaultAuthApi({ dev: true, syntheticPreviewEnabled: false, fetcher })

    await expect(normalDevelopment.startAuth()).resolves.toEqual(redirectProgress)
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(normalDevelopment.isSyntheticPreview).toBe(false)

    const explicitPreview = createDefaultAuthApi({ dev: true, syntheticPreviewEnabled: true, fetcher })
    await expect(explicitPreview.startAuth()).resolves.toMatchObject({ status: 'challenge' })
    await expect(explicitPreview.importWebSsoResponse('uninspected-preview-json')).resolves.toMatchObject({ status: 'authenticated' })
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(explicitPreview.isSyntheticPreview).toBe(true)

    const production = createDefaultAuthApi({ dev: false, syntheticPreviewEnabled: true, fetcher })
    expect(production.isSyntheticPreview).toBe(false)
  })
})
