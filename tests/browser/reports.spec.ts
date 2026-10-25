import { devices } from '@playwright/test'
import { test, expect, type Page } from './fixtures'

const photograph = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg==', 'base64')

async function prepareMap(page: Page) {
  await page.route('**/api/v1/report-addresses?*', route => route.fulfill({ json: [] }))
  // Exercise the bundled map without relying on a third party's tile server.
  await page.route('https://tile.openstreetmap.org/**', route => {
    // The map overrides the site's no-referrer policy with origin-only attribution.
    expect(route.request().headers()['referer']).toBe('http://127.0.0.1:3107/')
    return route.fulfill({
      status: 200,
      contentType: 'image/png',
      body: photograph,
    })
  })
}

async function countLocationRequests(page: Page) {
  await page.addInitScript(() => {
    Object.defineProperty(window, 'reportGeolocationCalls', { value: 0, writable: true })
    const original = navigator.geolocation.getCurrentPosition.bind(navigator.geolocation)
    navigator.geolocation.getCurrentPosition = (...args) => {
      Reflect.set(window, 'reportGeolocationCalls', Number(Reflect.get(window, 'reportGeolocationCalls')) + 1)
      original(...args)
    }
  })
}

async function submitReport(page: Page) {
  const submitted = page.waitForResponse(response =>
    new URL(response.url()).pathname === '/api/v1/reports' && response.request().method() === 'POST')
  await page.getByRole('button', { name: 'Odeslat hlášení', exact: true }).click()
  const response = await submitted
  expect(response.status()).toBe(201)
  // The completed POST navigates immediately, which can release its DevTools body.
  // Read the persisted public record after navigation instead of racing that body.
  await expect(page).toHaveURL(/\/hlaseni-zavad\/\d+\?odeslano=1$/)
  await expect(page.locator('#report-submitted')).toHaveAttribute('role', 'status')
  await expect(page.locator('#report-submitted')).toContainText('Hlášení bylo odesláno. Děkujeme.')
  const id = new URL(page.url()).pathname.split('/').pop()
  const persisted = await page.request.get(`/api/v1/reports/${id}`)
  expect(persisted.status()).toBe(200)
  return persisted.json()
}

async function selectSizedPhotos(page: Page, files: { name: string, size: number }[]) {
  // Construct real Files in the browser to avoid transferring 50 MB through DevTools.
  await page.locator('#report-photo').evaluate((element, { files, png }) => {
    const input = element as HTMLInputElement
    const transfer = new DataTransfer()
    const header = Uint8Array.from(atob(png), character => character.charCodeAt(0))
    for (const { name, size } of files) {
      const bytes = new Uint8Array(size)
      bytes.set(header.subarray(0, size))
      transfer.items.add(new File([bytes], name, { type: 'image/png' }))
    }
    input.files = transfer.files
    input.dispatchEvent(new Event('change', { bubbles: true }))
  }, { files, png: photograph.toString('base64') })
}

