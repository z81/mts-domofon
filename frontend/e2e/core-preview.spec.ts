import { expect, test } from '@playwright/test'

test('navigates core preview and keeps door actions behind confirmation', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Домофон' })).toBeVisible()
  await page.getByRole('button', { name: 'Открыть Подъезд' }).click()
  await expect(page.getByRole('dialog', { name: 'Открыть Подъезд?' })).toBeVisible()
})
