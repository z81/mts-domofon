import type { IntercomApi, LiveSession } from './intercomApi'

export interface LiveSessionLease {
  session: Promise<LiveSession>
  invalidate(sessionId?: string): void
  release(): void
}

interface LiveSessionEntry {
  refs: number
  promise: Promise<LiveSession>
  session: LiveSession | null
  forgetTimer: number | undefined
}

// Live sessions are shared by every viewer of a camera, including other devices, so the UI never DELETEs one.
// Released sessions are only forgotten locally; the server expires them on its own.
const startTimeoutMs = 50_000
const forgetDelayMs = 45_000
const registry = new WeakMap<IntercomApi, Map<string, LiveSessionEntry>>()

function startSession(api: IntercomApi, sessions: Map<string, LiveSessionEntry>, key: string, flatId: string, cameraId: string): LiveSessionEntry {
  // The start request is never aborted by an unmount: the server would keep a session nobody knows the id of.
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), startTimeoutMs)
  const promise = api.startLive
    ? api.startLive(flatId, cameraId, controller.signal)
    : Promise.reject(new Error('Live video is not supported'))
  const entry: LiveSessionEntry = { refs: 0, promise, session: null, forgetTimer: undefined }
  promise.then((session) => {
    entry.session = session
  }, () => {
    if (sessions.get(key) === entry) sessions.delete(key)
  }).finally(() => window.clearTimeout(timeout))
  return entry
}

export function acquireLiveSession(api: IntercomApi, flatId: string, cameraId: string): LiveSessionLease {
  let sessions = registry.get(api)
  if (!sessions) {
    sessions = new Map()
    registry.set(api, sessions)
  }
  const key = `${flatId}\u0000${cameraId}`
  let entry = sessions.get(key)
  if (!entry) {
    entry = startSession(api, sessions, key, flatId, cameraId)
    sessions.set(key, entry)
  }
  window.clearTimeout(entry.forgetTimer)
  entry.forgetTimer = undefined
  entry.refs += 1

  const owner = sessions
  const current = entry
  let released = false
  return {
    session: current.promise,
    invalidate(sessionId) {
      if (owner.get(key) !== current) return
      if (sessionId !== undefined && current.session?.sessionId !== sessionId) return
      window.clearTimeout(current.forgetTimer)
      owner.delete(key)
    },
    release() {
      if (released) return
      released = true
      current.refs -= 1
      if (current.refs > 0 || owner.get(key) !== current) return
      current.forgetTimer = window.setTimeout(() => {
        if (owner.get(key) === current && current.refs === 0) owner.delete(key)
      }, forgetDelayMs)
    },
  }
}