test('an anonymous resident opens the separate form and submits a public report with a gallery and date', async ({ page }, testInfo) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await prepareMap(page)
  await countLocationRequests(page)
  await page.goto('/')
  await page.getByRole('navigation', { name: 'Hlavní navigace', exact: true })
    .getByRole('link', { name: 'Hlášení závad', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  await expect(page.getByRole('heading', { name: 'Nahlášené závady', exact: true })).toBeVisible()
  await expect(page.locator('#report-form')).toHaveCount(0)
  await page.getByRole('main').getByRole('link', { name: 'Nahlásit závadu', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad\/nove$/)
  await expect(page.getByRole('heading', { name: 'Nahlásit závadu', exact: true })).toBeVisible()
  await expect(page.locator('#report-form')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Použít moji polohu', exact: true })).toBeHidden()
  const firstSentence = `Rozbitá lavička u návsi ${Date.now()}`
  const nextSentence = 'Uvolněná opěrka lavičky potřebuje připevnit.'
  const description = `${firstSentence}. ${nextSentence}`
  await page.getByLabel('Popis problému', { exact: false }).fill(description)
  await page.getByLabel('Adresa nebo upřesnění místa', { exact: false }).fill('Vyskeř, náves u zastávky')
  await expect(page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true })).toHaveValue('')
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles([
    { name: 'lavicka.png', mimeType: 'image/png', buffer: photograph },
    { name: 'lavicka-detail.png', mimeType: 'image/png', buffer: photograph },
  ])
  await expect(page.locator('[data-report-photo]')).toHaveCount(2)
  const report = await submitReport(page)
  expect(report.reporter_name).toBeNull()
  expect(report.latitude).toBeNull()
  expect(report.longitude).toBeNull()
  expect(report.has_photo).toBe(true)
  expect(report.photo_count).toBe(2)
  expect(report).not.toHaveProperty('request_ip')
  expect(report).not.toHaveProperty('recipients')
  expect(Number.isNaN(Date.parse(report.created_at))).toBe(false)
  await expect(page.getByRole('main')).toContainText(description)
  await expect(page.getByRole('main')).toContainText('Vyskeř, náves u zastávky')
  const gallery = page.getByRole('region', { name: 'Fotografie závady', exact: true })
  await expect(gallery.getByRole('img')).toHaveCount(2)
  for (const position of [1, 2]) {
    const photo = gallery.locator(`img[src="/api/v1/reports/${report.id}/photos/${position}"]`)
    await expect(photo).toBeVisible()
    await expect.poll(() => photo.evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0)
    await expect(photo.locator('..')).toHaveAttribute('href', `/api/v1/reports/${report.id}/photos/${position}`)
  }
  expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(0)
  await page.getByRole('main').screenshot({ path: testInfo.outputPath('report-public-detail.png') })

  await page.goto('/hlaseni-zavad')
  await expect(page.locator('#report-form')).toHaveCount(0)
  const detailLink = page.getByRole('main').locator(`a[href="/hlaseni-zavad/${report.id}"]`).first()
  await expect(detailLink).toBeVisible()
  const card = page.locator('.report-card').filter({ has: page.locator(`a[href="/hlaseni-zavad/${report.id}"]`) })
  await expect(card.getByRole('heading')).toHaveText(`${firstSentence}...`)
  await expect(card).not.toContainText(nextSentence)
  await detailLink.click()
  await expect(page.getByRole('main')).toContainText(description)
  await expect(page.getByRole('main').locator('time').first()).toHaveAttribute('datetime', report.created_at)
  expect(errors).toEqual([])
})

test('desktop pin selection fills exact coordinates and a narrow desktop does not request phone location', async ({ page }, testInfo) => {
  await prepareMap(page)
  await countLocationRequests(page)
  await page.goto('/hlaseni-zavad/nove')
  const map = page.locator('#report-map')
  await expect(map.locator('.leaflet-control-attribution')).toContainText('OpenStreetMap')
  const latitude = page.locator('#report-latitude')
  const longitude = page.locator('#report-longitude')
  await expect(latitude).toHaveValue('')
  await expect(longitude).toHaveValue('')
  await map.click({ position: { x: 180, y: 140 } })
  await expect(latitude).not.toHaveValue('')
  await expect(longitude).not.toHaveValue('')
  await expect(page.locator('#report-address')).toHaveValue(`${await latitude.inputValue()}, ${await longitude.inputValue()}`)
  await expect(map.locator('.leaflet-marker-icon')).toHaveCount(1)
  await page.getByRole('button', { name: 'Odebrat špendlík', exact: true }).click()
  await expect(latitude).toHaveValue('')
  await expect(longitude).toHaveValue('')
  await expect(page.locator('#report-address')).toHaveValue('')
  await page.getByRole('button', { name: 'Umístit špendlík do středu mapy', exact: true }).click()
  await expect(latitude).not.toHaveValue('')
  const selectedLatitude = Number(await latitude.inputValue())
  const selectedLongitude = Number(await longitude.inputValue())
  expect(selectedLatitude).toBeCloseTo(50.5295088, 5)
  expect(selectedLongitude).toBeCloseTo(15.158555, 5)
  await page.setViewportSize({ width: 375, height: 812 })
  await expect(page.getByRole('button', { name: 'Použít moji polohu', exact: true })).toBeHidden()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.locator('#report-form').screenshot({ path: testInfo.outputPath('report-form-narrow-desktop.png') })
  await page.getByLabel('Popis problému', { exact: false }).fill(`Poškozené zábradlí ${Date.now()}`)
  await page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true }).fill('Jan')
  const report = await submitReport(page)
  expect(report.address).toBe(`${selectedLatitude.toFixed(6)}, ${selectedLongitude.toFixed(6)}`)
  expect(report.reporter_name).toBe('Jan')
  expect(report.latitude).toBeCloseTo(selectedLatitude, 6)
  expect(report.longitude).toBeCloseTo(selectedLongitude, 6)
  expect(report.has_photo).toBe(false)
  expect(report.photo_count).toBe(0)
  await expect(page.locator('#report-detail-map .leaflet-marker-icon')).toHaveCount(1)
  expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(0)
})

