import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

function dateKey(date: Date) {
  return date.toISOString().slice(0, 10)
}

function shift(date: Date, days: number) {
  return new Date(date.getTime() + days * 86_400_000)
}

// Simulate the passage of time only in the isolated test database.
function finishRental(id: number) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  const samplePhoto = readFileSync('public/images/vysker-znak-app-192.png').toString('base64')
  execFileSync('python3', ['-c', [
    'import base64, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    name = connection.execute("SELECT current_database()").fetchone()[0]',
    '    if not name.startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    photo = base64.b64decode(sys.argv[3])',
    '    connection.execute("UPDATE rentals SET start_date = CURRENT_DATE - 3, end_date = CURRENT_DATE - 1, before_photo = %s, before_type = \'image/png\', after_photo = %s, after_type = \'image/png\', before_photo_taken_at = \'2026-11-01 08:30:00\', after_photo_taken_at = NULL WHERE id = %s", (photo, photo, int(sys.argv[2])))',
  ].join('\n'), database, String(id), samplePhoto])
}

test('admin configures rental, applicant sees availability, and admin settles the request', async ({ page, browser }) => {
  await page.clock.install()
  const errors: string[] = []
  const browserErrors: string[] = []
  let phase = 'initial'
  page.on('pageerror', error => errors.push(`${phase}: ${error.stack || error.message}`))
  page.on('console', message => { if (message.type() === 'error') browserErrors.push(`${phase}: console: ${message.text()}`) })
  await login(page)
  await page.getByRole('link', { name: 'Pronájmy', exact: true }).click()
  await page.getByRole('link', { name: 'Správa majetku a cen', exact: true }).click()
  await page.getByRole('button', { name: 'Přidat majetek', exact: true }).click()
  const assetName = `Zkušební sál ${Date.now()}`
  const form = page.locator('section').filter({ has: page.getByRole('heading', { name: 'Nový majetek', exact: true }) })
  await form.getByLabel('Název', { exact: true }).fill(assetName)
  await form.getByLabel('Popis', { exact: true }).fill('Sál pro ověření rezervace')
  await form.getByLabel('Soukromá akce: cena za hodinu v Kč').fill('100')
  await form.getByLabel('Soukromá akce: cena za den v Kč').fill('250')
  await form.getByLabel('E-mail pro upozornění', { exact: true }).fill('rental-admin@vysker.test')
  await form.getByLabel('Vyžadovat fotky elektroměru').check()
  const saved = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rental-assets') && response.request().method() === 'POST')
  await form.getByRole('button', { name: 'Uložit', exact: true }).click()
  expect((await saved).status()).toBe(200)
  await expect(page.getByRole('heading', { name: assetName, exact: true })).toBeVisible()

  const publicContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const publicPage = await publicContext.newPage()
  publicPage.on('pageerror', error => errors.push(`public: ${error.stack || error.message}`))
  try {
    await publicPage.goto('/pronajmy')
    const select = publicPage.getByLabel('Co si chcete pronajmout')
    await expect(select).toBeEnabled()
    await select.selectOption({ label: assetName })
    await expect(publicPage.locator('#rental-prices')).toContainText('100 Kč')
    await expect(publicPage.locator('#rental-prices')).toContainText('250 Kč')
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()

    const future = shift(new Date(), 14)
    const friday = shift(future, (5 - future.getUTCDay() + 7) % 7)
    const saturday = shift(friday, 1)
    await publicPage.getByLabel('Jméno a příjmení', { exact: true }).fill('Testovací nájemník')
    await publicPage.getByLabel('E-mail', { exact: true }).fill('renter@vysker.test')
    await publicPage.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
    await publicPage.getByLabel('Adresa', { exact: true }).fill('Vyskeř 123')
    await publicPage.getByLabel('Kopie občanského průkazu', { exact: true }).setInputFiles('public/images/vysker-znak-app-192.png')
    await publicPage.getByLabel('Souhlasím s dočasným uložením').check()
    await publicPage.locator('#rental-event-type').selectOption('standard')
    await publicPage.getByLabel('Datum', { exact: true }).fill(dateKey(saturday))
    const requested = publicPage.waitForResponse(response => response.url().endsWith('/api/v1/rentals') && response.request().method() === 'POST')
    await publicPage.getByRole('button', { name: 'Odeslat žádost', exact: true }).click()
    const created = await requested
    expect(created.status()).toBe(201)
    const { id } = await created.json()
    await expect(publicPage.locator('#rental-message')).toContainText('Žádost byla odeslána')

    const targetMonth = new Intl.DateTimeFormat('cs-CZ', { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(saturday)
    if ((await publicPage.locator('#rental-month').textContent()) !== targetMonth) {
      await publicPage.getByRole('button', { name: 'Další měsíc' }).click()
    }
    await expect(publicPage.locator('#rental-month')).toHaveText(targetMonth)
    await expect(publicPage.getByRole('cell', { name: `${saturday.getUTCDate()}. ${targetMonth}, žádost čeká na schválení` })).toBeVisible()
    expect((await publicPage.request.get(`/api/v1/admin/rentals/${id}/identity`)).status()).toBe(405)
    expect((await publicPage.request.post(`/api/v1/admin/rentals/${id}/identity`, { data: { password: 'anything' } })).status()).toBe(401)

    await page.goto('/admin/pronajmy')
    await page.getByLabel('Stav pronájmu').selectOption('pending')
    const row = page.getByRole('row').filter({ hasText: 'Testovací nájemník' }).filter({ hasText: assetName })
    await expect(row).toContainText('250 Kč')
    await row.getByRole('link', { name: 'Testovací nájemník' }).click()
    await expect(page.getByRole('heading', { name: `Pronájem č. ${id}` })).toBeVisible()
    expect((await page.request.get(`/api/v1/admin/rentals/${id}/identity`)).status()).toBe(405)
    expect((await page.request.get(`/api/v1/admin/rentals/${id}/photos/identity`)).status()).toBe(404)
    let identityRequests = 0
    page.on('request', request => {
      if (request.url().endsWith(`/api/v1/admin/rentals/${id}/identity`) && request.method() === 'POST') identityRequests++
    })
    await page.getByRole('button', { name: 'Zobrazit kopii dokladu', exact: true }).click()
    const password = page.getByLabel('Pro zobrazení zadejte znovu Vaše heslo')
    await password.fill('wrong-password')
    const failedIdentity = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/identity`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Ověřit a zobrazit' }).click()
    expect((await failedIdentity).status()).toBe(401)
    await expect(page.locator('[data-identity-error]')).not.toBeEmpty()
    await expect(password).toHaveValue('')
    await page.bringToFront()
    // Pause ahead of the browser clock, avoiding host/browser scheduling skew.
    await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1000))
    await password.fill('Disposable-browser-test-password!')
    const verifiedIdentity = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/identity`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Ověřit a zobrazit' }).click()
    expect((await verifiedIdentity).status()).toBe(200)
    const identityImage = page.getByRole('img', { name: 'Kopie občanského průkazu' })
    await expect(identityImage).toBeVisible()
    await expect.poll(async () => identityImage.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0)
    const identityUrl = await identityImage.getAttribute('src')
    expect(identityUrl).toMatch(/^blob:/)
    const originalImage = await identityImage.elementHandle()
    const enlarge = page.getByRole('button', { name: 'Zvětšit kopii dokladu', exact: true })
    const enlarged = page.getByRole('dialog', { name: 'Zvětšená kopie občanského průkazu', exact: true })
    await enlarge.click()
    await expect(enlarged).toBeVisible()
    await expect(enlarged.getByRole('img', { name: 'Kopie občanského průkazu' })).toBeVisible()
    await expect(page.locator('[data-identity-image]')).toHaveCount(1)
    await expect(identityImage).toHaveAttribute('src', identityUrl!)
    expect(await originalImage!.evaluate(image => image === document.querySelector('[data-identity-image]'))).toBe(true)
    expect(identityRequests).toBe(2)

    await page.clock.fastForward(30_000)
    await enlarged.getByRole('button', { name: 'Zmenšit zobrazení', exact: true }).click()
    await expect(enlarged).toBeHidden()
    await expect(enlarge).toBeFocused()
    await expect(enlarge.getByRole('img', { name: 'Kopie občanského průkazu' })).toBeVisible()
    expect(await originalImage!.evaluate(image => image === document.querySelector('[data-identity-image]'))).toBe(true)
    await enlarge.press('Enter')
    await expect(enlarged).toBeVisible()
    await page.clock.fastForward(30_000)
    await page.keyboard.press('Escape')
    await expect(enlarged).toBeHidden()
    await expect(enlarge).toBeFocused()
    await expect(page.locator('[data-identity-result] [data-identity-timer]')).toContainText('1:00')
    await expect(identityImage).toHaveAttribute('src', identityUrl!)
    expect(identityRequests).toBe(2)
    await enlarge.click()
    await page.clock.fastForward(59_000)
    await expect(enlarged).toBeVisible()
    await expect(identityImage).toBeVisible()
    await page.clock.fastForward(2_000)
    await expect(enlarged).toBeHidden()
    await expect(page.getByRole('img', { name: 'Kopie občanského průkazu', includeHidden: true })).toHaveCount(0)
    await expect(page.locator('[data-identity-image]')).toHaveCount(0)
    expect(await originalImage!.getAttribute('src')).toBeNull()
    await expect(page.locator('[data-identity-status]')).toContainText('vypršelo')
    await expect(page.getByRole('button', { name: 'Zobrazit kopii dokladu', exact: true })).toBeFocused()
    expect(await page.evaluate(async url => {
      try { return (await fetch(url!)).ok } catch { return false }
    }, identityUrl)).toBe(false)
    await page.clock.resume()
    await page.getByRole('button', { name: 'Zobrazit kopii dokladu', exact: true }).click()
    await expect(password).toBeVisible()
    await expect(password).toHaveValue('')
    await password.fill('Disposable-browser-test-password!')
    const reverifiedIdentity = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/identity`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Ověřit a zobrazit' }).click()
    expect((await reverifiedIdentity).status()).toBe(200)
    expect(identityRequests).toBe(3)
    await expect(identityImage).toBeVisible()
    const reopenedUrl = await identityImage.getAttribute('src')
    expect(reopenedUrl).toMatch(/^blob:/)
    expect(reopenedUrl).not.toBe(identityUrl)
    await page.locator('.rental-identity-card').screenshot({ path: 'target/rental-identity-panel-sample.png',
      style: '.site-header, .skip-link { visibility: hidden !important; }' })
    await enlarge.click()
    await expect(enlarged).toBeVisible()
    await enlarged.screenshot({ path: 'target/rental-identity-enlarged-sample.png' })
    await enlarged.getByRole('button', { name: 'Zavřít kopii dokladu', exact: true }).click()
    await expect(enlarged).toBeHidden()
    await expect(page.getByRole('img', { name: 'Kopie občanského průkazu', includeHidden: true })).toHaveCount(0)
    expect(await page.evaluate(async url => {
      try { return (await fetch(url!)).ok } catch { return false }
    }, reopenedUrl)).toBe(false)
    await page.getByRole('button', { name: 'Zobrazit kopii dokladu', exact: true }).click()
    await expect(password).toBeVisible()
    await expect(password).toHaveValue('')
    await password.fill('Disposable-browser-test-password!')
    const thirdIdentity = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/identity`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Ověřit a zobrazit' }).click()
    expect((await thirdIdentity).status()).toBe(200)
    expect(identityRequests).toBe(4)
    await expect(identityImage).toBeVisible()
    const leavingUrl = await identityImage.getAttribute('src')
    expect(leavingUrl).toMatch(/^blob:/)
    await enlarge.click()
    await expect(enlarged).toBeVisible()
    await page.goBack()
    await expect(page).toHaveURL(/\/admin\/pronajmy$/)
    await expect(page.locator('[data-identity-dialog]')).toHaveCount(0)
    await expect(page.locator('[data-identity-image]')).toHaveCount(0)
    await expect.poll(async () => page.evaluate(async url => {
      try { return (await fetch(url!)).ok } catch { return false }
    }, leavingUrl)).toBe(false)
    await page.goto(`/admin/pronajmy/${id}`)
    await expect(page.getByRole('heading', { name: `Pronájem č. ${id}` })).toBeVisible()
    let approvalRequests = 0
    page.on('request', request => {
      if (request.url().endsWith(`/api/v1/admin/rentals/${id}/approve`) && request.method() === 'POST') approvalRequests++
    })
    await page.getByRole('button', { name: 'Schválit žádost' }).click()
    const approvalDialog = page.getByRole('dialog', { name: 'Schválit žádost?' })
    await expect(approvalDialog).toBeVisible()
    await expect(approvalDialog).toContainText('Testovací nájemník')
    await expect(approvalDialog.getByRole('button', { name: 'Zrušit' })).toBeFocused()
    await approvalDialog.getByRole('button', { name: 'Zrušit' }).click()
    await expect(approvalDialog).toHaveCount(0)
    expect(approvalRequests).toBe(0)
    expect((await (await page.request.get(`/api/v1/admin/rentals/${id}`)).json()).status).toBe('pending')
    await page.getByRole('button', { name: 'Schválit žádost' }).click()
    const approved = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/approve`) && response.request().method() === 'POST')
    await page.getByRole('dialog', { name: 'Schválit žádost?' }).getByRole('button', { name: 'Schválit žádost' }).click()
    expect((await approved).status()).toBe(200)
    await expect(page.getByRole('heading', { name: 'Připravit vyúčtování' })).toBeVisible()
    await expect(page.locator('[data-invoice-available]')).toContainText('v 00:00')
    await expect(page.locator('[data-invoice-missing]')).toContainText('před pronájmem i po něm')
    await expect(page.getByRole('button', { name: 'Odeslat vyúčtování' })).toBeDisabled()

    finishRental(id)
    await page.reload()
    await expect(page.getByText('Před pronájmem, orientační čas pořízení: 2026-11-01 08:30:00')).toBeVisible()
    await expect(page.getByText('Po pronájmu, orientační čas pořízení: není k dispozici')).toBeVisible()
    const pagesBeforePhoto = page.context().pages().length
    const beforePhoto = page.getByRole('button', { name: 'Zobrazit fotografii před pronájmem', exact: true })
    const photoResponse = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/photos/before`))
    await beforePhoto.click()
    expect((await photoResponse).status()).toBe(200)
    expect((await photoResponse).headers()['cache-control']).toContain('no-store')
    const meterDialog = page.getByRole('dialog', { name: 'Elektroměr před pronájmem', exact: true })
    await expect(meterDialog).toBeVisible()
    const meterImage = meterDialog.getByRole('img', { name: 'Elektroměr před pronájmem', exact: true })
    await expect.poll(() => meterImage.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0)
    const meterUrl = await meterImage.getAttribute('src')
    expect(meterUrl).toMatch(/^blob:/)
    expect(page.context().pages().length).toBe(pagesBeforePhoto)
    await meterDialog.screenshot({ path: 'target/rental-admin-meter-modal-desktop.png' })
    const desktopViewport = page.viewportSize()!
    await page.setViewportSize({ width: 390, height: 844 })
    await meterDialog.screenshot({ path: 'target/rental-admin-meter-modal-mobile.png' })
    await page.setViewportSize(desktopViewport)
    await expect(meterDialog.getByRole('button', { name: 'Zavřít fotografii' })).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(meterDialog).toBeHidden()
    await expect(beforePhoto).toBeFocused()
    await expect(page.locator('[data-meter-dialog-image] img')).toHaveCount(0)
    expect(await page.evaluate(async url => {
      try { return (await fetch(url!)).ok } catch { return false }
    }, meterUrl)).toBe(false)
    await page.getByRole('button', { name: 'Zobrazit fotografii po pronájmu', exact: true }).click()
    const afterPhotoDialog = page.getByRole('dialog', { name: 'Elektroměr po pronájmu', exact: true })
    await expect(afterPhotoDialog.getByRole('img', { name: 'Elektroměr po pronájmu', exact: true })).toBeVisible()
    await afterPhotoDialog.getByRole('button', { name: 'Zavřít fotografii' }).click()
    await expect(afterPhotoDialog).toBeHidden()
    await page.getByLabel('Spotřeba elektřiny v kWh').fill('12,5')
    await page.getByLabel('Celková cena energie v Kč').fill('150')
    await expect(page.getByLabel('Splatnost ve dnech')).toHaveValue('7')
    await page.getByLabel('Číslo účtu včetně případného předčíslí').fill('19-2000145399')
    await page.getByLabel('Kód banky', { exact: true }).fill('0800')
    await expect(page.getByLabel('IBAN pro zahraniční platby (nepovinný)')).toHaveValue('')
    const invoiceAttachment = page.getByLabel('Faktura od účetní (nepovinné PDF)')
    await expect(invoiceAttachment).not.toHaveAttribute('multiple')
    await expect(page.getByRole('button', { name: 'Odebrat vybranou fakturu' })).toHaveCount(0)
    phase = 'select invoice attachment'
    await invoiceAttachment.setInputFiles({ name: 'faktura.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4\n%%EOF\n') })
    await expect(invoiceAttachment).not.toHaveValue('')
    await expect(page.locator('[data-invoice-filename]')).toHaveText('faktura.pdf')
    await expect(page.getByRole('button', { name: 'Odebrat vybranou fakturu' })).toHaveText('×')
    await page.locator('.rental-invoice-card').screenshot({ path: 'target/rental-admin-invoice-desktop.png' })
    await page.setViewportSize({ width: 390, height: 844 })
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    await page.locator('.rental-invoice-card').screenshot({ path: 'target/rental-admin-invoice-mobile.png' })
    await page.setViewportSize(desktopViewport)
    phase = 'remove invoice attachment'
    await page.getByRole('button', { name: 'Odebrat vybranou fakturu' }).click()
    await expect(invoiceAttachment).toHaveValue('')
    await expect(invoiceAttachment).toBeVisible()
    await expect(invoiceAttachment).toBeFocused()
    await expect(page.locator('[data-invoice-filename]')).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Odebrat vybranou fakturu' })).toHaveCount(0)
    await expect(page.getByText('Celkem k úhradě: 400 Kč')).toBeVisible()
    const invoice = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/invoice`) && response.request().method() === 'POST')
    phase = 'send invoice'
    await page.getByRole('button', { name: 'Odeslat vyúčtování' }).click()
    expect((await invoice).status()).toBe(200)
    await expect(page.getByRole('heading', { name: 'Vyúčtování', exact: true })).toBeVisible()
    await expect(page.getByText('Celkem: 400 Kč')).toBeVisible()
    await expect(page.getByText('Účet: 19-2000145399/0800')).toBeVisible()
    page.once('dialog', dialog => dialog.accept())
    const paid = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${id}/paid`) && response.request().method() === 'POST')
    phase = 'mark paid'
    await page.getByRole('button', { name: 'Potvrdit úhradu a uzavřít nájem' }).click()
    expect((await paid).status()).toBe(200)
    await expect(page.getByText('Zaplaceno', { exact: true })).toBeVisible()
    const session = await (await page.request.get('/api/v1/admin/session')).json()
    expect((await page.request.post(`/api/v1/admin/rentals/${id}/identity`, { headers: { 'X-CSRF-Token': session.csrf_token }, data: { password: 'Disposable-browser-test-password!' } })).status()).toBe(404)
    expect({ errors, browserErrors: errors.length ? browserErrors : [] }).toEqual({ errors: [], browserErrors: [] })
  } finally {
    await publicContext.close()
  }
})

