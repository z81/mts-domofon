import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Modal } from './Modal'

afterEach(() => {
  document.querySelector('[data-modal-return-focus]')?.remove()
})

describe('modal focus containment', () => {
  it('focuses its section fallback and keeps Tab there without enabled descendants', () => {
    const returnFocus = document.createElement('button')
    returnFocus.dataset.modalReturnFocus = 'true'
    document.body.append(returnFocus)
    returnFocus.focus()
    const view = render(<Modal labelledBy="empty-dialog-title" onClose={() => undefined}>
      <h2 id="empty-dialog-title">Пустой диалог</h2>
      <button disabled type="button">Недоступно</button>
    </Modal>)
    const dialog = screen.getByRole('dialog', { name: 'Пустой диалог' })

    expect(dialog).toHaveAttribute('tabindex', '-1')
    expect(dialog).toHaveFocus()
    returnFocus.focus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(dialog).toHaveFocus()
    returnFocus.focus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(dialog).toHaveFocus()

    view.unmount()
    expect(returnFocus).toHaveFocus()
  })

  it('recaptures forward and backward Tab from outside to its enabled controls', () => {
    const outside = document.createElement('button')
    outside.dataset.modalReturnFocus = 'true'
    document.body.append(outside)
    outside.focus()
    render(<Modal labelledBy="controlled-dialog-title" onClose={() => undefined}>
      <h2 id="controlled-dialog-title">Диалог с кнопками</h2>
      <button type="button">Первая</button>
      <button type="button">Последняя</button>
    </Modal>)
    const first = screen.getByRole('button', { name: 'Первая' })
    const last = screen.getByRole('button', { name: 'Последняя' })

    outside.focus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(first).toHaveFocus()
    outside.focus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(last).toHaveFocus()
  })

  it('lets only the dynamically nested topmost dialog handle Tab and Escape', async () => {
    const user = userEvent.setup()
    const closeOuter = vi.fn()

    function NestedDialogs() {
      const [innerOpen, setInnerOpen] = useState(false)
      return <Modal dismissible={false} labelledBy="outer-dialog-title" onClose={closeOuter}>
        <h2 id="outer-dialog-title">Внешний диалог</h2>
        <button onClick={() => setInnerOpen(true)} type="button">Открыть внутренний</button>
        {innerOpen && <Modal labelledBy="inner-dialog-title" onClose={() => setInnerOpen(false)}>
          <h2 id="inner-dialog-title">Внутренний диалог</h2>
          <button data-dialog-initial type="button">Первая внутренняя</button>
          <button type="button">Последняя внутренняя</button>
        </Modal>}
      </Modal>
    }

    render(<NestedDialogs />)
    const openInner = screen.getByRole('button', { name: 'Открыть внутренний' })
    await user.click(openInner)
    const inner = screen.getByRole('dialog', { name: 'Внутренний диалог' })
    const first = screen.getByRole('button', { name: 'Первая внутренняя' })
    const last = screen.getByRole('button', { name: 'Последняя внутренняя' })

    last.focus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(first).toHaveFocus()
    first.focus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(last).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(inner).not.toBeInTheDocument()
    expect(screen.getByRole('dialog', { name: 'Внешний диалог' })).toBeVisible()
    expect(openInner).toHaveFocus()
    expect(closeOuter).not.toHaveBeenCalled()
  })
})
