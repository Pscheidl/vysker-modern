import { test, expect } from './fixtures'
import { testRates } from './rental-test-data'

test('hourly selection and recurring administration fit on a mobile screen', async ({ page }) => {
  const pageErrors: string[] = []
  page.on('pageerror', error => pageErrors.push(error.message))
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const name = `Mobilní rezervace ${Date.now()}`
  const created = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name, description: '', rates: testRates(30), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(created.status()).toBe(200)
  const future = new Date(Date.now() + 14 * 86_400_000)
  future.setUTCDate(future.getUTCDate() + (8 - future.getUTCDay()) % 7)
  const date = future.toISOString().slice(0, 10)

  await page.goto('/pronajmy')
  await page.getByLabel('Co si chcete pronajmout').selectOption({ label: name })
  await expect(page.locator('#rental-fields')).toBeEnabled()
  await page.locator('#rental-event-type').selectOption('standard')
  await page.getByLabel('Datum', { exact: true }).fill(date)
  await page.locator('#rental-hours [data-start-minute="1200"]').click()
  await page.locator('#rental-hours [data-start-minute="1260"]').click()
  await expect(page.locator('#rental-selection-summary')).toContainText('60 Kč')
  await expect(page.locator('#rental-picked-price')).toBeVisible()
  await expect(page.locator('#rental-picked-price')).toContainText('60 Kč')
  await page.evaluate(() => document.fonts.ready)
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }))
  const consentBox = await page.getByLabel('Souhlasím s dočasným uložením').boundingBox()
  expect(consentBox?.width).toBeGreaterThanOrEqual(16)
  expect(consentBox?.height).toBeGreaterThanOrEqual(16)
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390)
  const screenshotStyle = '.site-header { position: static !important; } .skip-link { visibility: hidden !important; }'
  await page.screenshot({ path: 'target/rental-picker-full-mobile.png', fullPage: true, style: screenshotStyle })
  await page.setViewportSize({ width: 1440, height: 1000 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1440)
  await page.screenshot({ path: 'target/rental-picker-full-desktop.png', fullPage: true, style: screenshotStyle })

  await page.getByRole('button', { name: 'Tmavý motiv', exact: true }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 844 })
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width)
    await page.screenshot({ path: `target/rental-picker-dark-${width}.png`, fullPage: true, style: screenshotStyle })
  }
  await page.getByRole('button', { name: 'Světlý motiv', exact: true }).click()

  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/admin/pronajmy/nova')
  await page.getByLabel('Předmět pronájmu').selectOption({ label: name })
  await page.getByLabel('Datum', { exact: true }).fill(date)
  await page.getByRole('combobox', { name: 'Opakování', exact: true }).selectOption('14')
  await expect(page.getByLabel('Opakovat do včetně')).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390)
  await page.screenshot({ path: 'target/rental-mobile-recurring.png', fullPage: true })
  expect(pageErrors).toEqual([])
})