test('a phone requests its location only after the resident presses the location button', async ({ browser }, testInfo) => {
  const context = await browser.newContext({
    ...devices['iPhone 13'],
    baseURL: 'http://127.0.0.1:3107',
    geolocation: { latitude: 50.534123, longitude: 15.162456 },
    permissions: ['geolocation'],
  })
  const page = await context.newPage()
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  try {
    await prepareMap(page)
    await countLocationRequests(page)
    const response = await page.goto('/hlaseni-zavad/nove')
    expect(response?.headers()['permissions-policy']).toBe('camera=(), microphone=(), geolocation=(self)')
    const location = page.getByRole('button', { name: 'Použít moji polohu', exact: true })
    await expect(location).toBeVisible()
    await expect(page.locator('#report-map .leaflet-control-attribution')).toContainText('OpenStreetMap')
    expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(0)
    await page.getByLabel('Popis problému', { exact: false }).fill(`Nesvítící lampa ${Date.now()}`)
    expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(0)
    await location.click()
    await expect(page.locator('#report-latitude')).toHaveValue('50.534123')
    await expect(page.locator('#report-longitude')).toHaveValue('15.162456')
    await expect(page.locator('#report-address')).toHaveValue('50.534123, 15.162456')
    expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(1)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await page.locator('#report-form').screenshot({ path: testInfo.outputPath('report-form-phone.png') })
    const report = await submitReport(page)
    expect(report.latitude).toBe(50.534123)
    expect(report.longitude).toBe(15.162456)
    expect(report.reporter_name).toBeNull()
    expect(report.address).toBe('50.534123, 15.162456')
    expect(errors).toEqual([])
  } finally {
    await context.close()
  }
})

test('denying phone location keeps the form usable with an address', async ({ browser }) => {
  const context = await browser.newContext({
    ...devices['iPhone 13'],
    baseURL: 'http://127.0.0.1:3107',
  })
  const page = await context.newPage()
  try {
    await prepareMap(page)
    await page.addInitScript(() => {
      Object.defineProperty(window, 'reportGeolocationCalls', { value: 0, writable: true })
      navigator.geolocation.getCurrentPosition = (_success, failure) => {
        Reflect.set(window, 'reportGeolocationCalls', Number(Reflect.get(window, 'reportGeolocationCalls')) + 1)
        queueMicrotask(() => failure?.({
          code: 1, message: 'User denied geolocation',
          PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3,
        }))
      }
    })
    await page.goto('/hlaseni-zavad/nove')
    const location = page.getByRole('button', { name: 'Použít moji polohu', exact: true })
    await expect(location).toBeVisible()
    expect(await page.evaluate(() => Reflect.get(window, 'reportGeolocationCalls'))).toBe(0)
    await location.click()
    await expect(page.locator('#report-location-status')).toContainText('Přístup k poloze nebyl povolen.')
    await expect(location).toBeEnabled()
    await expect(page.locator('#report-latitude')).toHaveValue('')
    await expect(page.locator('#report-longitude')).toHaveValue('')
    await page.getByLabel('Popis problému', { exact: false }).fill(`Prasklá dlažba ${Date.now()}`)
    await page.getByLabel('Adresa nebo upřesnění místa', { exact: false }).fill('Chodník u obecního úřadu')
    const report = await submitReport(page)
    expect(report.address).toBe('Chodník u obecního úřadu')
    expect(report.latitude).toBeNull()
    expect(report.longitude).toBeNull()
  } finally {
    await context.close()
  }
})

