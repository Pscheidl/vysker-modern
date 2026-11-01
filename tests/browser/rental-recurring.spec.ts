import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'

const shift = (date: Date, days: number) => new Date(date.getTime() + days * 86_400_000)
const key = (date: Date) => date.toISOString().slice(0, 10)

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('weekly and fortnightly reservations have individual private prices and a conflict preview', async ({ page, browser }) => {
  await login(page)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const assetName = `Soukromá dohoda ${Date.now()}`
  const response = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name: assetName, description: '', rates: testRates(30), notification_email: 'office@vysker.test',
      require_meter: false, active: true },
  })
  expect(response.status()).toBe(200)
  const assetId = (await response.json()).id
  const future = shift(new Date(), 14)
  const monday = shift(future, (8 - future.getUTCDay()) % 7)
  const until = shift(monday, 28)

  async function fillForm(recurrence: string, from: string, to: string) {
    await page.goto('/admin/pronajmy/nova')
    await page.getByLabel('Předmět pronájmu').selectOption({ label: assetName })
    await page.getByLabel('Datum', { exact: true }).fill(key(monday))
    await page.getByLabel('Od hodin').selectOption(from)
    await page.getByLabel('Do hodin').selectOption(to)
    await page.getByRole('combobox', { name: 'Opakování', exact: true }).selectOption(recurrence)
    await page.getByLabel('Opakovat do včetně').fill(key(until))
    await page.getByLabel('Typ nájemce').selectOption('organization')
    await page.getByLabel('Typ akce').selectOption('standard')
    await page.getByLabel('Název firmy nebo organizace').fill('Soukromý spolek')
    await page.getByLabel('IČO').fill('12345679')
    await page.getByLabel('Jméno a příjmení odpovědné osoby').fill('Jana Zkušební')
    await page.getByLabel('E-mail pro vyúčtování').fill('private@vysker.test')
    await page.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
    await expect(page.getByLabel('Adresa sídla')).toHaveCount(0)
    await expect(page.getByLabel('Adresa', { exact: true })).toHaveCount(0)
    await page.getByLabel('Kopie občanského průkazu odpovědné osoby').setInputFiles('public/images/vysker-znak-app-192.png')
    await page.getByLabel('Potvrzuji, že odpovědná osoba souhlasí').check()
    await page.getByLabel('Sjednaná cena za jeden termín v Kč').fill('1739')
    await page.getByLabel('Energie zahrnuté v ceně pronájmu').check()
    await page.getByLabel('Nečekat s uzavřením na potvrzení platby').check()
  }

  async function previewAndCreate(count: number) {
    const preview = page.waitForResponse(res => res.url().endsWith('/api/v1/admin/rentals/preview') && res.request().method() === 'POST')
    await page.getByRole('button', { name: 'Zkontrolovat termíny' }).click()
    const previewResponse = await preview
    expect(previewResponse.status()).toBe(200)
    const result = await previewResponse.json()
    expect(result.occurrence_count).toBe(count)
    expect(result.conflict_count).toBe(0)
    expect(result.total_price).toBe(count * 1739)
    await expect(page.getByRole('heading', { name: 'Všechny termíny jsou volné' })).toBeVisible()
    const create = page.waitForResponse(res => res.url().endsWith('/api/v1/admin/rentals') && res.request().method() === 'POST')
    await page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' }).click()
    const created = await create
    expect(created.status()).toBe(201)
    const body = await created.json()
    expect(body.rentals).toHaveLength(count)
    for (const rental of body.rentals) {
      expect(rental.rental_price).toBe(1739)
      expect(rental.require_meter).toBe(false)
      expect(rental.wait_for_payment).toBe(false)
      expect(rental.tenant_type).toBe('organization')
      expect(rental.event_type).toBe('standard')
      expect(rental.organization_ico).toBe('12345679')
      expect(rental.responsible_name).toBe('Jana Zkušební')
      expect(rental.identity_uploaded).toBe(true)
      expect(rental.address).toBe('')
    }
    await expect(page.getByText(`Počet vytvořených termínů: ${count}.`, { exact: false })).toBeVisible()
    return body.rentals[0].id as number
  }

  await fillForm('7', '20:00', '21:00')
  const firstId = await previewAndCreate(5)
  await page.goto(`/admin/pronajmy/${firstId}`)
  await expect(page.getByRole('heading', { name: `Pronájem č. ${firstId}` })).toBeVisible()
  await expect(page.locator('.rental-detail-facts')).toContainText('12345679')
  await expect(page.locator('.rental-detail-facts')).toContainText('Jana Zkušební')
  await expect(page.getByRole('heading', { name: 'Kopie občanského průkazu odpovědné osoby', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Zobrazit kopii dokladu', exact: true })).toBeVisible()
  await expect(page.getByLabel('Spotřeba elektřiny v kWh')).toHaveCount(0)
  await expect(page.getByLabel('Celková cena energie v Kč')).toHaveCount(0)
  await fillForm('14', '21:00', '22:00')
  await previewAndCreate(3)

  await fillForm('7', '20:00', '21:00')
  const conflict = page.waitForResponse(res => res.url().endsWith('/api/v1/admin/rentals/preview') && res.request().method() === 'POST')
  await page.getByRole('button', { name: 'Zkontrolovat termíny' }).click()
  expect((await (await conflict).json()).conflict_count).toBe(5)
  await expect(page.getByRole('heading', { name: 'Některé termíny jsou obsazené' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' })).toHaveCount(0)

  const publicContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const publicPage = await publicContext.newPage()
    const availability = await publicPage.request.get(`/api/v1/rental-assets/${assetId}/availability`)
    expect(availability.status()).toBe(200)
    const text = await availability.text()
    expect(text).not.toContain('1739')
    expect(text).not.toContain('Soukromý spolek')
    expect(text).not.toContain('private@vysker.test')
    expect(text).not.toContain('12345679')
    expect(text).not.toContain('Jana Zkušební')
    const assets = await (await publicPage.request.get('/api/v1/rental-assets')).text()
    expect(assets).not.toContain('1739')
    expect(assets).not.toContain('Soukromý spolek')
    expect(assets).not.toContain('private@vysker.test')
    await publicPage.goto('/pronajmy')
    await publicPage.getByLabel('Co si chcete pronajmout').selectOption({ label: assetName })
    await expect(publicPage.locator('body')).not.toContainText('1739')
    await expect(publicPage.locator('body')).not.toContainText('Soukromý spolek')
  } finally {
    await publicContext.close()
  }
})

test('administrator can choose weekday hours or full days and weekends only permit full days', async ({ page }) => {
  await login(page)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const assetName = `Volba termínu ${Date.now()}`
  const response = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name: assetName, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(response.status()).toBe(200)
  const future = shift(new Date(), 14)
  const saturday = shift(future, (6 - future.getUTCDay() + 7) % 7)
  const sunday = shift(saturday, 1)
  const monday = shift(saturday, 2)
  await page.goto('/admin/pronajmy/nova')
  await page.getByLabel('Předmět pronájmu').selectOption({ label: assetName })
  await expect(page.getByText('Ověření dokladu provede úřední osoba.', { exact: false })).toBeVisible()
  await expect(page.getByLabel('Kopie občanského průkazu', { exact: true })).toHaveCount(0)
  const mode = page.getByLabel('Délka pronájmu')
  await expect(mode).toHaveValue('hourly')
  await page.getByLabel('Datum', { exact: true }).fill(key(saturday))
  await expect(mode).toHaveValue('daily')
  await expect(mode.locator('option[value="hourly"]')).toHaveJSProperty('disabled', true)
  const end = page.getByLabel('Do včetně')
  await expect(end).toHaveValue(key(saturday))
  await expect(end).toHaveAttribute('max', key(shift(saturday, 27)))
  await expect(page.getByLabel('Od hodin')).toHaveCount(0)
  await end.fill(key(sunday))
  await expect(end).toHaveValue(key(sunday))
  await page.getByLabel('Datum', { exact: true }).fill(key(sunday))
  await expect(end).toHaveValue(key(sunday))
  await expect(end).toHaveAttribute('max', key(shift(sunday, 27)))
  await page.getByLabel('Datum', { exact: true }).fill(key(monday))
  await expect(mode).toHaveValue('hourly')
  await expect(page.getByLabel('Od hodin')).toBeVisible()
  await expect(page.getByLabel('Do hodin')).toBeVisible()
  await expect(end).toHaveCount(0)
  await mode.selectOption('daily')
  await expect(page.getByLabel('Od hodin')).toHaveCount(0)
  await expect(end).toHaveValue(key(monday))
  await expect(end).toHaveAttribute('max', key(shift(monday, 27)))
  await page.getByLabel('Datum', { exact: true }).fill(key(saturday))
  await page.getByLabel('Datum', { exact: true }).fill(key(monday))
  await expect(mode).toHaveValue('daily')
})

test('monthly reservations use the same ordinal weekday, save the series and report conflicts', async ({ page }) => {
  await login(page)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const assetName = `Měsíční rezervace ${Date.now()}`
  const asset = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name: assetName, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(asset.status()).toBe(200)

  const firstMonth = new Date(Date.now() + 45 * 86_400_000)
  const ordinalThursday = (offset: number) => {
    const first = new Date(Date.UTC(firstMonth.getUTCFullYear(), firstMonth.getUTCMonth() + offset, 1))
    return new Date(Date.UTC(first.getUTCFullYear(), first.getUTCMonth(), 1 + (4 - first.getUTCDay() + 7) % 7 + 7))
  }
  const expectedDates = [0, 1, 2].map(offset => key(ordinalThursday(offset)))

  async function fillMonthlyForm() {
    await page.goto('/admin/pronajmy/nova')
    await page.getByLabel('Předmět pronájmu').selectOption({ label: assetName })
    await page.getByLabel('Datum', { exact: true }).fill(expectedDates[0])
    await page.getByLabel('Od hodin').selectOption('18:00')
    await page.getByLabel('Do hodin').selectOption('19:00')
    const recurrence = page.getByRole('combobox', { name: 'Opakování', exact: true })
    await expect(recurrence.locator('option[value="monthly"]')).toHaveText('Jednou měsíčně')
    await recurrence.selectOption('monthly')
    await expect(page.getByText('Každý měsíc: 2. čtvrtek.', { exact: true })).toBeVisible()
    await page.getByLabel('Opakovat do včetně').fill(expectedDates[2])
    await page.getByLabel('Jméno a příjmení', { exact: true }).fill('Jan Zkušební')
    await page.getByLabel('Adresa', { exact: true }).fill('Vyskeř 50')
    await page.getByLabel('E-mail pro vyúčtování').fill('mesicni@example.test')
    await page.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
    await page.getByLabel('Sjednaná cena za jeden termín v Kč').fill('550')
    await page.getByLabel('Energie zahrnuté v ceně pronájmu').check()
    await expect(page.getByText('Ověření dokladu provede úřední osoba.', { exact: false })).toBeVisible()
    await expect(page.getByLabel('Kopie občanského průkazu', { exact: true })).toHaveCount(0)
  }

  await fillMonthlyForm()
  const previewWait = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rentals/preview') && response.request().method() === 'POST')
  await page.getByRole('button', { name: 'Zkontrolovat termíny' }).click()
  const previewResponse = await previewWait
  expect(previewResponse.status()).toBe(200)
  expect(previewResponse.request().postDataJSON()).toMatchObject({ recurrence_interval_days: 0, recurrence_interval_months: 1 })
  const preview = await previewResponse.json()
  expect(preview.occurrences.map((item: { start_date: string }) => item.start_date)).toEqual(expectedDates)
  expect(preview.occurrence_count).toBe(3)
  expect(preview.conflict_count).toBe(0)
  expect(preview.total_price).toBe(1650)
  await expect(page.getByRole('heading', { name: 'Všechny termíny jsou volné' })).toBeVisible()
  await page.locator('section.rental-create-preview').screenshot({ path: 'target/rental-monthly-preview-test.png',
    style: '.site-header, .skip-link { visibility: hidden !important; }' })
  const createWait = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rentals') && response.request().method() === 'POST')
  await page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' }).click()
  const createResponse = await createWait
  expect(createResponse.status()).toBe(201)
  const created = await createResponse.json()
  expect(created.rentals.map((item: { start_date: string }) => item.start_date)).toEqual(expectedDates)
  expect(created.rentals).toHaveLength(3)
  const firstId = created.rentals[0].id
  const detail = await (await page.request.get(`/api/v1/admin/rentals/${firstId}`)).json()
  expect(detail.recurrence_interval_months).toBe(1)
  await page.goto(`/admin/pronajmy/${firstId}`)
  await expect(page.locator('main')).toContainText('měsíčně')

  await fillMonthlyForm()
  const conflictWait = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rentals/preview') && response.request().method() === 'POST')
  await page.getByRole('button', { name: 'Zkontrolovat termíny' }).click()
  const conflict = await (await conflictWait).json()
  expect(conflict.conflict_count).toBe(3)
  await expect(page.getByRole('heading', { name: 'Některé termíny jsou obsazené' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' })).toHaveCount(0)

  const fifthThursday = (offset: number) => {
    const first = new Date(Date.UTC(firstMonth.getUTCFullYear(), firstMonth.getUTCMonth() + offset, 1))
    const day = 1 + (4 - first.getUTCDay() + 7) % 7 + 28
    const result = new Date(Date.UTC(first.getUTCFullYear(), first.getUTCMonth(), day))
    return result.getUTCMonth() === first.getUTCMonth() ? result : null
  }
  let firstFifthMonth = 0
  while (!fifthThursday(firstFifthMonth)) firstFifthMonth++
  const fifthDates = Array.from({ length: 5 }, (_, offset) => fifthThursday(firstFifthMonth + offset)).filter((date): date is Date => date !== null).map(key)
  expect(fifthDates.length).toBeGreaterThan(0)
  expect(fifthDates.length).toBeLessThan(5)
  const lastMonthEnd = new Date(Date.UTC(firstMonth.getUTCFullYear(), firstMonth.getUTCMonth() + firstFifthMonth + 5, 0))
  await page.getByLabel('Datum', { exact: true }).fill(fifthDates[0])
  await expect(page.getByText('Pokud tento den v měsíci není, měsíc se vynechá.', { exact: false })).toBeVisible()
  await page.getByLabel('Opakovat do včetně').fill(key(lastMonthEnd))
  const fifthPreviewWait = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rentals/preview') && response.request().method() === 'POST')
  await page.getByRole('button', { name: 'Zkontrolovat termíny' }).click()
  const fifthPreviewResponse = await fifthPreviewWait
  expect(fifthPreviewResponse.status()).toBe(200)
  const fifthPreview = await fifthPreviewResponse.json()
  expect(fifthPreview.occurrences.map((item: { start_date: string }) => item.start_date)).toEqual(fifthDates)
  expect(fifthPreview.conflict_count).toBe(0)
})
