import { test, expect } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'

const shift = (date: Date, days: number) => new Date(date.getTime() + days * 86_400_000)
const key = (date: Date) => date.toISOString().slice(0, 10)

test('an applicant submits a weekday hourly request and its hours become unavailable', async ({ page, browser }) => {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const name = `Hodinový pronájem ${Date.now()}`
  const assetResponse = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name, description: '', rates: testRates(30), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(assetResponse.status()).toBe(200)
  const assetId = (await assetResponse.json()).id
  const future = shift(new Date(), 14)
  const monday = shift(future, (8 - future.getUTCDay()) % 7)
  const targetMonth = new Intl.DateTimeFormat('cs-CZ', { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(monday)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const publicPage = await context.newPage()
    await publicPage.goto('/pronajmy')
    await publicPage.getByLabel('Co si chcete pronajmout').selectOption({ label: name })
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()
    await publicPage.locator('#rental-event-type').selectOption('standard')
    for (let i = 0; i < 3 && await publicPage.locator('#rental-month').textContent() !== targetMonth; i++) {
      const oldMonth = await publicPage.locator('#rental-month').textContent()
      await publicPage.getByRole('button', { name: 'Další měsíc' }).click()
      await expect(publicPage.locator('#rental-month')).not.toHaveText(oldMonth || '')
    }
    await expect(publicPage.locator('#rental-month')).toHaveText(targetMonth)
    await publicPage.locator(`#rental-calendar [data-date="${key(monday)}"]`).click()
    await publicPage.locator('#rental-hours [data-start-minute="1200"]').click()
    await publicPage.locator('#rental-hours [data-start-minute="1260"]').click()
    await expect(publicPage.locator('#rental-start-time')).toHaveValue('20:00')
    await expect(publicPage.locator('#rental-end-time')).toHaveValue('22:00')
    await publicPage.getByLabel('Jméno a příjmení', { exact: true }).fill('Hodinový nájemce')
    await publicPage.getByLabel('E-mail', { exact: true }).fill('hourly@vysker.test')
    await publicPage.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
    await publicPage.getByLabel('Adresa', { exact: true }).fill('Vyskeř 123')
    await publicPage.getByLabel('Kopie občanského průkazu', { exact: true }).setInputFiles('public/images/vysker-znak-app-192.png')
    await publicPage.getByLabel('Souhlasím s dočasným uložením').check()
    const created = publicPage.waitForResponse(res => res.url().endsWith('/api/v1/rentals') && res.request().method() === 'POST')
    await publicPage.getByRole('button', { name: 'Odeslat žádost', exact: true }).click()
    expect((await created).status()).toBe(201)
    await expect(publicPage.locator('#rental-message')).toContainText('Žádost byla odeslána')
    const availability = await (await publicPage.request.get(`/api/v1/rental-assets/${assetId}/availability`)).json()
    expect(availability).toEqual(expect.arrayContaining([expect.objectContaining({
      start_date: key(monday), end_date: key(monday), start_time: '20:00', end_time: '22:00', status: 'pending',
    })]))
    const fullDayConflict = await publicPage.request.post('/api/v1/rentals', { multipart: {
      asset_id: String(assetId), start_date: key(monday), end_date: key(monday), event_type: 'standard',
      name: 'Další nájemce', email: 'full-day@example.test', phone: '+420 777 123 456',
      address: 'Vyskeř 1', consent: 'true',
      identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
    } })
    expect(fullDayConflict.status()).toBe(409)
    await publicPage.locator('#rental-mode-day').click()
    await expect(publicPage.locator(`#rental-calendar [data-date="${key(monday)}"]`)).toBeDisabled()
    await publicPage.locator('#rental-mode-hour').click()
    await publicPage.locator(`#rental-calendar [data-date="${key(monday)}"]`).click()
    await expect(publicPage.locator('#rental-hours [data-start-minute="1200"]')).toBeDisabled()
    await expect(publicPage.locator('#rental-hours [data-start-minute="1260"]')).toBeDisabled()
    await expect(publicPage.locator('#rental-hours [data-start-minute="1320"]')).toBeEnabled()
  } finally {
    await context.close()
  }
})