test('rejecting a request requires explicit confirmation', async ({ page }) => {
  await login(page)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const createdAsset = await page.request.post('/api/v1/admin/rental-assets', {
    headers: { 'X-CSRF-Token': session.csrf_token },
    data: { name: 'Sál pro zamítnutí', description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
  })
  expect(createdAsset.status()).toBe(200)
  const assetId = (await createdAsset.json()).id
  const future = shift(new Date(), 14)
  const monday = shift(future, (8 - future.getUTCDay()) % 7)
  const date = dateKey(monday)
  const response = await page.request.post('/api/v1/rentals', { multipart: {
    asset_id: String(assetId), start_date: date, end_date: date, start_time: '10:00', end_time: '11:00', event_type: 'standard',
    name: 'Jan Žadatel', email: 'request@vysker.test', phone: '+420 777 123 456',
    address: 'Vyskeř 123', consent: 'true',
    identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
  } })
  expect(response.status()).toBe(201)
  const { id } = await response.json()
  await page.goto(`/admin/pronajmy/${id}`)
  let rejectionRequests = 0
  page.on('request', request => {
    if (request.url().endsWith(`/api/v1/admin/rentals/${id}/reject`) && request.method() === 'POST') rejectionRequests++
  })
  await page.getByRole('button', { name: 'Zamítnout žádost' }).click()
  const dialog = page.getByRole('dialog', { name: 'Zamítnout žádost?' })
  await expect(dialog).toBeVisible()
  await expect(dialog).toContainText('Jan Žadatel')
  await expect(dialog.getByRole('button', { name: 'Zrušit' })).toBeFocused()
  await dialog.getByRole('button', { name: 'Zrušit' }).click()
  await expect(dialog).toHaveCount(0)
  expect(rejectionRequests).toBe(0)
  expect((await (await page.request.get(`/api/v1/admin/rentals/${id}`)).json()).status).toBe('pending')
  await page.getByRole('button', { name: 'Zamítnout žádost' }).click()
  const rejected = page.waitForResponse(request => request.url().endsWith(`/api/v1/admin/rentals/${id}/reject`) && request.request().method() === 'POST')
  await page.getByRole('dialog', { name: 'Zamítnout žádost?' }).getByRole('button', { name: 'Zamítnout žádost' }).click()
  expect((await rejected).status()).toBe(200)
  await expect(page.getByText('Zamítnuto', { exact: true })).toBeVisible()
  expect((await (await page.request.get(`/api/v1/admin/rentals/${id}`)).json()).status).toBe('rejected')
})
