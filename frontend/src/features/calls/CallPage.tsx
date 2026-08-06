import { useCallback, useEffect, useRef, useState } from 'react'
import { Modal } from '../../app/Modal'
import { callApi, type CallApi, type CallOpenIntent, type CurrentCall } from './callApi'

type Feedback = { kind: 'status' | 'error'; message: string }

interface PreparedCallOpen extends CallOpenIntent {
  callId: string
  revision: number
  callerName: string
}

export interface CallPageProps {
  api?: CallApi
  call: CurrentCall | null
  loading?: boolean
  pollingError?: boolean
  createPeerConnection?: () => RTCPeerConnection
  getUserMedia?: (constraints: MediaStreamConstraints) => Promise<MediaStream>
  iceGatheringTimeoutMs?: number
}

const DEFAULT_ICE_GATHERING_TIMEOUT_MS = 25_000

const phaseLabels: Record<CurrentCall['phase'], string> = {
  ringing: 'Входящий вызов',
  connecting: 'Подключаем звонок',
  active: 'Звонок идёт',
  ending: 'Завершаем звонок',
}

function defaultPeerConnection(): RTCPeerConnection {
  return new RTCPeerConnection()
}

function defaultGetUserMedia(constraints: MediaStreamConstraints): Promise<MediaStream> {
  if (!navigator.mediaDevices?.getUserMedia) {
    return Promise.reject(new Error('Browser media is unavailable'))
  }
  return navigator.mediaDevices.getUserMedia(constraints)
}

