import { test, expect } from './fixtures'
import { testRates } from './rental-test-data'

test('saving another asset keeps drafts and leaving asks before discarding them', async ({ page }) => {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const names = [`Rozepsaný majetek ${Date.now()}`, `Ukládaný majetek ${Date.now()}`]
  for (const name of names) {
    const created = await page.request.post('/api/v1/admin/rental-assets', {
      headers: { 'X-CSRF-Token': session.csrf_token },
      data: { name, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
    })
    expect(created.status()).toBe(200)
  }
  await page.goto('/admin/pronajmy/majetek')
  const card = (name: string) => page.locator('section.rental-asset-card').filter({ has: page.getByRole('heading', { name, exact: true }) })
  let first = card(names[0])
  const second = card(names[1])
  await expect(first).toBeVisible()
  await expect(second).toBeVisible()
  const draftName = `${names[0]} změna`
  await first.getByLabel('Název', { exact: true }).fill(draftName)
  first = card(draftName)
  const block = first.locator('section.rental-asset-blocks')
  await block.getByLabel('Od', { exact: true }).fill('2031-01-13')
  await block.getByLabel('Do včetně', { exact: true }).fill('2031-01-14')
  await second.getByLabel('Soukromá akce: cena za hodinu v Kč').fill('120')
  const saved = page.waitForResponse(response => /\/api\/v1\/admin\/rental-assets\/\d+$/.test(response.url()) && response.request().method() === 'PUT')
  await second.getByRole('button', { name: 'Uložit', exact: true }).click()
  expect((await saved).status()).toBe(200)
  await expect(first.getByLabel('Název', { exact: true })).toHaveValue(draftName)
  await expect(block.getByLabel('Od', { exact: true })).toHaveValue('2031-01-13')
  await expect(block.getByLabel('Do včetně', { exact: true })).toHaveValue('2031-01-14')

  const rejectedDialog = page.waitForEvent('dialog')
  const stay = page.getByRole('link', { name: 'Úřední deska', exact: true }).click()
  const rejected = await rejectedDialog
  expect(rejected.type()).toBe('confirm')
  expect(rejected.message()).toContain('neuložené změny')
  await rejected.dismiss()
  await stay
  await expect(page).toHaveURL(/\/admin\/pronajmy\/majetek$/)
  await expect(first.getByLabel('Název', { exact: true })).toHaveValue(draftName)
  await expect(block.getByLabel('Od', { exact: true })).toHaveValue('2031-01-13')

  const acceptedDialog = page.waitForEvent('dialog')
  const leave = page.getByRole('link', { name: 'Úřední deska', exact: true }).click()
  const accepted = await acceptedDialog
  expect(accepted.type()).toBe('confirm')
  await accepted.accept()
  await leave
  await expect(page).toHaveURL(/\/admin\/uredni-deska$/)
})

for (const operation of ['create', 'invoice'] as const) {
  test(`a late ${operation} response cannot clear the next editor's unsaved-change protection`, async ({ page }) => {
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    await page.goto('/admin')
    await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
    await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
    await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
    await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
    const session = await (await page.request.get('/api/v1/admin/session')).json()
    const assetName = `Nový rozepsaný majetek ${operation} ${Date.now()}`
    const asset = await page.request.post('/api/v1/admin/rental-assets', {
      headers: { 'X-CSRF-Token': session.csrf_token },
      data: { name: assetName, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true },
    })
    expect(asset.status()).toBe(200)
    const assetId = (await asset.json()).id
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    let held = false
    const path = operation === 'create' ? '/api/v1/admin/rentals' : '/api/v1/admin/rentals/777778/invoice'
    await page.route(`**${path}`, async route => {
      if (route.request().method() !== 'POST') return route.continue()
      held = true
      await gate
      // Hold only the browser response, without changing an unrelated database row.
      await route.fulfill({ status: operation === 'create' ? 201 : 200, json: operation === 'create' ? { rentals: [{ id: 777778 }] } : {} })
    })
    try {
      if (operation === 'create') {
        await page.goto('/admin/pronajmy/nova')
        await page.getByLabel('Předmět pronájmu').selectOption(String(assetId))
        await page.getByLabel('Délka pronájmu').selectOption('daily')
        await page.getByLabel('Datum', { exact: true }).fill(new Date(Date.now() + 14 * 86400000).toISOString().slice(0, 10))
        await page.getByLabel('Jméno a příjmení', { exact: true }).fill('Jan Zkušební')
        await page.getByLabel('E-mail pro vyúčtování').fill('draft@example.test')
        await page.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
        await page.getByLabel('Adresa', { exact: true }).fill('Testovací adresa 1')
        await page.getByLabel('Sjednaná cena za jeden termín v Kč').fill('0')
        await page.getByRole('button', { name: 'Zkontrolovat termíny', exact: true }).click()
        await expect(page.getByRole('heading', { name: 'Všechny termíny jsou volné' })).toBeVisible()
        await page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci', exact: true }).click()
      } else {
        await page.route('**/api/v1/admin/rentals/777778', route => route.fulfill({ json: {
          id: 777778, asset_id: assetId, asset_name: assetName, start_date: '2020-01-01', end_date: '2020-01-01',
          invoice_available_at: '2020-01-02T00:00:00', invoice_available_unix: 1577919600,
          source: 'admin', status: 'approved', name: 'Jan Zkušební', email: 'draft@example.test', phone: '+420 777 123 456',
          address: 'Testovací adresa 1', tenant_type: 'person', require_meter: false, wait_for_payment: true,
          before_uploaded: false, after_uploaded: false, rental_price: 0, event_type: 'standard',
        } }))
        await page.goto('/admin/pronajmy/777778')
        const due = page.getByLabel('Splatnost ve dnech', { exact: true })
        await due.fill((await due.inputValue()) === '7' ? '8' : '7')
        await page.getByRole('button', { name: 'Odeslat vyúčtování', exact: true }).click()
      }
      await expect.poll(() => held).toBe(true)
      const leaveDialog = page.waitForEvent('dialog')
      const leave = page.getByRole('link', { name: 'Zpět na pronájmy', exact: true }).click()
      const confirmation = await leaveDialog
      expect(confirmation.message()).toContain('neuložené změny')
      await confirmation.accept()
      await leave
      await expect(page).toHaveURL(/\/admin\/pronajmy$/)
      await page.getByRole('link', { name: 'Správa majetku a cen', exact: true }).click()
      const editor = page.locator('.rental-asset-card').filter({ has: page.getByRole('heading', { name: assetName, exact: true }) })
      await editor.getByLabel('Popis', { exact: true }).fill('Nová rozpracovaná změna musí zůstat chráněná.')

      const completed = page.waitForResponse(response => response.url().endsWith(path) && response.request().method() === 'POST')
      release()
      expect((await completed).status()).toBe(operation === 'create' ? 201 : 200)
      // Let the old asynchronous action process the delayed response.
      await page.waitForTimeout(150)
      await expect(page.getByText(operation === 'create' ? 'Rezervace byla vytvořena.' : 'Změna pronájmu byla uložena.', { exact: true })).toHaveCount(0)
      await expect(editor.getByLabel('Popis', { exact: true })).toHaveValue('Nová rozpracovaná změna musí zůstat chráněná.')

      const warning = page.waitForEvent('dialog', { timeout: 3_000 })
      const navigate = page.getByRole('link', { name: 'Úřední deska', exact: true }).click()
      const dialog = await warning
      expect(dialog.message()).toContain('neuložené změny')
      await dialog.dismiss()
      await navigate
      await expect(page).toHaveURL(/\/admin\/pronajmy\/majetek$/)
      expect(errors).toEqual([])
    } finally {
      release()
    }
  })
}
