import { expect, test } from '@playwright/test'

test('renders real local API data without enabling unsupported commands', async ({ page }) => {
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
          archive: { state: 'unavailable' },
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
          archive: { state: 'unavailable' },
        },
      }],
    },
  }))
  await page.route('**/api/v1/cameras/camera-1/snapshot?flatId=flat-81', async (route) => route.fulfill({
    body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64'),
    contentType: 'image/png',
  }))

  await page.goto('/')

  await expect(page.getByRole('heading', { name: 'Домофон' })).toBeVisible()
  await expect(page.getByText('Подъезд 1')).toBeVisible()
  await expect(page.getByRole('img', { name: 'Кадр с камеры Камера у подъезда' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Открытие Подъезд 1 пока недоступно' })).toBeDisabled()
  await expect(page.locator('body')).not.toContainText(/preview|демо|синтетическ/i)
})
