import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'

const shift = (date: Date, days: number) => new Date(date.getTime() + days * 86_400_000)
const key = (date: Date) => date.toISOString().slice(0, 10)

async function goToMonth(page: Page, date: Date) {
  const target = new Intl.DateTimeFormat('cs-CZ', { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(date)
  for (let attempt = 0; attempt < 3 && await page.locator('#rental-month').textContent() !== target; attempt++) {
    const previous = await page.locator('#rental-month').textContent()
    await page.getByRole('button', { name: 'Další měsíc' }).click()
    await expect(page.locator('#rental-month')).not.toHaveText(previous || '')
  }
  await expect(page.locator('#rental-month')).toHaveText(target)
}

test('administrator blocks and releases days, with conflict protection and public availability', async ({ page, browser }) => {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const created = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name: 'Sál pro blokace', description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(created.status()).toBe(200)
  const id = (await created.json()).id
  const future = shift(new Date(), 14)
  const monday = shift(future, (8 - future.getUTCDay()) % 7)
  const tuesday = shift(monday, 1)
  const wednesday = shift(monday, 2)
  const application = await page.request.post('/api/v1/rentals', { multipart: {
    asset_id: String(id), start_date: key(monday), end_date: key(monday), start_time: '10:00', end_time: '11:00', event_type: 'standard',
    name: 'Žadatel', email: 'request@vysker.test', phone: '+420 777 123 456',
    address: 'Vyskeř 123', consent: 'true',
    identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
  } })
  expect(application.status()).toBe(201)

  await page.goto('/admin/pronajmy/majetek')
  const asset = page.locator('section.rental-asset-card').filter({ has: page.getByRole('heading', { name: 'Sál pro blokace', exact: true }) })
  await expect(asset).toBeVisible()
  await expect(asset.getByText('Blokované termíny', { exact: true })).toBeVisible()
  const block = asset.locator('section').filter({ hasText: 'Blokované termíny' })
  await block.getByLabel('Od', { exact: true }).fill(key(monday))
  await block.getByLabel('Do včetně', { exact: true }).fill(key(monday))
  const conflict = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rental-assets/${id}/blocks`) && response.request().method() === 'POST')
  await block.getByRole('button', { name: 'Blokovat termín' }).click()
  expect((await conflict).status()).toBe(409)
  const listAfterConflict = await page.request.get(`/api/v1/admin/rental-assets/${id}/blocks`)
  expect(await listAfterConflict.json()).toEqual([])

  await block.getByLabel('Od', { exact: true }).fill(key(tuesday))
  await block.getByLabel('Do včetně', { exact: true }).fill(key(wednesday))
  const posted = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rental-assets/${id}/blocks`) && response.request().method() === 'POST')
  await block.getByRole('button', { name: 'Blokovat termín' }).click()
  expect((await posted).status()).toBe(201)
  await expect(block.getByRole('button', { name: 'Odstranit blokaci' })).toBeVisible()
  const publicAvailability = await (await page.request.get(`/api/v1/rental-assets/${id}/availability`)).json()
  const publicBlock = publicAvailability.find((item: { start_date: string, end_date: string }) => item.start_date === key(tuesday) && item.end_date === key(wednesday))
  expect(publicBlock).toEqual({ start_date: key(tuesday), end_date: key(wednesday), start_time: null, end_time: null, status: 'booked' })
  expect(JSON.stringify(publicAvailability)).not.toContain('blocked')
  expect(JSON.stringify(publicAvailability)).not.toContain('reason')

  const publicContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const publicPage = await publicContext.newPage()
  try {
    await publicPage.goto('/pronajmy')
    const select = publicPage.getByLabel('Co si chcete pronajmout')
    await expect(select).toBeEnabled()
    await select.selectOption({ label: 'Sál pro blokace' })
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()
    await publicPage.locator('#rental-event-type').selectOption('standard')
    await goToMonth(publicPage, tuesday)
    const blockedDay = publicPage.locator(`#rental-calendar [data-date="${key(tuesday)}"]`)
    await expect(blockedDay).toHaveAccessibleName(/obsazeno/)
    await expect(blockedDay).toBeDisabled()
    await expect(publicPage.getByText('Obsazeno', { exact: true })).toBeVisible()
    await expect(publicPage.getByText('Blokováno obcí', { exact: false })).toHaveCount(0)
    await expect(publicPage.getByLabel('Datum', { exact: true })).toHaveValue('')
    await expect(publicPage.getByLabel('Do včetně')).toHaveValue('')

    await block.getByRole('button', { name: 'Odstranit blokaci' }).click()
    const dialog = page.getByRole('dialog', { name: 'Odstranit blokaci?' })
    await expect(dialog).toBeVisible()
    const removed = page.waitForResponse(response => response.url().includes(`/api/v1/admin/rental-assets/${id}/blocks/`) && response.request().method() === 'DELETE')
    await dialog.getByRole('button', { name: 'Odstranit blokaci' }).click()
    expect((await removed).status()).toBe(204)
    await expect(block.getByRole('button', { name: 'Odstranit blokaci' })).toHaveCount(0)
    await publicPage.reload()
    await expect(select).toBeEnabled()
    await select.selectOption({ label: 'Sál pro blokace' })
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()
    await goToMonth(publicPage, tuesday)
    await expect(blockedDay).toHaveAccessibleName(/volno/)
    await expect(blockedDay).toBeEnabled()
    await blockedDay.click()
    await expect(publicPage.getByLabel('Datum', { exact: true })).toHaveValue(key(tuesday))
  } finally {
    await publicContext.close()
  }
})