test('individual photos can be removed and a failed submission retains every remaining photo and form field', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await prepareMap(page)
  await page.goto('/hlaseni-zavad/nove')
  const description = `Uvolněný poklop ${Date.now()}`
  await page.getByLabel('Popis problému', { exact: false }).fill(description)
  await page.getByLabel('Adresa nebo upřesnění místa', { exact: false }).fill('Před domem č. p. 12')
  await page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true }).fill('Alena')
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles({
    name: 'nepodporovany.svg', mimeType: 'image/svg+xml', buffer: Buffer.from('<svg/>'),
  })
  await expect(page.locator('#report-photo-error')).toContainText('JPG, PNG nebo WebP')
  await expect(page.locator('[data-report-photo]')).toHaveCount(0)
  expect(await page.locator('#report-photo').evaluate(input => (input as HTMLInputElement).files?.length)).toBe(0)
  expect(await page.locator('#report-photo').evaluate(input => (input as HTMLInputElement).checkValidity())).toBe(true)
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles([
    { name: 'poklop.png', mimeType: 'image/png', buffer: photograph },
    { name: 'odebrat.png', mimeType: 'image/png', buffer: photograph },
  ])
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles({ name: 'poklop-detail.png', mimeType: 'image/png', buffer: photograph })
  await expect(page.locator('[data-report-photo]')).toHaveCount(3)
  await page.getByRole('button', { name: 'Odebrat fotografii odebrat.png', exact: true }).click()
  await expect(page.locator('[data-report-photo]')).toHaveCount(2)
  await expect(page.getByRole('button', { name: 'Odebrat fotografii odebrat.png', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Umístit špendlík do středu mapy', exact: true }).click()
  const latitude = await page.locator('#report-latitude').inputValue()
  const longitude = await page.locator('#report-longitude').inputValue()
  let releaseFailure = () => {}
  const failureReady = new Promise<void>(resolve => { releaseFailure = resolve })
  await page.route('**/api/v1/reports', async route => {
    await failureReady
    await route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'Server nyní nemůže uložit hlášení.' }),
    })
  }, { times: 1 })
  await page.getByRole('button', { name: 'Odeslat hlášení', exact: true }).click()
  await expect(page.getByLabel('Popis problému', { exact: false })).toBeDisabled()
  await expect(page.locator('#report-map .leaflet-marker-icon')).not.toHaveClass(/leaflet-marker-draggable/)
  releaseFailure()
  await expect(page.locator('#report-submit-error')).toContainText('Server nyní nemůže uložit hlášení.')
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue(description)
  await expect(page.getByLabel('Adresa nebo upřesnění místa', { exact: false })).toHaveValue(`${latitude}, ${longitude}`)
  await expect(page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true })).toHaveValue('Alena')
  await expect(page.locator('#report-latitude')).toHaveValue(latitude)
  await expect(page.locator('#report-longitude')).toHaveValue(longitude)
  await expect(page.locator('#report-photo-preview')).toBeVisible()
  await expect(page.locator('[data-report-photo]')).toHaveCount(2)
  await expect(page.getByRole('button', { name: 'Odebrat fotografii poklop.png', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Odebrat fotografii poklop-detail.png', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Odeslat hlášení', exact: true })).toBeEnabled()
  await expect(page.locator('#report-map .leaflet-marker-icon')).toHaveClass(/leaflet-marker-draggable/)
  const report = await submitReport(page)
  expect(report.description).toBe(description)
  expect(report.reporter_name).toBe('Alena')
  expect(report.has_photo).toBe(true)
  expect(report.photo_count).toBe(2)
  expect(report.latitude).toBeCloseTo(Number(latitude), 6)
  expect(report.longitude).toBeCloseTo(Number(longitude), 6)
  expect(errors).toEqual([])
})

test('photo selection accepts the exact byte limits and rejects larger additions without losing existing files', async ({ page }) => {
  await prepareMap(page)
  await page.goto('/hlaseni-zavad/nove')
  await expect(page.getByRole('button', { name: 'Odeslat hlášení', exact: true })).toBeEnabled()
  expect(await page.locator('#report-photo').evaluate(input => (input as HTMLInputElement).multiple)).toBe(true)
  const photos = page.locator('[data-report-photo]')
  const error = page.locator('#report-photo-error')
  await selectSizedPhotos(page, [{ name: 'presne15.png', size: 15_000_000 }])
  await expect(photos).toHaveCount(1)
  await expect(error).toBeHidden()
  await selectSizedPhotos(page, [{ name: 'prilis-velka.png', size: 15_000_001 }])
  await expect(error).toContainText(/15\s*MB/)
  await expect(photos).toHaveCount(1)
  await expect(page.getByRole('button', { name: 'Odebrat fotografii presne15.png', exact: true })).toBeVisible()
  await selectSizedPhotos(page, [
    { name: 'druha15.png', size: 15_000_000 },
    { name: 'treti15.png', size: 15_000_000 },
  ])
  await expect(photos).toHaveCount(3)
  await expect(error).toBeHidden()
  await selectSizedPhotos(page, [{ name: 'celkem50plus1.png', size: 5_000_001 }])
  await expect(error).toContainText(/50\s*MB/)
  await expect(photos).toHaveCount(3)
  await selectSizedPhotos(page, [{ name: 'celkem50.png', size: 5_000_000 }])
  await expect(photos).toHaveCount(4)
  await expect(error).toBeHidden()
  await page.getByRole('button', { name: 'Odebrat fotografii druha15.png', exact: true }).click()
  await expect(photos).toHaveCount(3)
  await selectSizedPhotos(page, [{ name: 'nahradni15.png', size: 15_000_000 }])
  await expect(photos).toHaveCount(4)
  await expect(error).toBeHidden()
})

