import { useEffect, useRef, type ReactNode } from 'react'

const focusableSelector = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])'

export function Modal({ children, labelledBy, onClose, className = '', closeOnBackdrop = false }: {
  children: ReactNode
  labelledBy: string
  onClose: () => void
  className?: string
  closeOnBackdrop?: boolean
}) {
  const dialogRef = useRef<HTMLElement>(null)
  const restoreFocusRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const dialog = dialogRef.current
    const initial = dialog?.querySelector<HTMLElement>('[data-dialog-initial]') ?? dialog?.querySelector<HTMLElement>(focusableSelector)
    initial?.focus()

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault()
        onClose()
        return
      }
      if (event.key !== 'Tab' || !dialog) return
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector))
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      restoreFocusRef.current?.focus()
    }
  }, [onClose])

  return <div className="dialog-backdrop" onMouseDown={(event) => closeOnBackdrop && event.currentTarget === event.target && onClose()}>
    <section ref={dialogRef} className={`confirm-dialog ${className}`.trim()} role="dialog" aria-modal="true" aria-labelledby={labelledBy}>
      {children}
    </section>
  </div>
}
