import { Modal } from '../../app/Modal'
import { CallPage, type CallPageProps } from './CallPage'
import type { AutoOpenResult } from './callApi'

const ignoreDismiss = () => undefined

const autoOpenResultMessages = {
  opened: 'Дверь открыта автоматически',
  failed: 'Автооткрытие не сработало. Автоматического повтора не будет.',
  outcome_unknown: 'Результат автооткрытия неизвестен. Автоматического повтора не будет.',
} as const

interface IncomingCallOverlayProps extends Pick<CallPageProps, 'api' | 'call' | 'loading' | 'pollingError'> {
  autoOpenResult?: AutoOpenResult
}

export function IncomingCallOverlay({ autoOpenResult, ...callPageProps }: IncomingCallOverlayProps) {
  if (!callPageProps.call) return null
  return <Modal labelledBy="incoming-call-title" onClose={ignoreDismiss} dismissible={false} className="incoming-call-dialog">
    <h1 id="incoming-call-title" className="visually-hidden">Входящий вызов</h1>
    {autoOpenResult && <p
      className={`incoming-call-auto-open-result result-${autoOpenResult.status}`}
      role={autoOpenResult.status === 'opened' ? 'status' : 'alert'}
    >{autoOpenResultMessages[autoOpenResult.status]}</p>}
    <CallPage {...callPageProps} presentation="embedded" />
  </Modal>
}