test('address suggestions support keyboard selection and moving the pin replaces the address with exact coordinates', async ({ page }, testInfo) => {
  await prepareMap(page)
  const suggestion = { label: 'Vyskeř 50, 512 64 Vyskeř', latitude: 50.5295088, longitude: 15.158555 }
  await page.route('**/api/v1/report-addresses?*', route => route.fulfill({ json: [suggestion] }))
  await page.goto('/hlaseni-zavad/nove')
  const address = page.getByRole('combobox', { name: 'Adresa nebo upřesnění místa', exact: true })
  await expect(address).toHaveAttribute('placeholder', suggestion.label)
  await address.fill('Vyskeř 50')
  await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toBeVisible()
  await expect(address).toHaveAttribute('aria-expanded', 'true')
  await address.press('ArrowDown')
  await expect(page.getByRole('option', { name: suggestion.label, exact: true })).toHaveAttribute('aria-selected', 'true')
  await address.press('Enter')
  await expect(address).toHaveValue(suggestion.label)
  await expect(address).toHaveAttribute('aria-expanded', 'false')
  await expect(page.locator('#report-latitude')).toHaveValue('50.529509')
  await expect(page.locator('#report-longitude')).toHaveValue('15.158555')
  const marker = page.locator('#report-map .leaflet-marker-icon')
  await expect(marker).toHaveCount(1)
  await marker.scrollIntoViewIfNeeded()
  const box = await marker.boundingBox()
  expect(box).not.toBeNull()
  await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2)
  await page.mouse.down()
  await page.mouse.move(box!.x + box!.width / 2 + 45, box!.y + box!.height / 2 + 30, { steps: 8 })
  await page.mouse.up()
  await expect(page.locator('#report-latitude')).not.toHaveValue('50.529509')
  await expect(page.locator('#report-longitude')).not.toHaveValue('15.158555')
  await expect(address).toHaveValue(`${await page.locator('#report-latitude').inputValue()}, ${await page.locator('#report-longitude').inputValue()}`)
  await page.locator('#report-form').screenshot({ path: testInfo.outputPath('report-address-suggestion-pin.png') })
})

test('typed GPS coordinates place a pin without an address search and clearing the pin removes the coordinates', async ({ page }) => {
  await prepareMap(page)
  const searches: string[] = []
  page.on('request', request => {
    if (new URL(request.url()).pathname === '/api/v1/report-addresses') searches.push(request.url())
  })
  await page.goto('/hlaseni-zavad/nove')
  const address = page.getByRole('combobox', { name: 'Adresa nebo upřesnění místa', exact: true })
  for (const value of ['50.531234, 15.162345', '50.531234 15.162345', '50,531234; 15,162345']) {
    await address.fill(value)
    await expect(page.locator('#report-latitude')).toHaveValue('50.531234')
    await expect(page.locator('#report-longitude')).toHaveValue('15.162345')
    await address.press('Tab')
    await expect(address).toHaveValue('50.531234, 15.162345')
    await expect(page.locator('#report-map .leaflet-marker-icon')).toHaveCount(1)
  }
  await address.fill('50.529509, 15.158555')
  // Let the normal debounce interval expire while the coordinate input stays focused.
  const addressRequest = await page.waitForRequest(request => new URL(request.url()).pathname === '/api/v1/report-addresses', { timeout: 750 }).catch(() => null)
  expect(addressRequest).toBeNull()
  expect(searches).toEqual([])
  await page.getByRole('button', { name: 'Odebrat špendlík', exact: true }).click()
  await expect(address).toHaveValue('')
  await expect(page.locator('#report-latitude')).toHaveValue('')
  await expect(page.locator('#report-longitude')).toHaveValue('')
  await expect(page.locator('#report-map .leaflet-marker-icon')).toHaveCount(0)
  await address.fill('91.000000, 15.158555')
  expect(await address.evaluate(input => (input as HTMLInputElement).validity.customError)).toBe(true)
  await expect(page.locator('#report-latitude')).toHaveValue('')
  await expect(page.locator('#report-longitude')).toHaveValue('')
  await address.fill('50.529509, 15.158555')
  expect(await address.evaluate(input => (input as HTMLInputElement).validity.customError)).toBe(false)
  await expect(page.locator('#report-latitude')).toHaveValue('50.529509')
  await expect(page.locator('#report-longitude')).toHaveValue('15.158555')
})

test('cancelling departure preserves a draft and leaving an empty form needs no confirmation', async ({ page }) => {
  await prepareMap(page)
  const dialogs: string[] = []
  let allowDeparture = false
  page.on('dialog', async dialog => {
    dialogs.push(dialog.type())
    if (dialog.type() === 'confirm') expect(dialog.message()).toContain('Máte rozpracované hlášení.')
    if (allowDeparture) await dialog.accept()
    else await dialog.dismiss()
  })
  await page.goto('/hlaseni-zavad/nove')
  const back = page.getByRole('main').getByRole('link', { name: 'Nahlášené závady', exact: true })
  await back.click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  expect(dialogs).toEqual([])
  await page.getByRole('main').getByRole('link', { name: 'Nahlásit závadu', exact: true }).click()
  await page.getByLabel('Popis problému', { exact: false }).fill('Rozpracovaná závada před odesláním')
  await page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true }).fill('Marie')
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles({ name: 'rozpracovana.png', mimeType: 'image/png', buffer: photograph })
  await page.getByRole('button', { name: 'Umístit špendlík do středu mapy', exact: true }).click()
  const coordinates = await page.locator('#report-address').inputValue()
  await back.click()
  expect(dialogs).toEqual(['confirm'])
  await expect(page).toHaveURL(/\/hlaseni-zavad\/nove$/)
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue('Rozpracovaná závada před odesláním')
  await expect(page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true })).toHaveValue('Marie')
  await expect(page.locator('#report-address')).toHaveValue(coordinates)
  await expect(page.locator('[data-report-photo]')).toHaveCount(1)
  await expect(page.getByRole('button', { name: 'Odebrat fotografii rozpracovana.png', exact: true })).toBeVisible()
  // A cancelled reload never reaches Playwright's navigation completion event.
  await page.evaluate(() => window.location.reload())
  await expect.poll(() => dialogs).toEqual(['confirm', 'beforeunload'])
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue('Rozpracovaná závada před odesláním')
  await expect(page.locator('[data-report-photo]')).toHaveCount(1)
  allowDeparture = true
  await back.click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  expect(dialogs).toEqual(['confirm', 'beforeunload', 'confirm'])
  await page.getByRole('main').getByRole('link', { name: 'Nahlásit závadu', exact: true }).click()
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue('')
  await expect(page.locator('[data-report-photo]')).toHaveCount(0)
})

