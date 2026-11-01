import { test, expect } from './fixtures'

const shift = (date: Date, days: number) => new Date(date.getTime() + days * 86_400_000)
const key = (date: Date) => date.toISOString().slice(0, 10)

test('weekday and weekend full days use the same server-calculated daily price', async ({ page, browser }) => {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const name = `Sál se dvěma ceníky ${Date.now()}`
  const assetResponse = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name, description: '', rates: {
      nonprofit: { hourly_price: 0, weekend_day_price: 0 },
      standard: { hourly_price: 100, weekend_day_price: 250 },
    }, notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(assetResponse.status()).toBe(200)
  const assetId = (await assetResponse.json()).id
  const future = shift(new Date(), 14)
  const saturday = shift(future, (6 - future.getUTCDay() + 7) % 7)
  const sunday = shift(saturday, 1)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const publicPage = await context.newPage()
    await publicPage.goto('/pronajmy')
    await publicPage.getByLabel('Co si chcete pronajmout').selectOption({ label: name })
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()

    async function requestDay(day: Date, eventType: string, price: number, lastDay = day) {
      await publicPage.locator('#rental-event-type').selectOption(eventType)
      await publicPage.locator('#rental-tenant-type').selectOption('organization')
      await publicPage.getByLabel('Název firmy nebo organizace').fill('Zkušební spolek')
      await publicPage.getByLabel('IČO', { exact: true }).fill('12345679')
      await publicPage.getByLabel('Jméno a příjmení odpovědné osoby').fill('Jana Odpovědná')
      await publicPage.getByLabel('E-mail', { exact: true }).fill('group@example.test')
      await publicPage.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
      await publicPage.getByLabel('Kopie občanského průkazu odpovědné osoby', { exact: true }).setInputFiles('public/images/vysker-znak-app-192.png')
      await publicPage.locator('input[name="consent"]').check()
      await publicPage.locator('#rental-start').fill(key(day))
      await expect(publicPage.locator('#rental-end')).toHaveValue(key(day))
      if (key(lastDay) !== key(day)) await publicPage.locator('#rental-end').fill(key(lastDay))
      await expect(publicPage.locator('#rental-selection-summary')).toContainText(`${price} Kč`)
      if (eventType === 'standard' && key(day) === key(lastDay)) {
        await publicPage.locator('#rental-form').screenshot({ path: 'target/rental-organization-form-sample.png',
          style: '.site-header, .skip-link { visibility: hidden !important; }' })
      }
      const submitted = publicPage.waitForResponse(response => response.url().endsWith('/api/v1/rentals') && response.request().method() === 'POST')
      await publicPage.getByRole('button', { name: 'Odeslat žádost' }).click()
      const response = await submitted
      expect(response.status()).toBe(201)
      const { id } = await response.json()
      await expect(publicPage.locator('#rental-message')).toContainText('Žádost byla odeslána')
      const detailResponse = await page.request.get(`/api/v1/admin/rentals/${id}`)
      expect(detailResponse.status()).toBe(200)
      const detail = await detailResponse.json()
      expect(detail.rental_price).toBe(price)
      expect(detail.event_type).toBe(eventType)
      expect(detail.tenant_type).toBe('organization')
      expect(detail.name).toBe('Zkušební spolek')
      expect(detail.organization_ico).toBe('12345679')
      expect(detail.responsible_name).toBe('Jana Odpovědná')
      expect(detail.address).toBe('')
      expect(detail.start_date).toBe(key(day))
      expect(detail.end_date).toBe(key(lastDay))
      expect(detail.start_time).toBeNull()
      expect(detail.end_time).toBeNull()
      if (eventType === 'standard' && key(day) === key(lastDay)) {
        await page.goto(`/admin/pronajmy/${id}`)
        await expect(page.locator('main')).toContainText('Zkušební spolek')
        await expect(page.locator('main')).toContainText('12345679')
        await expect(page.locator('main')).toContainText('Jana Odpovědná')
      }
    }

    await publicPage.locator('#rental-mode-day').click()
    await requestDay(shift(saturday, -5), 'standard', 250)
    await requestDay(saturday, 'standard', 250)
    const sundayMonth = new Intl.DateTimeFormat('cs-CZ', { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(sunday)
    if (await publicPage.locator('#rental-month').textContent() !== sundayMonth) {
      await publicPage.getByRole('button', { name: 'Další měsíc' }).click()
      await expect(publicPage.locator('#rental-month')).toHaveText(sundayMonth)
    }
    await expect(publicPage.locator(`#rental-calendar [data-date="${key(sunday)}"]`)).toBeEnabled()
    await requestDay(sunday, 'nonprofit', 0)
    await requestDay(shift(saturday, 7), 'standard', 500, shift(sunday, 7))
    const availability = await (await publicPage.request.get(`/api/v1/rental-assets/${assetId}/availability`)).text()
    expect(availability).not.toContain('Zkušební spolek')
    expect(availability).not.toContain('group@example.test')
    expect(availability).not.toContain('rental_price')
  } finally {
    await context.close()
  }
})
