import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { AccessPoint } from './intercomApi'
import { DoorGrid } from './DoorGrid'

const pointWithTwoLocks: AccessPoint = {
  id: 'entrance-1',
  flatId: 'flat-81',
  name: 'Подъезд 1',
  kind: 'intercom',
  locks: [
    { number: '1', name: 'Основная дверь', permanentlyOpen: false },
    { number: '2', name: 'Вторая дверь', permanentlyOpen: false },
  ],
  cameraIds: [],
  available: true,
}

const gate: AccessPoint = {
  ...pointWithTwoLocks,
  id: 'gate-1',
  name: 'Калитка',
  kind: 'gate',
  locks: [{ number: '1', name: 'Замок калитки', permanentlyOpen: false }],
}

describe('DoorGrid', () => {
  it('prepares the selected lock from a direct door action without a selector', async () => {
    const user = userEvent.setup()
    const onPrepare = vi.fn()
    render(<DoorGrid accessPoints={[pointWithTwoLocks, gate]} interactionLocked={false} preparingLock={null} onPrepare={onPrepare} />)

    const secondLock = screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })
    expect(secondLock).toHaveTextContent('Открыть')
    expect(secondLock).toHaveTextContent('Вторая дверь')
    expect(secondLock).not.toHaveTextContent('Подъезд 1')
    expect(screen.getAllByText('Подъезд 1')).toHaveLength(1)
    expect(screen.getByRole('button', { name: 'Открыть Калитка' })).toHaveTextContent(/^Открыть$/)
    expect(screen.getAllByText('Калитка')).toHaveLength(1)

    await user.click(secondLock)

    expect(onPrepare).toHaveBeenCalledWith(pointWithTwoLocks, pointWithTwoLocks.locks[1])
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
  })

  it('blocks and shows feedback beside only the affected lock', () => {
    const onPrepare = vi.fn()
    const { rerender } = render(<DoorGrid accessPoints={[pointWithTwoLocks]} interactionLocked={false} preparingLock={null} onPrepare={onPrepare} />)

    rerender(<DoorGrid
      accessPoints={[pointWithTwoLocks]}
      interactionLocked={false}
      preparingLock={null}
      blockedLock={{ accessPointId: 'entrance-1', lockNumber: '2' }}
      feedback={{ accessPointId: 'entrance-1', lockNumber: '2', kind: 'error', message: 'Результат открытия неизвестен' }}
      onPrepare={onPrepare}
    />)

    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Вторая дверь' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Открыть Подъезд 1: Основная дверь' })).toBeEnabled()
    expect(screen.getByRole('alert')).toHaveTextContent('Результат открытия неизвестен')
  })

  it('shows an unavailable access point as a disabled door action', () => {
    render(<DoorGrid accessPoints={[{ ...pointWithTwoLocks, available: false }]} interactionLocked={false} preparingLock={null} onPrepare={vi.fn()} />)

    expect(screen.getByRole('button', { name: 'Открытие Подъезд 1 недоступно' })).toBeDisabled()
  })
})