test('a draft survives browser Back and Forward within the application including its pin and photographs', async ({ page }) => {
  await prepareMap(page)
  const dialogs: string[] = []
  page.on('dialog', async dialog => {
    dialogs.push(dialog.type())
    await dialog.dismiss()
  })
  // Establish hydrated in-app history before testing popstate navigation.
  await page.goto('/hlaseni-zavad/nove')
  await expect(page.getByRole('button', { name: 'Odeslat hlášení', exact: true })).toBeEnabled()
  await page.getByRole('main').getByRole('link', { name: 'Nahlášené závady', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  await page.getByRole('main').getByRole('link', { name: 'Nahlásit závadu', exact: true }).click()
  await page.getByLabel('Popis problému', { exact: false }).fill('Nedokončené hlášení po návratu')
  await page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true }).fill('Eva')
  await page.getByLabel('Fotografie (nepovinné)', { exact: true }).setInputFiles({ name: 'zachovana.png', mimeType: 'image/png', buffer: photograph })
  await page.getByRole('button', { name: 'Umístit špendlík do středu mapy', exact: true }).click()
  const latitude = await page.locator('#report-latitude').inputValue()
  const longitude = await page.locator('#report-longitude').inputValue()
  await page.goBack()
  await expect(page.getByRole('heading', { name: 'Nahlášené závady', exact: true })).toBeVisible()
  await expect(page.locator('#report-form')).toHaveCount(0)
  await page.goForward()
  await expect(page).toHaveURL(/\/hlaseni-zavad\/nove$/)
  await expect(page.locator('#report-draft-status')).toHaveAttribute('role', 'status')
  await expect(page.locator('#report-draft-status')).toHaveText('Rozpracované hlášení bylo obnoveno.')
  await expect(page.locator('#report-draft-status')).toBeVisible()
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue('Nedokončené hlášení po návratu')
  await expect(page.getByLabel('Kdo nahlásil (nepovinné)', { exact: true })).toHaveValue('Eva')
  await expect(page.locator('#report-address')).toHaveValue(`${latitude}, ${longitude}`)
  await expect(page.locator('#report-latitude')).toHaveValue(latitude)
  await expect(page.locator('#report-longitude')).toHaveValue(longitude)
  await expect(page.locator('#report-map .leaflet-marker-icon')).toHaveCount(1)
  await expect(page.locator('[data-report-photo]')).toHaveCount(1)
  await expect(page.getByRole('button', { name: 'Odebrat fotografii zachovana.png', exact: true })).toBeVisible()
  expect(dialogs).toEqual([])
})

