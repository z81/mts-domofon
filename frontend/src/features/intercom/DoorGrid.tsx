import type { AccessPoint, Lock } from './intercomApi'

export interface PreparingLock {
  accessPointId: string
  lockNumber: string
}

export interface DoorFeedback extends PreparingLock {
  kind: 'success' | 'error'
  message: string
}

export interface DoorGridProps {
  accessPoints: readonly AccessPoint[]
  interactionLocked: boolean
  preparingLock: PreparingLock | null
  blockedLock?: PreparingLock | null
  feedback?: DoorFeedback | null
  onPrepare(point: AccessPoint, lock: Lock): void
}

export function DoorGrid({ accessPoints, interactionLocked, preparingLock, blockedLock, feedback, onPrepare }: DoorGridProps) {
  if (accessPoints.length === 0) return <p className="empty-copy">Дверей для этого объекта нет</p>
  return <div className="door-grid">{accessPoints.flatMap((point) => {
    if (!point.available || point.locks.length === 0) return [<button
      aria-label={`Открытие ${point.name} недоступно`}
      className="door-card"
      disabled
      type="button"
      key={`${point.id}:unavailable`}
    ><span>{point.name}</span><b>Недоступно</b></button>]
    return point.locks.map((lock) => {
      const lockName = lock.name.trim() || `Замок ${lock.number}`
      const label = point.locks.length === 1 ? `Открыть ${point.name}` : `Открыть ${point.name}: ${lockName}`
      const preparing = preparingLock?.accessPointId === point.id && preparingLock.lockNumber === lock.number
      const blocked = blockedLock?.accessPointId === point.id && blockedLock.lockNumber === lock.number
      const ownFeedback = feedback?.accessPointId === point.id && feedback.lockNumber === lock.number ? feedback : null
      return <div className="door-action" key={`${point.id}:${lock.number}`}>
        <button aria-label={label} className="door-card" disabled={interactionLocked || blocked} onClick={() => onPrepare(point, lock)} type="button">
          <span>{point.name}</span><b>{preparing ? 'Подготовка…' : point.locks.length === 1 ? 'Открыть' : lockName}</b>
        </button>
        {ownFeedback && <p role={ownFeedback.kind === 'error' ? 'alert' : 'status'}>{ownFeedback.message}</p>}
      </div>
    })
  })}</div>
}
