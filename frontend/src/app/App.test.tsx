import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import App from './App'

describe('core intercom shell', () => {
  it('shows the core dashboard without excluded product sections', () => {
    render(<App />)

    expect(screen.getByRole('heading', { name: 'Домофон' })).toBeVisible()
    expect(screen.getAllByText('Камера у подъезда')[0]).toBeVisible()
    expect(screen.getByText('Последние события')).toBeVisible()
    expect(screen.queryByText('Поддержка')).not.toBeInTheDocument()
    expect(screen.queryByText('Документы')).not.toBeInTheDocument()
    expect(screen.queryByText('Избранное')).not.toBeInTheDocument()
  })

  it('asks for confirmation before a door can be opened', async () => {
    const user = userEvent.setup()
    render(<App />)

    await user.click(screen.getByRole('button', { name: 'Открыть Подъезд' }))

    expect(screen.getByRole('dialog', { name: 'Открыть Подъезд?' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Подтвердить открытие' })).toBeVisible()
  })

  it('opens the synthetic MTS ID SMS challenge without collecting a real secret', async () => {
    const user = userEvent.setup()
    render(<App />)

    await user.click(screen.getByRole('button', { name: 'Профиль подключения' }))
    await user.click(screen.getByRole('button', { name: 'Войти по номеру телефона' }))
    await user.type(screen.getByRole('textbox', { name: 'Номер телефона' }), '+7 999 000-00-00')
    await user.click(screen.getByRole('button', { name: 'Получить код' }))

    expect(screen.getByRole('heading', { name: 'Подтверждение номера' })).toBeVisible()
    expect(screen.getByText('Демо-режим: SMS не отправляется')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Подтвердить код' })).toBeDisabled()
  })

  it('switches to camera archive and presents a synthetic clip download', async () => {
    const user = userEvent.setup()
    render(<App />)

    await user.click(within(screen.getByRole('complementary', { name: 'Основная навигация' })).getByRole('button', { name: 'Камеры' }))
    await user.click(screen.getByRole('button', { name: 'Архив за сегодня' }))
    await user.click(screen.getByRole('button', { name: 'Скачать фрагмент' }))

    expect(screen.getByRole('heading', { name: 'Архив камеры' })).toBeVisible()
    expect(screen.getByText('Фрагмент подготовлен только для демонстрации')).toBeVisible()
  })

  it('requires confirmation before opening a door during an active call', async () => {
    const user = userEvent.setup()
    render(<App />)

    await user.click(within(screen.getByRole('complementary', { name: 'Основная навигация' })).getByRole('button', { name: 'Звонки' }))
    await user.click(screen.getByRole('button', { name: 'Принять вызов' }))
    await user.click(screen.getByRole('button', { name: 'Открыть дверь во время звонка' }))

    expect(screen.getByRole('dialog', { name: 'Открыть дверь во время звонка?' })).toBeVisible()
  })

  it('asks to confirm blocking a caller and exposes browser notification request', async () => {
    const user = userEvent.setup()
    render(<App />)

    await user.click(screen.getByRole('button', { name: 'Настройки звонков' }))
    await user.click(screen.getByRole('button', { name: 'Запросить уведомления браузера' }))
    await user.click(screen.getByRole('button', { name: 'Заблокировать номер +7 999 111-22-33' }))

    expect(screen.getByText('Запрос уведомлений доступен только в браузере')).toBeVisible()
    expect(screen.getByRole('dialog', { name: 'Заблокировать номер?' })).toBeVisible()
  })
})
