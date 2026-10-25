import { test, expect, type Page } from './fixtures'

const PASSWORD = 'Disposable-browser-test-password!'
const photograph = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg==', 'base64')
const report = {
  id: 41,
  description: 'Uvolněné prkno na lavičce u zastávky.',
  address: 'Vyskeř, autobusová zastávka',
  latitude: 50.532,
  longitude: 15.161,
  reporter_name: null,
  created_at: '2026-10-25T10:20:00+01:00',
  status: 'reported',
  has_photo: true,
  photo_count: 3,
  request_ip: '192.0.2.15',
  hidden: false,
}

async function openReports(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  await page.getByRole('link', { name: 'Hlášení závad', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Hlášení závad', exact: true })).toBeVisible()
}

test('report recipients start empty, preserve failed edits, and can be cleared', async ({ page }) => {
  const errors: string[] = []
  const saves: any[] = []
  let recipients: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/v1/admin/reports?*', route => route.fulfill({ json: [] }))
  await page.route('**/api/v1/admin/report-settings', async route => {
    if (route.request().method() === 'PUT') {
      expect(route.request().headers()['x-csrf-token']).toBeTruthy()
      const body = route.request().postDataJSON()
      saves.push(body)
      if (body.recipients.includes('neplatna-adresa')) {
        await route.fulfill({ status: 422, json: { error: 'E-mailová adresa není platná.' } })
        return
      }
      recipients = body.recipients
    }
    await route.fulfill({ json: { recipients } })
  })
  await openReports(page)
  const addresses = page.getByLabel('Příjemci upozornění', { exact: true })
  const save = page.getByRole('button', { name: 'Uložit příjemce', exact: true })
  await expect(addresses).toHaveValue('')
  await expect(page.getByText('Bez příjemců se upozornění neposílají', { exact: false })).toBeVisible()
  await expect(save).toBeDisabled()
  await addresses.fill('neplatna-adresa')
  await save.click()
  await expect(page.getByRole('alert')).toContainText('E-mailová adresa není platná.')
  await expect(addresses).toHaveValue('neplatna-adresa')
  await addresses.fill('  opravy@example.test  \n\nobec@example.test')
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('link', { name: 'Přehled', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/hlaseni-zavad$/)
  await save.click()
  await expect(page.locator('.admin-flash')).toContainText('Příjemci upozornění na závady byli uloženi.')
  await expect(addresses).toHaveValue('opravy@example.test\nobec@example.test')
  await expect(save).toBeDisabled()
  await addresses.fill('')
  await save.click()
  await expect(save).toBeDisabled()
  expect(saves).toEqual([
    { recipients: ['neplatna-adresa'] },
    { recipients: ['opravy@example.test', 'obec@example.test'] },
    { recipients: [] },
  ])
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(errors).toEqual([])
})

test('administrators see private report details and can update status and visibility', async ({ page }) => {
  const errors: string[] = []
  const updates: any[] = []
  let current = { ...report }
  let failNextUpdate = false
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/v1/admin/report-settings', route => route.fulfill({ json: { recipients: [] } }))
  await page.route('**/api/v1/admin/reports?*', route => route.fulfill({ json: [current] }))
  await page.route('**/api/v1/admin/reports/41/photos/*', route => route.fulfill({ contentType: 'image/png', body: photograph }))
  await page.route('**/api/v1/admin/reports/41', async route => {
    expect(route.request().headers()['x-csrf-token']).toBeTruthy()
    const body = route.request().postDataJSON()
    updates.push(body)
    if (failNextUpdate) {
      await route.fulfill({ status: 500, json: { error: 'Změnu se nepodařilo uložit.' } })
      failNextUpdate = false
      return
    }
    current = { ...current, ...body }
    await route.fulfill({ json: current })
  })
  await openReports(page)
  await page.getByRole('button', { name: 'Detail #41', exact: true }).click()
  const detail = page.getByRole('region', { name: 'Detail hlášení', exact: true })
  await expect(detail).toContainText('Anonymní hlášení')
  await expect(detail).toContainText('192.0.2.15')
  await expect(detail).toContainText('25. 10. 2026 v 10:20')
  await expect(detail.getByRole('img')).toHaveCount(3)
  for (const position of [1, 2, 3]) {
    const photo = detail.getByRole('link', { name: `Otevřít fotografii ${position} hlášení`, exact: true })
    await expect(photo).toHaveAttribute('href', `/api/v1/admin/reports/41/photos/${position}`)
    await expect(photo.getByRole('img')).toHaveAttribute('src', `/api/v1/admin/reports/41/photos/${position}`)
  }
  await expect(detail.getByRole('link', { name: 'Otevřít místo v OpenStreetMap' })).toHaveAttribute('href', /mlat=50\.532&mlon=15\.161/)
  await detail.getByRole('button', { name: 'Převzít k řešení', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Převzít k řešení', exact: true })).toBeDisabled()
  await detail.getByRole('button', { name: 'Označit jako vyřešené', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Označit jako vyřešené', exact: true })).toBeDisabled()
  await detail.getByRole('button', { name: 'Skrýt na veřejném webu', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true })).toBeEnabled()
  await expect(detail.getByRole('link', { name: 'Otevřít veřejný detail', exact: true })).toHaveCount(0)
  await expect(detail.getByRole('img')).toHaveCount(3)
  failNextUpdate = true
  await detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true }).click()
  await expect(detail.getByRole('alert')).toContainText('Změnu se nepodařilo uložit.')
  await expect(detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true })).toBeEnabled()
  await detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true }).click()
  await expect(detail.getByRole('link', { name: 'Otevřít veřejný detail', exact: true })).toHaveAttribute('href', '/hlaseni-zavad/41')
  // Another administrator hides the report while this detail is still open.
  current.hidden = true
  await detail.getByRole('button', { name: 'Vrátit mezi nahlášené', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true })).toBeEnabled()
  await expect(detail.getByRole('link', { name: 'Otevřít veřejný detail', exact: true })).toHaveCount(0)
  // A visibility-only update must likewise preserve the latest server status.
  current.status = 'resolved'
  await detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Označit jako vyřešené', exact: true })).toBeDisabled()
  await expect(detail.getByRole('link', { name: 'Otevřít veřejný detail', exact: true })).toBeVisible()
  expect(updates).toEqual([
    { status: 'in_progress' },
    { status: 'resolved' },
    { hidden: true },
    { hidden: false },
    { hidden: false },
    { status: 'reported' },
    { hidden: false },
  ])
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(errors).toEqual([])
})

