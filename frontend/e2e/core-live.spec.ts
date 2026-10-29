import { expect, test } from '@playwright/test'

test('renders real local API data and confirms a door command', async ({ page }) => {
  let executeCalls = 0
  await page.route('**/api/v1/dashboard', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      flats: [{
        id: 'flat-81',
        label: 'Квартира 81',
        address: 'Москва, ул. Примерная, 7',
        capabilities: {
          accessPoints: { state: 'available' },
          cameras: { state: 'available' },
          archive: { state: 'available' },
          events: { state: 'unavailable' },
          calls: { state: 'unavailable' },
          callSettings: { state: 'unavailable' },
        },
      }],
      selectedFlatId: 'flat-81',
      callSettings: { state: 'unavailable' },
    },
  }))
  await page.route('**/api/v1/devices?flatId=flat-81', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      accessPoints: [{
        id: 'entrance-1',
        flatId: 'flat-81',
        name: 'Подъезд 1',
        kind: 'intercom',
        locks: [{ number: '1', name: 'Дверь', permanentlyOpen: false }],
        cameraIds: ['camera-1'],
        available: true,
      }],
      cameras: [{
        id: 'camera-1',
        flatId: 'flat-81',
        name: 'Камера у подъезда',
        accessPointId: 'entrance-1',
        capabilities: {
          snapshot: { state: 'available' },
          live: { state: 'unavailable' },
          archive: { state: 'available' },
        },
      }],
    },
  }))
  await page.route('**/api/v1/cameras/camera-1/snapshot?flatId=flat-81', async (route) => route.fulfill({
    body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64'),
    contentType: 'image/png',
  }))
  await page.route('**/api/v1/cameras/camera-1/archive/distribution?flatId=flat-81', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { intervals: [] },
  }))
  await page.route('**/api/v1/access/intents', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { intentId: 'intent-1', expiresAt: new Date(Date.now() + 60_000).toISOString() },
    status: 201,
  }))
  await page.route('**/api/v1/access/intents/intent-1/execute', async (route) => {
    executeCalls += 1
    await route.fulfill({ body: '', status: 204 })
  })

  await page.goto('/')

  await expect(page.getByRole('heading', { name: 'Главная' })).toBeVisible()
  const navigation = page.getByRole('navigation', { name: 'Основная навигация' })
  await expect(navigation).toBeVisible()
  await expect(navigation.getByRole('button', { name: 'Главная' })).toBeVisible()
  await expect(navigation.getByRole('button', { name: 'События' })).toBeVisible()
  await expect(navigation.getByRole('button', { name: 'Ещё' })).toBeVisible()
  const destinationButtons = navigation.locator('button')
  await expect(destinationButtons).toHaveCount(3)
  await expect(destinationButtons).toHaveText(['Главная', 'События', 'Ещё'])
  await expect(page.getByText('Подъезд 1')).toBeVisible()
  await expect(page.getByText('Квартира', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: /Квартира/ })).toHaveCount(0)
  await expect(page.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()
  await page.getByRole('button', { name: 'Открыть камеру Камера у подъезда' }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Камера у подъезда' })).toBeFocused()
  await expect(page.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()
  await expect(page.getByRole('main').getByRole('button', { name: 'Открыть Подъезд 1' })).toBeVisible()
  await page.getByRole('button', { name: 'Архив', exact: true }).click()
  await expect(page.getByLabel('Архив камеры Камера у подъезда')).toHaveCSS('aspect-ratio', '16 / 9')
  await page.goBack()
  await expect(page.getByRole('heading', { level: 1, name: 'Главная' })).toBeFocused()
  for (const destination of ['Главная', 'События', 'Ещё']) {
    await expect(navigation.getByRole('button', { name: destination, exact: true })).toHaveCSS('min-height', '56px')
  }
  for (const removedDestination of ['Доступ', 'Камеры', 'Звонки']) {
    await expect(navigation.getByRole('button', { name: removedDestination, exact: true, includeHidden: true })).toHaveCount(0)
  }
  const openDoor = page.getByRole('button', { name: 'Открыть Подъезд 1' })
  await expect(openDoor).toHaveCSS('min-height', '56px')
  await openDoor.click()
  const dialog = page.getByRole('dialog', { name: 'Открыть дверь?' })
  await expect(dialog).toBeVisible()
  expect(executeCalls).toBe(0)
  await dialog.getByRole('button', { name: 'Подтвердить открытие' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Открыто: Подъезд 1' })).toContainText('Открыто: Подъезд 1')
  expect(executeCalls).toBe(1)
  await expect(page.locator('body')).not.toContainText(/preview|демо|синтетическ/i)
})

test('keeps secondary media and physical actions at least 44px tall', async ({ page }) => {
  await page.route('**/api/v1/session', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { authenticated: true, status: 'active', authenticated_at: '2026-08-07T12:00:00Z', expires_at: null },
  }))
  await page.route('**/api/v1/dashboard', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      flats: [{
        id: 'flat-81',
        label: 'Квартира 81',
        address: 'Москва, ул. Примерная, 7',
        capabilities: {
          accessPoints: { state: 'available' }, cameras: { state: 'available' }, archive: { state: 'available' },
          events: { state: 'available' }, calls: { state: 'available' }, callSettings: { state: 'available' },
        },
      }],
      selectedFlatId: 'flat-81',
      callSettings: { state: 'available' },
    },
  }))
  await page.route('**/api/v1/devices?flatId=flat-81', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      accessPoints: [],
      cameras: [{
        id: 'camera-1', flatId: 'flat-81', name: 'Камера у подъезда', accessPointId: 'entrance-1',
        capabilities: { snapshot: { state: 'available' }, live: { state: 'available' }, archive: { state: 'available' } },
      }],
    },
  }))
  await page.route('**/api/v1/cameras/camera-1/snapshot?flatId=flat-81', async (route) => route.fulfill({
    body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64'),
    contentType: 'image/png',
  }))
  await page.route('**/api/v1/cameras/camera-1/live?flatId=flat-81', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { sessionId: 'mock-live-session', playlistUrl: '/api/v1/live/mock-live-session/index.m3u8' },
  }))
  await page.route('**/api/v1/live/mock-live-session/index.m3u8', async (route) => route.fulfill({
    body: '#EXTM3U\n#EXT-X-ENDLIST\n',
    contentType: 'application/vnd.apple.mpegurl',
  }))
  await page.route('**/api/v1/calls/current', async (route) => route.fulfill({ body: '', status: 204 }))
  await page.route('**/api/v1/calls/auto-open', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { mode: 'next_call', dismissAfterOpen: false },
  }))
  await page.route('**/api/v1/events?**', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      page: 0,
      hasMore: false,
      items: [{
        id: 'camera-event-1', occurredAt: '2026-08-07T11:55:00Z', kind: 'camera', title: 'Движение у двери',
        cameraId: 'camera-1', archiveAt: '2026-08-07T11:55:00Z',
      }],
    },
  }))

  const computedMinHeight = async (selector: ReturnType<typeof page.getByRole>) => {
    await expect(selector).toBeVisible()
    return selector.evaluate((element) => Number.parseFloat(getComputedStyle(element).minHeight))
  }

  await page.goto('/')

  const autoOpenDisable = page.getByRole('status', { name: 'Автооткрытие включено' }).getByRole('button', { name: 'Выключить' })
  expect.soft(await computedMinHeight(autoOpenDisable), 'Home auto-open disable').toBeGreaterThanOrEqual(44)
  expect.soft(await computedMinHeight(page.getByRole('button', { name: 'Включить звук' })), 'live tile sound').toBeGreaterThanOrEqual(44)
  expect.soft(await computedMinHeight(page.getByRole('button', { name: 'Аккаунт' })), 'account').toBeGreaterThanOrEqual(44)

  await page.getByRole('button', { name: 'Открыть камеру Камера у подъезда' }).click()
  await expect(page.getByRole('button', { name: 'Эфир', exact: true })).toHaveAttribute('aria-pressed', 'true')
  expect.soft(await computedMinHeight(page.getByRole('button', { name: 'На весь экран' })), 'live fullscreen').toBeGreaterThanOrEqual(44)
  expect.soft(await computedMinHeight(page.getByRole('button', { name: 'Назад' })), 'camera back').toBeGreaterThanOrEqual(44)
  await page.getByRole('navigation', { name: 'Основная навигация' }).getByRole('button', { name: 'События' }).click()
  expect(await page.evaluate(() => window.history.state)).toBeNull()
  expect.soft(await computedMinHeight(page.getByRole('button', { name: 'Открыть запись камеры' })), 'archive event action').toBeGreaterThanOrEqual(44)
})

