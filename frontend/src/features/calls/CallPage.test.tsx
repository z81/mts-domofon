import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { CallPage } from './CallPage'
import type { CallApi, CurrentCall } from './callApi'

const ringingCall: CurrentCall = {
  id: 'call / 81',
  revision: 4,
  phase: 'ringing',
  callerName: 'Подъезд 1',
  startedAt: '2026-08-06T12:00:00Z',
  remoteSdp: 'v=0\r\na=ice-ufrag:remote\r\n',
  previewUrl: '/api/v1/calls/call%2081/preview',
  canOpen: true,
}

function createApi(overrides: Partial<CallApi> = {}): CallApi {
  return {
    answer: async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }),
    createOpenIntent: async () => ({ intentId: 'intent / 1', expiresAt: new Date(Date.now() + 60_000).toISOString() }),
    decline: async () => undefined,
    executeOpenIntent: async () => undefined,
    getCurrent: async () => null,
    hangup: async () => undefined,
    ignore: async () => undefined,
    ...overrides,
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve
    reject = promiseReject
  })
  return { promise, reject, resolve }
}

class FakePeerConnection extends EventTarget {
  readonly sender = { dtmf: { insertDTMF: vi.fn() } }
  readonly addTrack = vi.fn(() => this.sender)
  readonly close = vi.fn()
  readonly createAnswer = vi.fn(async () => ({ type: 'answer', sdp: 'v=0\r\na=answer:initial\r\n' } as RTCSessionDescriptionInit))
  readonly setRemoteDescription = vi.fn(async (description: RTCSessionDescriptionInit) => {
    this.remoteDescription = description
  })
  readonly setLocalDescription = vi.fn(async (description: RTCSessionDescriptionInit) => {
    this.localDescription = this.iceGatheringState === 'complete'
      ? { type: 'answer', sdp: 'v=0\r\na=candidate:complete\r\n' }
      : description
  })
  iceGatheringState: RTCIceGatheringState = 'complete'
  localDescription: RTCSessionDescriptionInit | null = {
    type: 'answer',
    sdp: 'v=0\r\na=candidate:complete\r\n',
  }
  remoteDescription: RTCSessionDescriptionInit | null = null
  ontrack: ((event: RTCTrackEvent) => void) | null = null

  completeIce(sdp = 'v=0\r\na=candidate:complete\r\n') {
    this.localDescription = { type: 'answer', sdp }
    this.iceGatheringState = 'complete'
    this.dispatchEvent(new Event('icegatheringstatechange'))
  }

  emitRemoteStream(stream: MediaStream) {
    this.ontrack?.({ streams: [stream] } as unknown as RTCTrackEvent)
  }

  emitRemoteTrackWithoutStream(track: MediaStreamTrack) {
    this.ontrack?.({ streams: [], track } as unknown as RTCTrackEvent)
  }
}

function mediaFixture() {
  const localTrack = { kind: 'audio', stop: vi.fn() } as unknown as MediaStreamTrack
  const localStream = {
    getAudioTracks: () => [localTrack],
    getTracks: () => [localTrack],
  } as unknown as MediaStream
  const getUserMedia = vi.fn(async () => localStream)
  return { getUserMedia, localStream, localTrack }
}

