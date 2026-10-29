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

export function DoorIcon() {
  return <svg aria-hidden="true" fill="none" height="22" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.9" viewBox="0 0 24 24" width="22">
    <path d="M6 21V4.8C6 3.8 6.8 3 7.8 3h8.4c1 0 1.8.8 1.8 1.8V21M4 21h16M14.5 11.5v2" />
  </svg>
}

export function DoorGrid({ accessPoints, interactionLocked, preparingLock, blockedLock, feedback, onPrepare }: DoorGridProps) {
  if (accessPoints.length === 0) return <p className="empty-copy">Дверей по этому адресу нет</p>
  // Buttons name their lock, or their entrance when it has one lock. Once several entrances include
  // one with several locks, every entrance heads its own buttons instead, so each name shows once.
  const grouped = accessPoints.length > 1 && accessPoints.some((point) => point.available && point.locks.length > 1)
  return <div className="door-grid">{accessPoints.flatMap((point) => {
    const actions = !point.available || point.locks.length === 0
      ? [<button
          aria-label={`Открытие ${point.name} недоступно`}
          className="door-card"
          disabled
          type="button"
          key={`${point.id}:unavailable`}
        ><span className="door-icon"><DoorIcon /></span><span className="door-label"><b>Недоступно</b>{!grouped && <span>{point.name}</span>}</span></button>]
      : point.locks.map((lock) => {
          const lockName = lock.name.trim() || `Замок ${lock.number}`
          const label = point.locks.length === 1 ? `Открыть ${point.name}` : `Открыть ${point.name}: ${lockName}`
          const doorName = point.locks.length > 1 ? lockName : grouped ? null : point.name
          const preparing = preparingLock?.accessPointId === point.id && preparingLock.lockNumber === lock.number
          const blocked = blockedLock?.accessPointId === point.id && blockedLock.lockNumber === lock.number
          const ownFeedback = feedback?.accessPointId === point.id && feedback.lockNumber === lock.number ? feedback : null
          return <div className="door-action" key={`${point.id}:${lock.number}`}>
            <button aria-label={label} className="door-card" disabled={interactionLocked || blocked} onClick={() => onPrepare(point, lock)} type="button">
              <span className="door-icon"><DoorIcon /></span>
              <span className="door-label"><b>{preparing ? 'Подготовка…' : 'Открыть'}</b>{doorName && <span>{doorName}</span>}</span>
            </button>
            {ownFeedback && <p role={ownFeedback.kind === 'error' ? 'alert' : 'status'}>{ownFeedback.message}</p>}
          </div>
        })
    if (!grouped) return actions
    return [<div className="door-group" key={point.id}>
      <p className="door-group-title">{point.name}</p>
      <div className="door-grid">{actions}</div>
    </div>]
  })}</div>
}