function formatCallTime(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'время не указано'
  return new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function formatIntentExpiry(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'в течение короткого времени'
  return new Intl.DateTimeFormat('ru-RU', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(date)
}

function abortError(): DOMException {
  return new DOMException('WebRTC operation aborted', 'AbortError')
}

async function waitForIceGatheringComplete(peer: RTCPeerConnection, signal: AbortSignal, timeoutMs: number): Promise<void> {
  if (peer.iceGatheringState === 'complete') return
  await new Promise<void>((resolve, reject) => {
    const cleanup = () => {
      peer.removeEventListener('icegatheringstatechange', onStateChange)
      signal.removeEventListener('abort', onAbort)
      window.clearTimeout(timeout)
    }
    const onStateChange = () => {
      if (peer.iceGatheringState !== 'complete') return
      cleanup()
      resolve()
    }
    const onAbort = () => {
      cleanup()
      reject(abortError())
    }
    const onTimeout = () => {
      cleanup()
      reject(new Error('ICE gathering timed out'))
    }
    peer.addEventListener('icegatheringstatechange', onStateChange)
    signal.addEventListener('abort', onAbort, { once: true })
    const timeout = window.setTimeout(onTimeout, Math.max(1, timeoutMs))
    if (signal.aborted) onAbort()
    else onStateChange()
  })
}

export function CallPage(props: CallPageProps) {
  return <CallPageSession key={props.call?.id ?? 'no-current-call'} {...props} />
}

function CallPageSession({
  api = callApi,
  call,
  loading = false,
  pollingError = false,
  createPeerConnection = defaultPeerConnection,
  getUserMedia = defaultGetUserMedia,
  iceGatheringTimeoutMs = DEFAULT_ICE_GATHERING_TIMEOUT_MS,
}: CallPageProps) {
  const [commandBusy, setCommandBusy] = useState<'answer' | 'decline' | 'ignore' | 'hangup' | null>(null)
  const [feedback, setFeedback] = useState<Feedback | null>(null)
  const [preparedOpen, setPreparedOpen] = useState<PreparedCallOpen | null>(null)
  const [openPreparing, setOpenPreparing] = useState(false)
  const [openExecuting, setOpenExecuting] = useState(false)
  const [openOutcomeUnknown, setOpenOutcomeUnknown] = useState(false)
  const [answerSubmittedRevision, setAnswerSubmittedRevision] = useState<number | null>(null)
  const commandInFlight = useRef(false)
  const openPreparationInFlight = useRef(false)
  const openExecutionInFlight = useRef(false)
  const peerRef = useRef<RTCPeerConnection | null>(null)
  const mediaAbortRef = useRef<AbortController | null>(null)
  const localStreamRef = useRef<MediaStream | null>(null)
  const remoteStreamRef = useRef<MediaStream | null>(null)
  const audioSenderRef = useRef<RTCRtpSender | null>(null)
  const videoRef = useRef<HTMLVideoElement>(null)

  const cleanupMedia = useCallback(() => {
    mediaAbortRef.current?.abort()
    mediaAbortRef.current = null
    const peer = peerRef.current
    peerRef.current = null
    peer?.close()
    const localStream = localStreamRef.current
    localStreamRef.current = null
    localStream?.getTracks().forEach((track) => track.stop())
    remoteStreamRef.current = null
    audioSenderRef.current = null
    if (videoRef.current) videoRef.current.srcObject = null
  }, [])

  useEffect(() => {
    return cleanupMedia
  }, [cleanupMedia])

  useEffect(() => {
    if (!call || call.phase === 'ending') cleanupMedia()
  }, [call, cleanupMedia])

  useEffect(() => {
    if (!preparedOpen || openExecuting) return
    const expiresAt = new Date(preparedOpen.expiresAt).getTime()
    if (Number.isNaN(expiresAt)) return
    const timer = window.setTimeout(() => {
      setPreparedOpen(null)
      setFeedback({ kind: 'error', message: 'Подтверждение открытия истекло. Подготовьте его заново.' })
    }, Math.min(Math.max(0, expiresAt - Date.now()), 2_147_483_647))
    return () => window.clearTimeout(timer)
  }, [openExecuting, preparedOpen])

  async function answerCall() {
    const answerAwaitingPoll = call?.phase === 'ringing' && answerSubmittedRevision === call.revision
    if (!call || call.phase !== 'ringing' || answerAwaitingPoll || commandInFlight.current) return
    const selectedCall = call
    commandInFlight.current = true
    setCommandBusy('answer')
    setFeedback(null)
    cleanupMedia()
    const mediaAbort = new AbortController()
    mediaAbortRef.current = mediaAbort

    try {
      const peer = createPeerConnection()
      peerRef.current = peer
      peer.ontrack = (event) => {
        if (peerRef.current !== peer || !videoRef.current) return
        const signaledStream = event.streams[0]
        const stream = signaledStream ?? remoteStreamRef.current ?? new MediaStream()
        remoteStreamRef.current = stream
        if (!signaledStream && event.track && !stream.getTracks().includes(event.track)) stream.addTrack(event.track)
        videoRef.current.srcObject = stream
      }
      await peer.setRemoteDescription({ type: 'offer', sdp: selectedCall.remoteSdp })
      if (mediaAbort.signal.aborted || peerRef.current !== peer) return
      const localStream = await getUserMedia({ audio: true, video: false })
      if (mediaAbort.signal.aborted || peerRef.current !== peer) {
        localStream.getTracks().forEach((track) => track.stop())
        return
      }
      localStreamRef.current = localStream
      for (const track of localStream.getAudioTracks()) {
        const sender = peer.addTrack(track, localStream)
        if (!audioSenderRef.current) audioSenderRef.current = sender
      }
      const answer = await peer.createAnswer()
      await peer.setLocalDescription(answer)
      await waitForIceGatheringComplete(peer, mediaAbort.signal, iceGatheringTimeoutMs)
      if (mediaAbort.signal.aborted || peerRef.current !== peer) return
      const localSdp = peer.localDescription?.sdp
      if (!localSdp) throw new Error('Browser did not produce local SDP')
      await api.answer(selectedCall.id, selectedCall.revision, localSdp)
      setAnswerSubmittedRevision(selectedCall.revision)
      setFeedback({ kind: 'status', message: 'Ответ отправлен. Подключаем аудио и видео…' })
    } catch (error) {
      if (!(error instanceof DOMException && error.name === 'AbortError')) {
        cleanupMedia()
        setFeedback({ kind: 'error', message: 'Не удалось принять вызов. Проверьте доступ к микрофону.' })
      }
    } finally {
      commandInFlight.current = false
      setCommandBusy(null)
    }
  }

  async function runCommand(action: 'decline' | 'ignore' | 'hangup') {
    if (!call || commandInFlight.current) return
    const selectedCall = call
    commandInFlight.current = true
    setCommandBusy(action)
    setFeedback(null)
    try {
      await api[action](selectedCall.id, selectedCall.revision)
      setFeedback({
        kind: 'status',
        message: action === 'hangup' ? 'Завершаем вызов…' : action === 'ignore' ? 'Вызов проигнорирован' : 'Вызов отклонён',
      })
    } catch {
      setFeedback({ kind: 'error', message: 'Команда не подтверждена локальным сервисом. Состояние звонка обновится автоматически.' })
    } finally {
      commandInFlight.current = false
      setCommandBusy(null)
    }
  }

  async function prepareOpen() {
    if (!call || !call.canOpen || call.phase === 'ending' || openOutcomeUnknown || openPreparationInFlight.current || openExecutionInFlight.current || preparedOpen) return
    const selectedCall = call
    openPreparationInFlight.current = true
    setOpenPreparing(true)
    setFeedback(null)
    try {
      const intent = await api.createOpenIntent(selectedCall.id, selectedCall.revision)
      setPreparedOpen({
        ...intent,
        callId: selectedCall.id,
        revision: selectedCall.revision,
        callerName: selectedCall.callerName,
      })
    } catch {
      setFeedback({ kind: 'error', message: 'Не удалось подготовить подтверждение открытия.' })
    } finally {
      openPreparationInFlight.current = false
      setOpenPreparing(false)
    }
  }

  const closeOpenConfirmation = useCallback(() => {
    if (openExecutionInFlight.current) return
    setPreparedOpen(null)
  }, [])

  async function executeOpen() {
    if (!preparedOpen || openExecutionInFlight.current) return
    const intent = preparedOpen
    openExecutionInFlight.current = true
    setOpenExecuting(true)
    setFeedback(null)
    try {
      try {
        audioSenderRef.current?.dtmf?.insertDTMF('1', 1_000, 500)
      } catch {
        // DTMF is best-effort browser compatibility; the confirmed REST action is still required.
      }
      await api.executeOpenIntent(intent.callId, intent.intentId, intent.revision)
      setPreparedOpen(null)
      setFeedback({ kind: 'status', message: 'Команда открытия отправлена' })
    } catch {
      setPreparedOpen(null)
      setOpenOutcomeUnknown(true)
      setFeedback({ kind: 'error', message: 'Результат открытия неизвестен. Не повторяйте команду автоматически — сначала проверьте дверь.' })
    } finally {
      openExecutionInFlight.current = false
      setOpenExecuting(false)
    }
  }

  if (loading && !call) {
    return <>
      <section className="page-heading"><div><p className="eyebrow">Звонки</p><h1>Звонки</h1></div></section>
      <section className="content-card call-empty" role="status"><span className="loading-ring" /><h2>Проверяем входящие звонки…</h2></section>
    </>
  }

  if (pollingError && !call) {
    return <>
      <section className="page-heading"><div><p className="eyebrow">Звонки</p><h1>Звонки</h1></div></section>
      <section className="content-card call-empty" role="alert"><span className="state-icon" aria-hidden="true">!</span><h2>Не удалось проверить звонки</h2><p>Локальный сервис продолжит попытки подключения автоматически.</p></section>
    </>
  }

  if (!call) {
    return <>
      <section className="page-heading"><div><p className="eyebrow">Звонки</p><h1>Звонки</h1></div></section>
      <section className="content-card call-empty"><span className="state-icon" aria-hidden="true">☎</span><h2>Нет активного звонка</h2><p>Здесь появится только реальный вызов, полученный локальным сервисом.</p></section>
    </>
  }

  const answerAwaitingPoll = call.phase === 'ringing' && answerSubmittedRevision === call.revision
  const controlsDisabled = commandBusy !== null || call.phase === 'ending' || answerAwaitingPoll
  const liveMessage = feedback?.message ?? phaseLabels[call.phase]
  const liveRole = feedback?.kind === 'error' ? 'alert' : 'status'

  return <>
    <section className="page-heading"><div><p className="eyebrow">Домофон</p><h1>Звонок</h1><p>Сигнал и медиа получены через локальный API.</p></div></section>
    <section className={`call-stage call-${call.phase}`} aria-labelledby="current-call-title">
      <p className="call-live-message" role={liveRole}>{liveMessage}</p>
      <video
        aria-label={`Видео звонка от ${call.callerName}`}
        autoPlay
        className="call-video"
        playsInline
        poster={call.previewUrl}
        ref={videoRef}
      />
      <div className="call-overlay">
        <span className="call-avatar" aria-hidden="true">☎</span>
        <h2 id="current-call-title">{call.callerName}</h2>
        <p><time dateTime={call.startedAt}>с {formatCallTime(call.startedAt)}</time></p>
        <div className="call-actions">
          {call.phase === 'ringing' && <>
            <button className="primary-button" disabled={controlsDisabled} onClick={() => void answerCall()} type="button">{commandBusy === 'answer' ? 'Подключаем…' : answerAwaitingPoll ? 'Ответ отправлен…' : 'Принять вызов'}</button>
            <button className="secondary-button danger-outline" disabled={controlsDisabled} onClick={() => void runCommand('decline')} type="button">Отклонить вызов</button>
            <button className="secondary-button" disabled={controlsDisabled} onClick={() => void runCommand('ignore')} type="button">Игнорировать вызов</button>
          </>}
          {(call.phase === 'connecting' || call.phase === 'active') && <button className="secondary-button danger-outline" disabled={controlsDisabled} onClick={() => void runCommand('hangup')} type="button">Завершить вызов</button>}
          {call.canOpen && <button
            className="secondary-button"
            disabled={controlsDisabled || openPreparing || preparedOpen !== null || openExecuting || openOutcomeUnknown}
            onClick={() => void prepareOpen()}
            type="button"
          >{openPreparing ? 'Готовим подтверждение…' : 'Открыть дверь'}</button>}
        </div>
      </div>
    </section>
    {preparedOpen && <Modal className="door-dialog call-open-dialog" labelledBy="call-open-title" onClose={closeOpenConfirmation} closeOnBackdrop>
      <button aria-label="Закрыть" className="dialog-close" disabled={openExecuting} onClick={closeOpenConfirmation} type="button">×</button>
      <span className="dialog-icon" aria-hidden="true">⌑</span>
      <h2 id="call-open-title">Открыть дверь во время звонка?</h2>
      <p>Следующее нажатие отправит физическую команду для текущего звонка.</p>
      <dl className="door-summary">
        <div><dt>Домофон</dt><dd>{preparedOpen.callerName}</dd></div>
        <div><dt>Звонок</dt><dd>Текущий вызов, ревизия {preparedOpen.revision}</dd></div>
      </dl>
      <p className="door-expiry">Действует до {formatIntentExpiry(preparedOpen.expiresAt)}</p>
      <div className="dialog-actions">
        <button className="secondary-button" data-dialog-initial disabled={openExecuting} onClick={closeOpenConfirmation} type="button">Отмена</button>
        <button className="primary-button danger" disabled={openExecuting} onClick={() => void executeOpen()} type="button">{openExecuting ? 'Открываем…' : 'Подтвердить открытие'}</button>
      </div>
    </Modal>}
  </>
}
