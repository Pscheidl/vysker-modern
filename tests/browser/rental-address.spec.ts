import { test, expect, type Page } from './fixtures'

test.use({ timezoneId: 'Europe/Prague' })
test.beforeEach(async ({ page }) => {
  // Address tests never send typed test data to the external lookup provider.
  await page.route('**/api/v1/report-addresses?*', route => route.fulfill({ json: [] }))
})
const suggestion = { label: 'Vyskeř 50, 512 64 Vyskeř', latitude: 50.5295088, longitude: 15.158555,
  municipality: 'Vyskeř', locality: 'Vyskeř', is_vysker: true }

async function prepare(page: Page) {
  await page.clock.setFixedTime(new Date('2030-06-03T12:00:00Z'))
  await page.route('**/api/v1/rental-assets', route => route.fulfill({ json: [
    { id: 1, name: 'Sál', description: '', rates: {
      nonprofit: { hourly_price: 0, weekend_day_price: 0 },
      standard: { hourly_price: 200, weekend_day_price: 6000 },
    } },
  ] }))
  await page.route('**/api/v1/rental-assets/1/availability', route => route.fulfill({ json: [] }))
  await page.goto('/pronajmy')
  await expect(page.getByLabel('Datum', { exact: true })).toBeEnabled()
  await page.locator('#rental-event-type').selectOption('standard')
}

async function fillRequiredFields(page: Page) {
  await page.getByLabel('Datum', { exact: true }).fill('2030-06-04')
  await page.locator('#rental-start-time').selectOption('10:00')
  await page.locator('#rental-end-time').selectOption('11:00')
  await page.getByLabel('Jméno a příjmení', { exact: true }).fill('Jan Novák')
  await page.getByLabel('E-mail', { exact: true }).fill('jan@vysker.test')
  await page.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
  await page.getByLabel('Kopie občanského průkazu', { exact: true }).setInputFiles('public/images/vysker-znak-app-192.png')
  await page.getByLabel('Souhlasím s dočasným uložením').check()
}

test('address suggestions appear automatically, support keyboard and do not affect event pricing', async ({ page }) => {
  let searches = 0
  let submitted = ''
  await page.route('**/api/v1/report-addresses?*', route => { searches++; return route.fulfill({ json: [suggestion] }) })
  await page.route('**/api/v1/rentals', route => {
    submitted = route.request().postDataBuffer()?.toString('utf8') || ''
    return route.fulfill({ status: 201, json: { id: 1 } })
  })
  await prepare(page)
  const address = page.getByRole('combobox', { name: 'Adresa', exact: true })
  await expect(page.getByLabel('Nabídnout adresy')).toHaveCount(0)
  await address.fill('Vy')
  await page.waitForTimeout(800)
  expect(searches).toBe(0)
  await address.fill('Vyskeř 5')
  await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toBeVisible()
  expect(searches).toBe(1)
  await expect(address).toHaveValue('Vyskeř 5')
  await address.press('ArrowDown')
  await address.press('Enter')
  await expect(address).toHaveValue(suggestion.label)
  await expect(address).toHaveAttribute('aria-expanded', 'false')
  await expect(page.locator('#rental-event-type')).toHaveValue('standard')
  await expect(page.locator('#rental-is-local')).toHaveCount(0)
  await fillRequiredFields(page)
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  await expect(page.locator('#rental-message')).toContainText('Žádost byla odeslána')
  expect(submitted).toContain('name="event_type"\r\n\r\nstandard')
  expect(submitted).toContain('name="address"\r\n\r\nVyskeř 50, 512 64 Vyskeř')
  expect(submitted).not.toContain('name="organization_ico"')
  expect(submitted).not.toContain('name="responsible_name"')
  expect(submitted).not.toContain('name="locality_source"')
  expect(submitted).not.toContain('name="is_local"')
})