describe('call page states and controls', () => {
  it('renders an honest empty state without invented caller controls', () => {
    render(<CallPage api={createApi()} call={null} />)

    expect(screen.getByRole('heading', { name: 'Нет активного звонка' })).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Принять вызов' })).not.toBeInTheDocument()
    expect(screen.queryByText('Подъезд 1')).not.toBeInTheDocument()
  })

  it('shows the real ringing caller and accessible incoming-call actions', () => {
    render(<CallPage api={createApi()} call={ringingCall} />)

    expect(screen.getByRole('status')).toHaveTextContent('Входящий вызов')
    expect(screen.getByRole('heading', { name: 'Подъезд 1' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Принять вызов' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Отклонить вызов' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Игнорировать вызов' })).toBeEnabled()
    expect(screen.getByLabelText('Видео звонка от Подъезд 1')).toHaveAttribute('poster', ringingCall.previewUrl)
    expect(document.body).not.toHaveTextContent('ice-ufrag')
  })

  it('dispatches only one decline while the command is in flight', async () => {
    const user = userEvent.setup()
    const pending = deferred<void>()
    const decline = vi.fn<CallApi['decline']>(() => pending.promise)
    render(<CallPage api={createApi({ decline })} call={ringingCall} />)
    const button = screen.getByRole('button', { name: 'Отклонить вызов' })

    await user.dblClick(button)

    expect(decline).toHaveBeenCalledTimes(1)
    expect(decline).toHaveBeenCalledWith('call / 81', 4)
    expect(button).toBeDisabled()
    await act(async () => pending.resolve())
  })

  it('uses distinct ignore and active-call hangup commands', async () => {
    const user = userEvent.setup()
    const ignore = vi.fn<CallApi['ignore']>(async () => undefined)
    const hangup = vi.fn<CallApi['hangup']>(async () => undefined)
    const api = createApi({ hangup, ignore })
    const view = render(<CallPage api={api} call={ringingCall} />)

    await user.click(screen.getByRole('button', { name: 'Игнорировать вызов' }))
    expect(ignore).toHaveBeenCalledWith('call / 81', 4)

    view.rerender(<CallPage api={api} call={{ ...ringingCall, phase: 'active', revision: 7 }} />)
    await user.click(screen.getByRole('button', { name: 'Завершить вызов' }))
    expect(hangup).toHaveBeenCalledWith('call / 81', 7)
  })
})

describe('browser WebRTC answer', () => {
  it('adds only microphone audio and posts the candidate-complete local SDP', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const answer = vi.fn<CallApi['answer']>(async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }))
    render(<CallPage
      api={createApi({ answer })}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)

    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))

    await waitFor(() => expect(answer).toHaveBeenCalledWith('call / 81', 4, 'v=0\r\na=candidate:complete\r\n', expect.any(AbortSignal)))
    expect(peer.setRemoteDescription).toHaveBeenCalledWith({ type: 'offer', sdp: ringingCall.remoteSdp })
    expect(media.getUserMedia).toHaveBeenCalledWith({ audio: true, video: false })
    expect(peer.addTrack).toHaveBeenCalledWith(media.localTrack, media.localStream)
  })

  it('does not post the answer before non-trickle ICE gathering completes', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    peer.iceGatheringState = 'gathering'
    peer.localDescription = { type: 'answer', sdp: 'v=0\r\na=answer:without-candidates\r\n' }
    const media = mediaFixture()
    const answer = vi.fn<CallApi['answer']>(async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }))
    render(<CallPage
      api={createApi({ answer })}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)

    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    await waitFor(() => expect(peer.setLocalDescription).toHaveBeenCalled())
    expect(answer).not.toHaveBeenCalled()

    act(() => peer.completeIce('v=0\r\na=candidate:last\r\n'))
    await waitFor(() => expect(answer).toHaveBeenCalledWith('call / 81', 4, 'v=0\r\na=candidate:last\r\n', expect.any(AbortSignal)))
  })

  it('stops a broken ICE gather after the bounded wait without posting partial SDP', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    peer.iceGatheringState = 'gathering'
    peer.localDescription = { type: 'answer', sdp: 'v=0\r\na=answer:without-candidates\r\n' }
    const media = mediaFixture()
    const answer = vi.fn<CallApi['answer']>(async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }))
    render(<CallPage
      api={createApi({ answer })}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
      iceGatheringTimeoutMs={10}
    />)

    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Не удалось принять вызов')
    expect(answer).not.toHaveBeenCalled()
    expect(peer.close).toHaveBeenCalledTimes(1)
    expect(media.localTrack.stop).toHaveBeenCalledTimes(1)
  })

  it('keeps an accepted ringing snapshot submitted until polling advances it', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const createPeerConnection = vi.fn(() => peer as unknown as RTCPeerConnection)
    const answer = vi.fn<CallApi['answer']>(async () => ({ ...ringingCall, phase: 'connecting', revision: 5 }))
    const view = render(<CallPage
      api={createApi({ answer })}
      call={ringingCall}
      createPeerConnection={createPeerConnection}
      getUserMedia={media.getUserMedia}
    />)

    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    const submitted = await screen.findByRole('button', { name: 'Ответ отправлен…' })
    expect(submitted).toBeDisabled()
    await user.click(submitted)
    expect(answer).toHaveBeenCalledTimes(1)
    expect(createPeerConnection).toHaveBeenCalledTimes(1)
    expect(peer.close).not.toHaveBeenCalled()

    view.rerender(<CallPage
      api={createApi({ answer })}
      call={{ ...ringingCall, phase: 'connecting', revision: 5 }}
      createPeerConnection={createPeerConnection}
      getUserMedia={media.getUserMedia}
    />)
    expect(screen.getByRole('button', { name: 'Завершить вызов' })).toBeEnabled()
  })

  it('renders the received remote stream and closes media on call end', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const api = createApi()
    const view = render(<CallPage
      api={api}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    const remoteStream = { getTracks: () => [] } as unknown as MediaStream

    act(() => peer.emitRemoteStream(remoteStream))
    expect(screen.getByLabelText('Видео звонка от Подъезд 1')).toHaveProperty('srcObject', remoteStream)

    view.rerender(<CallPage
      api={api}
      call={{ ...ringingCall, phase: 'ending', revision: 5 }}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)
    expect(peer.close).toHaveBeenCalledTimes(1)
    expect(media.localTrack.stop).toHaveBeenCalledTimes(1)
    expect(screen.getByLabelText('Видео звонка от Подъезд 1')).toHaveProperty('srcObject', null)
  })

  it('builds and retains a remote stream when the browser track event has no streams', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const createdStreams: Array<{ addTrack: ReturnType<typeof vi.fn>; getTracks: () => MediaStreamTrack[] }> = []
    class StreamWithoutConstructorTracks {
      private tracks: MediaStreamTrack[] = []
      addTrack = vi.fn((track: MediaStreamTrack) => this.tracks.push(track))
      getTracks = () => this.tracks
      constructor() {
        createdStreams.push(this)
      }
    }
    vi.stubGlobal('MediaStream', StreamWithoutConstructorTracks)
    const view = render(<CallPage
      api={createApi()}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    const remoteTrack = { kind: 'video' } as MediaStreamTrack

    act(() => peer.emitRemoteTrackWithoutStream(remoteTrack))
    act(() => peer.emitRemoteTrackWithoutStream(remoteTrack))

    expect(createdStreams).toHaveLength(1)
    expect(createdStreams[0].addTrack).toHaveBeenCalledTimes(1)
    expect(createdStreams[0].addTrack).toHaveBeenCalledWith(remoteTrack)
    expect(screen.getByLabelText('Видео звонка от Подъезд 1')).toHaveProperty('srcObject', createdStreams[0])
    view.unmount()
    vi.unstubAllGlobals()
  })

  it('does not request microphone permission after unmount interrupts remote SDP setup', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const remoteDescription = deferred<void>()
    peer.setRemoteDescription.mockImplementationOnce(() => remoteDescription.promise)
    const media = mediaFixture()
    const view = render(<CallPage
      api={createApi()}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    expect(peer.setRemoteDescription).toHaveBeenCalledTimes(1)

    view.unmount()
    await act(async () => remoteDescription.resolve())

    expect(media.getUserMedia).not.toHaveBeenCalled()
    expect(peer.close).toHaveBeenCalledTimes(1)
  })

  it('aborts an in-flight answer request when the call session is torn down', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    let answerSignal: AbortSignal | undefined
    const answer = vi.fn<CallApi['answer']>((_callId, _revision, _sdp, signal) => {
      answerSignal = signal
      return new Promise((_resolve, reject) => {
        signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')), { once: true })
      })
    })
    const view = render(<CallPage
      api={createApi({ answer })}
      call={ringingCall}
      createPeerConnection={() => peer as unknown as RTCPeerConnection}
      getUserMedia={media.getUserMedia}
    />)

    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    await waitFor(() => expect(answer).toHaveBeenCalledTimes(1))
    view.unmount()

    expect(answerSignal?.aborted).toBe(true)
    expect(peer.close).toHaveBeenCalledTimes(1)
    expect(media.localTrack.stop).toHaveBeenCalledTimes(1)
  })
})

