import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react'

const focusableSelector = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])'
const mountedDialogs: HTMLElement[] = []

function focusableElements(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector))
}

function focusInitialOrFallback(dialog: HTMLElement) {
  const initial = dialog.querySelector<HTMLElement>('[data-dialog-initial]:not(:disabled)')
    ?? focusableElements(dialog)[0]
    ?? dialog
  initial.focus()
}

function registerDialog(dialog: HTMLElement): () => void {
  mountedDialogs.push(dialog)
  mountedDialogs.sort((left, right) => {
    const position = left.compareDocumentPosition(right)
    if (position & Node.DOCUMENT_POSITION_FOLLOWING) return -1
    if (position & Node.DOCUMENT_POSITION_PRECEDING) return 1
    return 0
  })
  return () => {
    const index = mountedDialogs.indexOf(dialog)
    if (index >= 0) mountedDialogs.splice(index, 1)
  }
}

function isTopmostDialog(dialog: HTMLElement): boolean {
  return mountedDialogs[mountedDialogs.length - 1] === dialog
}

interface ModalProps {
  children: ReactNode
  labelledBy: string
  onClose: () => void
  className?: string
  closeOnBackdrop?: boolean
  dismissible?: boolean
}

export function Modal({ children, labelledBy, onClose, className = '', closeOnBackdrop = false, dismissible = true }: ModalProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const restoreFocusRef = useRef<HTMLElement | null>(null)

  useLayoutEffect(() => {
    const dialog = dialogRef.current
    if (!dialog || !isTopmostDialog(dialog) || dialog.contains(document.activeElement)) return
    focusInitialOrFallback(dialog)
  })

  useEffect(() => {
    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const dialog = dialogRef.current
    if (!dialog) return
    const mountedDialog: HTMLElement = dialog
    const unregisterDialog = registerDialog(mountedDialog)
    if (isTopmostDialog(mountedDialog)) focusInitialOrFallback(mountedDialog)

    function onKeyDown(event: KeyboardEvent) {
      if (!isTopmostDialog(mountedDialog)) return
      if (event.key === 'Escape') {
        if (!dismissible) return
        event.preventDefault()
        onClose()
        return
      }
      if (event.key !== 'Tab') return
      const focusable = focusableElements(mountedDialog)
      if (focusable.length === 0) {
        event.preventDefault()
        mountedDialog.focus()
        return
      }
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      const active = document.activeElement
      if (!mountedDialog.contains(active) || !focusable.includes(active as HTMLElement)) {
        event.preventDefault()
        const boundary = event.shiftKey ? last : first
        boundary.focus()
      } else if (event.shiftKey && active === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && active === last) {
        event.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      const wasTopmost = isTopmostDialog(mountedDialog)
      unregisterDialog()
      if (wasTopmost) restoreFocusRef.current?.focus()
    }
  }, [dismissible, onClose])

  return <div className="dialog-backdrop" onMouseDown={(event) => {
    if (dismissible && closeOnBackdrop && event.currentTarget === event.target) onClose()
  }}>
    <section ref={dialogRef} className={`confirm-dialog ${className}`.trim()} role="dialog" aria-modal="true" aria-labelledby={labelledBy} tabIndex={-1}>
      {children}
    </section>
  </div>
}