test('an administrator saves a repair comment and photograph, resolves the report and controls public visibility', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('https://tile.openstreetmap.org/**', route => route.fulfill({ contentType: 'image/png', body: photograph }))
  const description = `Opravená lavička ${Date.now()}`
  const created = await page.request.post('/api/v1/reports', {
    headers: { Origin: 'http://127.0.0.1:3107' },
    multipart: { description, address: 'Vyskeř 50', photo: { name: 'puvodni.png', mimeType: 'image/png', buffer: photograph } },
  })
  expect(created.status()).toBe(201)
  const { id } = await created.json()
  await openReports(page)
  await page.getByRole('button', { name: `Detail #${id}`, exact: true }).click()
  const detail = page.getByRole('region', { name: 'Detail hlášení', exact: true })
  const resolution = page.getByRole('region', { name: 'Řešení hlášení', exact: true })
  const comment = 'Uvolněná prkna jsme vyměnili a lavičku znovu natřeli.'
  await resolution.getByLabel('Komentář k řešení (nepovinné)', { exact: true }).fill(comment)
  await resolution.getByLabel('Fotografie řešení (nepovinné)', { exact: true }).setInputFiles([
    { name: 'oprava.png', mimeType: 'image/png', buffer: photograph },
    { name: 'odebrat-opravu.png', mimeType: 'image/png', buffer: photograph },
  ])
  await expect(page.locator('[data-resolution-photo]')).toHaveCount(2)
  await resolution.getByRole('button', { name: 'Odebrat novou fotografii řešení odebrat-opravu.png', exact: true }).click()
  await expect(page.locator('[data-resolution-photo]')).toHaveCount(1)
  let discardDialogs = 0
  page.on('dialog', async dialog => {
    discardDialogs += 1
    expect(dialog.message()).toContain('neuložené změny')
    await dialog.dismiss()
  })
  await detail.getByRole('button', { name: 'Zavřít detail', exact: true }).click()
  await page.getByRole('button', { name: 'Obnovit hlášení', exact: true }).click()
  expect(discardDialogs).toBe(2)
  await expect(resolution.getByLabel('Komentář k řešení (nepovinné)', { exact: true })).toHaveValue(comment)
  await expect(page.locator('[data-resolution-photo]')).toHaveCount(1)
  await page.route(`**/api/v1/admin/reports/${id}/resolution`, route => route.fulfill({ status: 503, json: { error: 'Uložení řešení nyní není dostupné.' } }), { times: 1 })
  const resolve = resolution.getByRole('button', { name: 'Uložit a označit jako vyřešené', exact: true })
  await resolve.click()
  await expect(resolution.getByRole('alert')).toContainText('Uložení řešení nyní není dostupné.')
  await expect(resolution.getByLabel('Komentář k řešení (nepovinné)', { exact: true })).toHaveValue(comment)
  await expect(page.locator('[data-resolution-photo]')).toHaveCount(1)
  const saved = page.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/admin/reports/${id}/resolution` && response.request().method() === 'PUT')
  await resolve.click()
  expect((await saved).status()).toBe(200)
  await expect(page.locator('.admin-flash')).toContainText('Řešení hlášení bylo uloženo.')
  await expect(page.locator('[data-resolution-photo]')).toHaveCount(0)
  await expect(resolution.getByRole('img', { name: 'Fotografie provedené opravy', exact: true })).toHaveCount(1)
  await expect(detail.getByRole('button', { name: 'Označit jako vyřešené', exact: true })).toBeDisabled()
  const persisted = await (await page.request.get(`/api/v1/reports/${id}`)).json()
  expect(persisted.status).toBe('resolved')
  expect(persisted.resolution_comment).toBe(comment)
  expect(persisted.photo_count).toBe(1)
  expect(persisted.resolution_photo_ids).toHaveLength(1)
  expect(persisted).not.toHaveProperty('request_ip')
  const resolutionPhoto = `/api/v1/reports/${id}/resolution-photos/${persisted.resolution_photo_ids[0]}`
  await page.goto(`/hlaseni-zavad/${id}`)
  await expect(page.locator('.report-resolution')).toContainText(comment)
  await expect(page.locator('.report-resolution')).toContainText('Řešení závady')
  await expect(page.getByRole('main').locator('.report-status')).toHaveText('Vyřešeno')
  await expect(page.locator('.report-resolution img')).toHaveAttribute('src', resolutionPhoto)
  await expect.poll(() => page.locator('.report-resolution img').evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0)
  await expect(page.locator(`img[src="/api/v1/reports/${id}/photos/1"]`)).toBeVisible()
  await page.goto('/admin/hlaseni-zavad')
  await page.getByRole('button', { name: `Detail #${id}`, exact: true }).click()
  await detail.getByRole('button', { name: 'Skrýt na veřejném webu', exact: true }).click()
  await expect(detail.getByRole('button', { name: 'Znovu zveřejnit', exact: true })).toBeEnabled()
  expect((await page.request.get(resolutionPhoto)).status()).toBe(404)
  expect((await page.request.get(`/api/v1/reports/${id}/photos/1`)).status()).toBe(404)
  expect((await page.request.get(`/api/v1/admin/reports/${id}/resolution-photos/${persisted.resolution_photo_ids[0]}`)).status()).toBe(200)
  expect(discardDialogs).toBe(2)
  expect(errors).toEqual([])
})