test('late address responses cannot replace a newer search and unavailable search keeps free text usable', async ({ page }) => {
  await prepareMap(page)
  // Simulate a response already in flight when cancellation arrives.
  await page.addInitScript(() => {
    const original = window.fetch.bind(window)
    window.fetch = (input, init) => String(input).includes('/api/v1/report-addresses?')
      ? original(input, { ...init, signal: undefined })
      : original(input, init)
  })
  let releaseOld = () => {}
  const oldReady = new Promise<void>(resolve => { releaseOld = resolve })
  await page.route('**/api/v1/report-addresses?*', async route => {
    const q = new URL(route.request().url()).searchParams.get('q')
    if (q === 'Stará adresa') {
      await oldReady
      await route.fulfill({ json: [{ label: 'Stará odpověď', latitude: 50.1, longitude: 15.1 }] })
    } else if (q === 'Nová adresa') {
      await route.fulfill({ json: [{ label: 'Nová odpověď', latitude: 50.2, longitude: 15.2 }] })
    } else {
      await route.fulfill({ status: 503, json: { error: 'Vyhledávání adres nyní není dostupné.' } })
    }
  })
  await page.goto('/hlaseni-zavad/nove')
  const address = page.getByRole('combobox', { name: 'Adresa nebo upřesnění místa', exact: true })
  const oldRequest = page.waitForRequest(request => new URL(request.url()).searchParams.get('q') === 'Stará adresa')
  await address.fill('Stará adresa')
  await oldRequest
  await address.fill('Nová adresa')
  await expect(page.getByRole('option', { name: 'Nová odpověď', exact: true })).toBeVisible()
  const oldResponse = page.waitForResponse(response => new URL(response.url()).searchParams.get('q') === 'Stará adresa')
  releaseOld()
  await (await oldResponse).finished()
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))))
  await expect(page.getByRole('option', { name: 'Stará odpověď', exact: true })).toHaveCount(0)
  await expect(page.getByRole('option', { name: 'Nová odpověď', exact: true })).toBeVisible()
  await address.press('Escape')
  await expect(address).toHaveAttribute('aria-expanded', 'false')
  await address.fill('Před domem č. p. 12')
  await expect(page.locator('#report-address-status')).toContainText(/nedostupné|nepodařilo|není dostupné/)
  await expect(address).toHaveValue('Před domem č. p. 12')
  await expect(page.locator('#report-latitude')).toHaveValue('')
  await page.getByLabel('Popis problému', { exact: false }).fill(`Závada při výpadku adres ${Date.now()}`)
  const report = await submitReport(page)
  expect(report.address).toBe('Před domem č. p. 12')
  expect(report.latitude).toBeNull()
  expect(report.longitude).toBeNull()
})