for (const serviceState of ['available', 'empty', 'unavailable']) {
  test(`address can be typed without choosing a suggestion when search is ${serviceState}`, async ({ page }) => {
    await page.route('**/api/v1/report-addresses?*', route => serviceState !== 'unavailable'
      ? route.fulfill({ json: serviceState === 'available' ? [suggestion] : [] })
      : route.fulfill({ status: 503, json: { error: 'Služba není dostupná.' } }))
    let submitted = ''
    await page.route('**/api/v1/rentals', route => {
      submitted = route.request().postDataBuffer()?.toString('utf8') || ''
      return route.fulfill({ status: 201, json: { id: 1 } })
    })
    await prepare(page)
    await fillRequiredFields(page)
    const address = page.getByRole('combobox', { name: 'Adresa', exact: true })
    const typed = 'Vlastní adresa 123, 512 64'
    await address.fill(typed)
    if (serviceState === 'available') await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toBeVisible()
    else if (serviceState === 'empty') await expect(page.locator('#rental-address-status')).toContainText('Adresu jsme nenašli')
    else await expect(page.locator('#rental-address-status')).toContainText('Adresu můžete zadat ručně')
    await page.getByRole('button', { name: 'Odeslat žádost' }).click()
    await expect(page.locator('#rental-message')).toContainText('Žádost byla odeslána')
    expect(submitted).toContain(typed)
    expect(submitted).toContain('name="event_type"\r\n\r\nstandard')
  })
}

test('dismissing suggestions and switching to an organization preserve the typed address', async ({ page }) => {
  let searches = 0
  await page.route('**/api/v1/report-addresses?*', route => {
    searches++
    return route.fulfill({ json: [suggestion] })
  })
  await prepare(page)
  const address = page.getByRole('combobox', { name: 'Adresa', exact: true })
  const typed = 'Moje vlastní adresa 12'
  await address.fill(typed)
  await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toBeVisible()
  await address.press('ArrowDown')
  await address.press('Escape')
  await expect(address).toHaveValue(typed)
  await expect(address).toHaveAttribute('aria-expanded', 'false')
  await expect(address).not.toHaveAttribute('aria-activedescendant')

  await address.fill('Jiná vlastní adresa 34')
  await page.locator('#rental-tenant-type').selectOption('organization')
  await expect(address).toBeHidden()
  await page.waitForTimeout(800)
  expect(searches).toBe(1)
  await page.locator('#rental-tenant-type').selectOption('person')
  await expect(address).toHaveValue('Jiná vlastní adresa 34')
  await address.fill('Další vlastní adresa 56')
  await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toBeVisible()
  expect(searches).toBe(2)
  await address.press('Tab')
  await expect(address).toHaveAttribute('aria-expanded', 'false')
  await expect(address).toHaveValue('Další vlastní adresa 56')
})