test('keeps state and incoming-call headings within the product type scale', async ({ page }) => {
  await page.route('**/api/v1/session', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      authenticated: true,
      status: 'active',
      authenticated_at: '2026-08-07T12:00:00Z',
      expires_at: null,
    },
  }))
  await page.route('**/api/v1/dashboard', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { flats: [], selectedFlatId: null, callSettings: { state: 'unavailable' } },
  }))
  await page.route('**/api/v1/calls/current', async (route) => route.fulfill({
    contentType: 'application/json',
    json: {
      id: 'call-81',
      revision: 3,
      phase: 'ringing',
      callerName: 'Домофон у подъезда',
      startedAt: '2026-08-07T12:00:00Z',
      remoteSdp: 'v=0\r\n',
      canOpen: false,
    },
  }))
  await page.route('**/api/v1/calls/auto-open', async (route) => route.fulfill({
    contentType: 'application/json',
    json: { mode: 'disabled', dismissAfterOpen: false },
  }))

  await page.goto('/')

  const stateHeading = page.getByRole('heading', { name: 'Квартир не найдено' })
  const callHeading = page.getByRole('heading', { name: 'Домофон у подъезда' })
  await expect(stateHeading).toBeAttached()
  await expect(callHeading).toBeVisible()
  const stateFontSize = await stateHeading.evaluate((heading) => Number.parseFloat(getComputedStyle(heading).fontSize))
  const callFontSize = await callHeading.evaluate((heading) => Number.parseFloat(getComputedStyle(heading).fontSize))
  expect.soft(stateFontSize).toBeGreaterThanOrEqual(20)
  expect.soft(stateFontSize).toBeLessThanOrEqual(28)
  expect.soft(callFontSize).toBeGreaterThanOrEqual(20)
  expect.soft(callFontSize).toBeLessThanOrEqual(28)
})