test('search text entered before hydration is preserved when the application becomes interactive', async ({ page }) => {
  await prepareMap(page)
  let releaseHydration = () => {}
  const hydrationReady = new Promise<void>(resolve => { releaseHydration = resolve })
  await page.route(/\/pkg\/[^/]+\.wasm$/, async route => {
    await hydrationReady
    await route.continue()
  })
  await page.goto('/hlaseni-zavad', { waitUntil: 'commit' })
  const query = page.getByRole('searchbox', { name: 'Hledat v hlášeních', exact: true })
  await expect(page.locator('#mobile-menu-toggle')).toBeDisabled()
  await query.fill('ZachovaneVyhledavani')
  releaseHydration()
  await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
  await expect(query).toHaveValue('ZachovaneVyhledavani')
  await page.getByRole('button', { name: 'Hledat', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad\?q=ZachovaneVyhledavani$/)
  await expect(query).toHaveValue('ZachovaneVyhledavani')
  await page.getByRole('link', { name: 'Zrušit hledání', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  await expect(query).toHaveValue('')
})

test('map zoom controls keep a filled form open without a departure dialog', async ({ page }) => {
  await prepareMap(page)
  const dialogs: string[] = []
  page.on('dialog', async dialog => { dialogs.push(dialog.type()); await dialog.dismiss() })
  await page.goto('/hlaseni-zavad/nove')
  await page.getByLabel('Popis problému', { exact: false }).fill('Rozpracovaný popis při přibližování mapy')
  await page.getByRole('button', { name: 'Umístit špendlík do středu mapy', exact: true }).click()
  const location = await page.locator('#report-address').inputValue()
  const map = page.locator('#report-map')
  await map.locator('.leaflet-control-zoom-in').click()
  await expect.poll(() => map.locator('img.leaflet-tile[src*="/18/"]').count()).toBeGreaterThan(0)
  await expect(map.locator('.leaflet-map-pane')).not.toHaveClass(/\bleaflet-zoom-anim\b/)
  await map.locator('.leaflet-control-zoom-out').click()
  await expect(map.locator('.leaflet-map-pane')).not.toHaveClass(/\bleaflet-zoom-anim\b/)
  await expect(map.locator('img.leaflet-tile[src*="/18/"]')).toHaveCount(0)
  await expect.poll(() => map.locator('img.leaflet-tile[src*="/17/"]').count()).toBeGreaterThan(0)
  await expect(page.getByLabel('Popis problému', { exact: false })).toHaveValue('Rozpracovaný popis při přibližování mapy')
  await expect(page.locator('#report-address')).toHaveValue(location)
  expect(dialogs).toEqual([])
})

test('the overview map follows the list query and overlapping reports open separate public details on mobile', async ({ page }, testInfo) => {
  await prepareMap(page)
  const token = `Mapa${Date.now()}`
  const created = []
  for (const [description, located] of [[`${token} první závada s uvolněnými prkny a dalším poškozením kolem lavičky u obecního úřadu.`, true], [`${token} druhá závada`, true], [`${token} bez polohy`, false], [`MimoFiltr${Date.now()}`, true]] as const) {
    const response = await page.request.post('/api/v1/reports', {
      headers: { Origin: 'http://127.0.0.1:3107' },
      multipart: { description, address: 'Vyskeř 50', ...(located ? { latitude: '50.529509', longitude: '15.158555' } : {}), ...(created.length === 0 ? { photo: { name: 'mapa.png', mimeType: 'image/png', buffer: photograph } } : {}) },
    })
    expect(response.status()).toBe(201)
    created.push(await response.json())
  }
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto(`/hlaseni-zavad?q=${token}`)
  const map = page.locator('#reports-overview-map')
  await expect(map).toHaveAttribute('data-loaded', 'true')
  await expect(map).toHaveAttribute('data-point-count', '2')
  await expect(page.locator('.report-card')).toHaveCount(3)
  await expect(page.locator('#reports-map-summary')).toContainText('Bez vyznačené polohy: 1')
  await expect(map.locator('.report-map-cluster')).toHaveCount(1)
  await map.locator('.report-map-cluster').click()
  await expect(map.locator('img.leaflet-marker-icon')).toHaveCount(2)
  const marker = map.locator(`img.leaflet-marker-icon[alt^="Hlášení č. ${created[0].id}:"]`)
  await marker.click()
  const popup = map.locator(`.report-map-popup[data-report-id="${created[0].id}"]`)
  await expect(map.locator('.leaflet-popup-close-button')).toHaveAttribute('aria-label', 'Zavřít náhled')
  await expect.poll(async () => {
    const bounds = await map.boundingBox()
    const close = await map.locator('.leaflet-popup-close-button').boundingBox()
    return Boolean(bounds && close && close.x >= bounds.x && close.y >= bounds.y
      && close.x + close.width <= bounds.x + bounds.width && close.y + close.height <= bounds.y + bounds.height)
  }).toBe(true)
  await expect(popup).toContainText(`${token} první závada`)
  await expect(popup.locator('time')).toHaveAttribute('datetime', created[0].created_at)
  await expect(popup.getByRole('link', { name: 'Zobrazit detail', exact: true })).toHaveAttribute('href', `/hlaseni-zavad/${created[0].id}`)
  await expect(popup.getByRole('link', { name: 'Zobrazit detail', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await map.screenshot({ path: testInfo.outputPath('reports-overview-phone.png') })
  await popup.getByRole('link', { name: 'Zobrazit detail', exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`/hlaseni-zavad/${created[0].id}$`))
  await expect(page.getByRole('main')).toContainText(created[0].description)
  await page.goBack()
  await page.getByRole('searchbox', { name: 'Hledat v hlášeních', exact: true }).fill(`Nenalezena${token}`)
  await page.getByRole('button', { name: 'Hledat', exact: true }).click()
  await expect(map).toHaveAttribute('data-point-count', '0')
  await expect(map.locator('img.leaflet-marker-icon, .report-map-cluster')).toHaveCount(0)
  await expect(page.locator('.report-card')).toHaveCount(0)
})

test('residents search the public list by description or address, see no-match feedback and reset the search', async ({ page }) => {
  await prepareMap(page)
  const token = `Vyhledatelna${Date.now()}`
  const description = `Poškozená lavička ${token}`
  const addressText = `Adresa-${token}`
  await page.goto('/hlaseni-zavad/nove')
  await page.getByLabel('Popis problému', { exact: false }).fill(description)
  await page.getByLabel('Adresa nebo upřesnění místa', { exact: false }).fill(addressText)
  const report = await submitReport(page)
  await page.goto('/hlaseni-zavad')
  const query = page.getByRole('searchbox', { name: 'Hledat v hlášeních', exact: true })
  const search = page.getByRole('button', { name: 'Hledat', exact: true })
  await query.fill(token)
  await search.click()
  await expect(page).toHaveURL(new RegExp(`/hlaseni-zavad\\?q=${token}$`))
  await expect(page.locator('.report-card')).toHaveCount(1)
  await expect(page.locator('.report-card')).toContainText(description)
  await page.reload()
  await expect(query).toHaveValue(token)
  await expect(page.locator('.report-card')).toHaveCount(1)
  await query.fill(addressText)
  await search.click()
  await expect(page.locator('.report-card')).toHaveCount(1)
  await expect(page.locator('.report-card')).toContainText(description)
  await page.locator(`.report-card a[href="/hlaseni-zavad/${report.id}"]`).first().click()
  await expect(page).toHaveURL(new RegExp(`/hlaseni-zavad/${report.id}$`))
  await page.goBack()
  await query.fill(`Nenalezeno-${token}`)
  await search.click()
  await expect(page.getByRole('main')).toContainText('Hledání neodpovídá žádné hlášení.')
  await expect(page.locator('.report-card')).toHaveCount(0)
  await page.getByRole('link', { name: 'Zrušit hledání', exact: true }).click()
  await expect(page).toHaveURL(/\/hlaseni-zavad$/)
  await expect(query).toHaveValue('')
  await expect(page.getByRole('main')).toContainText(description)
  await expect(page.locator('#report-form')).toHaveCount(0)
})