test('changing person or organization does not change the event type', async ({ page }) => {
  await prepare(page)
  await expect(page.locator('#rental-event-type option[value="nonprofit"]')).toHaveText('Veřejně prospěšná nezisková akce')
  await page.locator('#rental-event-type').selectOption('nonprofit')
  await page.locator('#rental-tenant-type').selectOption('organization')
  await expect(page.getByLabel('Název firmy nebo organizace')).toBeVisible()
  await expect(page.getByLabel('IČO', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Jméno a příjmení odpovědné osoby')).toBeVisible()
  await expect(page.locator('#rental-address')).toBeHidden()
  await expect(page.locator('#rental-event-type')).toHaveValue('nonprofit')
  await expect(page.locator('#rental-is-local')).toHaveCount(0)
  await page.locator('#rental-tenant-type').selectOption('person')
  await expect(page.getByLabel('Jméno a příjmení', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Adresa', { exact: true })).toBeVisible()
  await expect(page.locator('#rental-event-type')).toHaveValue('nonprofit')
  await expect(page.getByRole('link', { name: '481 329 211' })).toHaveAttribute('href', 'tel:+420481329211')
  await expect(page.getByRole('link', { name: 'vysker@craj.cz' })).toHaveAttribute('href', 'mailto:vysker@craj.cz')
})

test('organization requires an ID, responsible person and their document without submitting a personal address', async ({ page }) => {
  let submitted = ''
  let requests = 0
  await page.route('**/api/v1/rentals', route => {
    requests++
    submitted = route.request().postDataBuffer()?.toString('utf8') || ''
    return route.fulfill({ status: 201, json: { id: 1 } })
  })
  await prepare(page)
  await fillRequiredFields(page)
  await page.locator('#rental-address').fill('Osobní adresa se nesmí odeslat')
  await page.locator('#rental-tenant-type').selectOption('organization')
  await page.getByLabel('Název firmy nebo organizace').fill('Testovací spolek')
  const ico = page.getByLabel('IČO', { exact: true })
  const responsible = page.getByLabel('Jméno a příjmení odpovědné osoby')
  const identity = page.getByLabel('Kopie občanského průkazu odpovědné osoby', { exact: true })
  const consent = page.locator('input[name="consent"]')
  await expect(page.locator('#rental-address')).toBeHidden()
  await expect(ico).toBeVisible()
  await expect(responsible).toBeVisible()
  await expect(consent).not.toBeChecked()
  await consent.check()
  expect(await ico.evaluate((input: HTMLInputElement) => input.validity.valueMissing)).toBe(true)
  expect(await responsible.evaluate((input: HTMLInputElement) => input.validity.valueMissing)).toBe(true)
  await identity.setInputFiles([])
  expect(await identity.evaluate((input: HTMLInputElement) => input.validity.valueMissing)).toBe(true)
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  expect(requests).toBe(0)

  await ico.fill('12345679')
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  expect(requests).toBe(0)
  await responsible.fill('Jana Odpovědná')
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  expect(requests).toBe(0)
  await identity.setInputFiles('public/images/vysker-znak-app-192.png')
  await ico.fill('12345678')
  await ico.blur()
  expect(await ico.evaluate((input: HTMLInputElement) => input.validity.customError)).toBe(true)
  expect(await ico.evaluate((input: HTMLInputElement) => input.validationMessage)).not.toBe('')
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  expect(requests).toBe(0)
  await ico.fill('12345679')
  await ico.blur()
  expect(await ico.evaluate((input: HTMLInputElement) => input.validity.valid)).toBe(true)
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  await expect(page.locator('#rental-message')).toContainText('Žádost byla odeslána')
  expect(requests).toBe(1)
  expect(submitted).toContain('name="organization_ico"\r\n\r\n12345679')
  expect(submitted).toContain('name="responsible_name"\r\n\r\nJana Odpovědná')
  expect(submitted).not.toContain('name="address"')
  expect(submitted).not.toContain('Osobní adresa se nesmí odeslat')
})

test('personal applications still require an address', async ({ page }) => {
  let requests = 0
  let submitted = ''
  await page.route('**/api/v1/rentals', route => {
    requests++
    submitted = route.request().postDataBuffer()?.toString('utf8') || ''
    return route.fulfill({ status: 201, json: { id: 1 } })
  })
  await prepare(page)
  await page.locator('#rental-tenant-type').selectOption('organization')
  await page.getByLabel('IČO', { exact: true }).fill('12345679')
  await page.getByLabel('Jméno a příjmení odpovědné osoby').fill('Jana Odpovědná')
  await page.locator('#rental-tenant-type').selectOption('person')
  await fillRequiredFields(page)
  const address = page.getByLabel('Adresa', { exact: true })
  await expect(address).toBeVisible()
  expect(await address.evaluate((input: HTMLInputElement) => input.validity.valueMissing)).toBe(true)
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  expect(requests).toBe(0)
  await address.fill('Vyskeř 123')
  await page.getByRole('button', { name: 'Odeslat žádost' }).click()
  await expect(page.locator('#rental-message')).toContainText('Žádost byla odeslána')
  expect(submitted).toContain('name="address"\r\n\r\nVyskeř 123')
  expect(submitted).not.toContain('name="organization_ico"')
  expect(submitted).not.toContain('name="responsible_name"')
  expect(submitted).not.toContain('Jana Odpovědná')
})
