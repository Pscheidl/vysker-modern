import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'

const adminPassword = 'Disposable-browser-test-password!'
const limitedPassword = 'Dlouhe-heslo-noveho-spravce-2026!'

async function login(page: Page, email: string, password: string) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill(email)
  await page.getByLabel('Heslo', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('disabling rentals hides public navigation and rejects even already open forms until re-enabled', async ({ page, browser }) => {
  test.setTimeout(120_000)
  await login(page, 'e2e-admin@vysker.test', adminPassword)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const headers = { 'X-CSRF-Token': session.csrf_token }
  const original = await (await page.request.get('/api/v1/admin/rental-settings')).json()
  expect(original.enabled).toBe(true)
  const suffix = Date.now()
  const limitedEmail = `bez-rezervaci-${suffix}@vysker.test`
  const account = await page.request.post('/api/v1/admin/accounts', { headers, data: {
    email: limitedEmail, password: limitedPassword, current_password: adminPassword,
    can_view_identity: false, can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false,
  } })
  expect(account.status()).toBe(201)
  const assetName = `Sál pro vypnutí ${suffix}`
  const asset = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
    name: assetName, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true,
  } })
  expect(asset.status()).toBe(200)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const limitedContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const publicPage = await context.newPage()
  const limitedPage = await limitedContext.newPage()
  try {
    await publicPage.goto('/pronajmy')
    await publicPage.getByLabel('Co si chcete pronajmout').selectOption({ label: assetName })
    await expect(publicPage.locator('#rental-fields')).toBeEnabled()
    await publicPage.locator('#rental-event-type').selectOption('standard')
    const saturday = new Date(Date.now() + 14 * 86_400_000)
    saturday.setUTCDate(saturday.getUTCDate() + (6 - saturday.getUTCDay() + 7) % 7)
    await publicPage.locator('#rental-start').fill(saturday.toISOString().slice(0, 10))
    await expect(publicPage.locator('#rental-selection-summary')).toContainText('250 Kč')
    await publicPage.getByLabel('Jméno a příjmení', { exact: true }).fill('Ukázkový žadatel')
    await publicPage.getByLabel('E-mail', { exact: true }).fill('zadatel@example.test')
    await publicPage.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
    await publicPage.getByLabel('Adresa', { exact: true }).fill('Vyskeř 1')
    await publicPage.getByLabel('Kopie občanského průkazu', { exact: true }).setInputFiles('public/images/vysker-znak-app-192.png')
    await publicPage.getByLabel('Souhlasím s dočasným uložením').check()

    await page.goto('/admin/pronajmy')
    await expect(page.getByRole('heading', { name: 'Rezervační systém', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Vypnout rezervace', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: 'Vypnout rezervační systém?' })
    await expect(dialog).toBeVisible()
    await expect(dialog.getByRole('button', { name: 'Zrušit' })).toBeFocused()
    await dialog.getByRole('button', { name: 'Zrušit' }).click()
    expect(await (await page.request.get('/api/v1/admin/rental-settings')).json()).toEqual(original)
    await page.getByRole('button', { name: 'Vypnout rezervace', exact: true }).click()
    dialog = page.getByRole('dialog', { name: 'Vypnout rezervační systém?' })
    const disabled = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rental-settings') && response.request().method() === 'PUT')
    await dialog.getByRole('button', { name: 'Vypnout rezervace', exact: true }).click()
    expect((await disabled).status()).toBe(200)
    const off = await (await page.request.get('/api/v1/admin/rental-settings')).json()
    expect(off.enabled).toBe(false)
    await expect(page.getByRole('button', { name: 'Zapnout rezervace', exact: true })).toBeVisible()
    await page.locator('section[aria-label="Provoz rezervačního systému"]').screenshot({
      path: 'target/rental-system-off-admin.png', style: '.skip-link { visibility: hidden !important; }',
    })

    const rejected = publicPage.waitForResponse(response => response.url().endsWith('/api/v1/rentals') && response.request().method() === 'POST')
    await publicPage.getByRole('button', { name: 'Odeslat žádost' }).click()
    const rejectedResponse = await rejected
    expect(rejectedResponse.status()).toBe(503)
    await expect(publicPage.locator('#rental-message')).toContainText('Rezervační systém je vypnutý')
    const unavailableAssets = await publicPage.request.get('/api/v1/rental-assets')
    expect(unavailableAssets.status()).toBe(503)
    expect(unavailableAssets.headers()['cache-control']).toContain('no-store')
    expect((await publicPage.request.get('/api/v1/rental-access/ukazkovy-token')).status()).toBe(503)
    expect((await publicPage.request.post('/api/v1/rental-access/ukazkovy-token/photos')).status()).toBe(503)
    expect((await page.request.get('/api/v1/admin/rentals')).status()).toBe(503)
    expect((await page.request.post('/api/v1/admin/rentals/1/identity', { headers, data: { password: adminPassword } })).status()).toBe(503)
    expect((await page.request.put('/api/v1/admin/rental-settings', { headers, data: { enabled: true, expected_version: original.version } })).status()).toBe(409)

    let assetRequests = 0
    page.on('request', request => { if (request.url().endsWith('/api/v1/admin/rental-assets')) assetRequests++ })
    await page.goto('/admin/pronajmy/majetek')
    await expect(page.getByText('Rezervační systém je vypnutý', { exact: false })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Přejít na nastavení rezervací' })).toBeVisible()
    expect(assetRequests).toBe(0)
    await page.getByRole('link', { name: 'Přejít na nastavení rezervací' }).click()
    await expect(page.getByRole('button', { name: 'Zapnout rezervace', exact: true })).toBeVisible()

    await login(limitedPage, limitedEmail, limitedPassword)
    const limitedSession = await (await limitedPage.request.get('/api/v1/admin/session')).json()
    await limitedPage.goto('/admin/pronajmy')
    await expect(limitedPage.getByRole('heading', { name: 'Rezervační systém', exact: true })).toBeVisible()
    await expect(limitedPage.getByRole('button', { name: 'Zapnout rezervace', exact: true })).toHaveCount(0)
    const forbidden = await limitedPage.request.put('/api/v1/admin/rental-settings', {
      headers: { 'X-CSRF-Token': limitedSession.csrf_token }, data: { enabled: true, expected_version: off.version },
    })
    expect(forbidden.status()).toBe(403)
    expect((await (await page.request.get('/api/v1/admin/rental-settings')).json()).enabled).toBe(false)

    for (const [width, javaScriptEnabled] of [[1440, true], [390, true], [1440, false], [390, false]] as const) {
      const visitor = await browser.newContext({ baseURL: new URL(page.url()).origin, viewport: { width, height: 844 }, javaScriptEnabled })
      try {
        const visitorPage = await visitor.newPage()
        await visitorPage.goto('/')
        await expect(visitorPage.locator('.site-header a[href^="/pronajmy"]')).toHaveCount(0)
        await expect(visitorPage.locator('.site-header a').filter({ hasText: /^Pronájmy$/ })).toHaveCount(0)
        const direct = await visitorPage.goto('/pronajmy')
        expect(direct?.status()).toBe(503)
        expect(direct?.headers()['cache-control']).toContain('no-store')
        await expect(visitorPage.getByRole('heading', { name: 'Pronájmy nejsou dostupné' })).toBeVisible()
        await expect(visitorPage.locator('meta[name="robots"][content*="noindex"]').first()).toHaveCount(1)
        await expect(visitorPage.locator('#rental-form')).toHaveCount(0)
        if (width === 1440 && javaScriptEnabled) {
          await visitorPage.evaluate(() => document.fonts.ready)
          await visitorPage.screenshot({ path: 'target/rental-system-off-public.png', fullPage: true,
            style: '.site-header { position: static !important; } .skip-link { visibility: hidden !important; }' })
        }
        const meter = await visitorPage.goto('/pronajmy/meridlo/ukazkovy-token')
        expect(meter?.status()).toBe(503)
        await expect(visitorPage.getByRole('heading', { name: 'Pronájmy nejsou dostupné' })).toBeVisible()
      } finally {
        await visitor.close()
      }
    }

    await page.getByRole('button', { name: 'Zapnout rezervace', exact: true }).click()
    const enableDialog = page.getByRole('dialog', { name: 'Zapnout rezervační systém?' })
    const enabled = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rental-settings') && response.request().method() === 'PUT')
    await enableDialog.getByRole('button', { name: 'Zapnout rezervace', exact: true }).click()
    expect((await enabled).status()).toBe(200)
    expect((await (await page.request.get('/api/v1/admin/rental-settings')).json()).enabled).toBe(true)
    expect((await publicPage.request.get('/api/v1/rental-assets')).status()).toBe(200)
    const visitorPage = await context.newPage()
    await visitorPage.goto('/')
    await expect(visitorPage.locator('.site-header a[href="/pronajmy"]').first()).toBeAttached()
    const noScript = await browser.newContext({ baseURL: new URL(page.url()).origin, viewport: { width: 390, height: 844 }, javaScriptEnabled: false })
    try {
      const mobilePage = await noScript.newPage()
      await mobilePage.goto('/')
      await expect(mobilePage.locator('.site-header a[href="/pronajmy"]').first()).toBeAttached()
      expect((await mobilePage.goto('/pronajmy'))?.status()).toBe(200)
      await expect(mobilePage.locator('#rental-form')).toHaveCount(1)
    } finally {
      await noScript.close()
    }
  } finally {
    const current = await page.request.get('/api/v1/admin/rental-settings')
    if (current.ok()) {
      const settings = await current.json()
      if (!settings.enabled) {
        const restored = await page.request.put('/api/v1/admin/rental-settings', { headers, data: { enabled: true, expected_version: settings.version } })
        expect(restored.status()).toBe(200)
      }
    }
    await limitedContext.close()
    await context.close()
  }
})