describe('confirmed call-scoped open', () => {
  function AnsweredHarness({ api, peer, getUserMedia }: {
    api: CallApi
    peer: FakePeerConnection
    getUserMedia: (constraints: MediaStreamConstraints) => Promise<MediaStream>
  }) {
    const [call, setCall] = useState(ringingCall)
    return <>
      <CallPage
        api={api}
        call={call}
        createPeerConnection={() => peer as unknown as RTCPeerConnection}
        getUserMedia={getUserMedia}
      />
      <button onClick={() => setCall({ ...call, phase: 'active', revision: 5 })} type="button">Server active</button>
    </>
  }

  it('requires confirmation, then sends DTMF immediately before one REST execute', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const createOpenIntent = vi.fn<CallApi['createOpenIntent']>(async () => ({
      intentId: 'intent / 1',
      expiresAt: new Date(Date.now() + 60_000).toISOString(),
    }))
    const executeOpenIntent = vi.fn<CallApi['executeOpenIntent']>(async () => undefined)
    const api = createApi({ createOpenIntent, executeOpenIntent })
    render(<AnsweredHarness api={api} peer={peer} getUserMedia={media.getUserMedia} />)
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    await user.click(screen.getByRole('button', { name: 'Server active' }))

    await user.click(screen.getByRole('button', { name: 'Открыть дверь' }))
    expect(createOpenIntent).toHaveBeenCalledWith('call / 81', 5)
    expect(executeOpenIntent).not.toHaveBeenCalled()
    const dialog = await screen.findByRole('dialog', { name: 'Открыть дверь во время звонка?' })
    expect(within(dialog).getByText('Подъезд 1')).toBeVisible()

    const confirm = within(dialog).getByRole('button', { name: 'Подтвердить открытие' })
    await user.dblClick(confirm)

    expect(peer.sender.dtmf.insertDTMF).toHaveBeenCalledTimes(1)
    expect(peer.sender.dtmf.insertDTMF).toHaveBeenCalledWith('1', 1_000, 500)
    expect(executeOpenIntent).toHaveBeenCalledTimes(1)
    expect(executeOpenIntent).toHaveBeenCalledWith('call / 81', 'intent / 1', 5)
    expect(peer.sender.dtmf.insertDTMF.mock.invocationCallOrder[0]).toBeLessThan(executeOpenIntent.mock.invocationCallOrder[0] ?? 0)
    expect(await screen.findByRole('status')).toHaveTextContent('Команда открытия отправлена')
  })

  it('shows an unknown-result warning and never retries a failed execute automatically', async () => {
    const user = userEvent.setup()
    const peer = new FakePeerConnection()
    const media = mediaFixture()
    const executeOpenIntent = vi.fn<CallApi['executeOpenIntent']>(async () => {
      throw new TypeError('network response lost')
    })
    const api = createApi({ executeOpenIntent })
    render(<AnsweredHarness api={api} peer={peer} getUserMedia={media.getUserMedia} />)
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    await user.click(screen.getByRole('button', { name: 'Server active' }))
    await user.click(screen.getByRole('button', { name: 'Открыть дверь' }))
    await user.click(within(await screen.findByRole('dialog', { name: 'Открыть дверь во время звонка?' })).getByRole('button', { name: 'Подтвердить открытие' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Результат открытия неизвестен. Не повторяйте команду автоматически')
    expect(executeOpenIntent).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('button', { name: 'Открыть дверь' })).toBeDisabled()
  })
})
