import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

const primaryPassword = 'Disposable-browser-test-password!'
const accountPassword = 'Dlouhe-heslo-noveho-spravce-2026!'

async function login(page: Page, email: string, password: string) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill(email)
  await page.getByLabel('Heslo', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

function endRental(id: number) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  execFileSync('python3', ['-c', [
    'import psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    if not connection.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    connection.execute("UPDATE rentals SET start_date = CURRENT_DATE - 1, end_date = CURRENT_DATE - 1 WHERE id = %s", (int(sys.argv[2]),))',
  ].join('\n'), database, String(id)])
}

test('only an administrator with document access can decide rental applications', async ({ page, browser }) => {
  await login(page, 'e2e-admin@vysker.test', primaryPassword)
  const primary = await (await page.request.get('/api/v1/admin/session')).json()
  const headers = { 'x-csrf-token': primary.csrf_token }
  const suffix = Date.now()
  const basicEmail = `rozhodovani-zakladni-${suffix}@vysker.test`
  const permittedEmail = `rozhodovani-povereny-${suffix}@vysker.test`
  let basicId = 0
  let permittedId = 0

  for (const [email, canView] of [[basicEmail, false], [permittedEmail, true]] as const) {
    const response = await page.request.post('/api/v1/admin/accounts', { headers, data: {
      email, password: accountPassword, current_password: primaryPassword,
      can_view_identity: canView, can_manage_permissions: false,
      can_create_accounts: false, can_manage_mail_settings: false,
    } })
    expect(response.status()).toBe(201)
    const created = await response.json()
    if (canView) permittedId = created.id
    else basicId = created.id
  }

  const assetResponse = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
    name: `Sál pro rozhodování ${suffix}`, description: '', rates: testRates(), notification_email: 'office@vysker.test', require_meter: false, active: true,
  } })
  expect(assetResponse.status()).toBe(200)
  const asset = await assetResponse.json()
  const monday = new Date(Date.now() + 14 * 86_400_000)
  monday.setUTCDate(monday.getUTCDate() + (1 - monday.getUTCDay() + 7) % 7)
  const createApplication = async (offset: number, name: string) => {
    const day = new Date(monday.getTime() + offset * 86_400_000).toISOString().slice(0, 10)
    const response = await page.request.post('/api/v1/rentals', { multipart: {
      asset_id: String(asset.id), start_date: day, end_date: day, start_time: '10:00', end_time: '11:00', event_type: 'standard', name,
      email: 'zadatel@vysker.test', phone: '+420 777 123 456', address: 'Vyskeř 1', consent: 'true',
      identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
    } })
    expect(response.status()).toBe(201)
    return (await response.json()).id as number
  }
  const approvedId = await createApplication(0, 'Žadatel ke schválení')
  const rejectedId = await createApplication(1, 'Žadatel k zamítnutí')
  const blockedDay = new Date(monday.getTime() + 3 * 86_400_000).toISOString().slice(0, 10)
  const blockResponse = await page.request.post(`/api/v1/admin/rental-assets/${asset.id}/blocks`, {
    headers, data: { start_date: blockedDay, end_date: blockedDay },
  })
  expect(blockResponse.status()).toBe(201)
  const block = await blockResponse.json()

  const basicContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const allowedContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const basic = await basicContext.newPage()
    await login(basic, basicEmail, accountPassword)
    const basicSession = await (await basic.request.get('/api/v1/admin/session')).json()
    const basicHeaders = { 'x-csrf-token': basicSession.csrf_token }
    expect((await basic.request.get('/api/v1/admin/rental-assets')).status()).toBe(200)
    const readableBlocks = await basic.request.get(`/api/v1/admin/rental-assets/${asset.id}/blocks`)
    expect(readableBlocks.status()).toBe(200)
    expect(await readableBlocks.json()).toEqual(expect.arrayContaining([expect.objectContaining({ id: block.id })]))
    const assetInput = { name: 'Nepovolená změna', description: '', rates: testRates(),
      notification_email: 'office@vysker.test', require_meter: false, active: true }
    expect((await basic.request.post('/api/v1/admin/rental-assets', { headers: basicHeaders, data: assetInput })).status()).toBe(403)
    expect((await basic.request.put(`/api/v1/admin/rental-assets/${asset.id}`, { headers: basicHeaders, data: assetInput })).status()).toBe(403)
    expect((await basic.request.post(`/api/v1/admin/rental-assets/${asset.id}/blocks`, {
      headers: basicHeaders, data: { start_date: blockedDay, end_date: blockedDay },
    })).status()).toBe(403)
    expect((await basic.request.delete(`/api/v1/admin/rental-assets/${asset.id}/blocks/${block.id}`, { headers: basicHeaders })).status()).toBe(403)
    await basic.goto('/admin/pronajmy/majetek')
    await expect(basic.getByText('Rezervace a majetek můžete pouze prohlížet. Pro změny požádejte pověřeného správce.')).toBeVisible()
    await expect(basic.getByRole('heading', { name: `Sál pro rozhodování ${suffix}` })).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Přidat majetek' })).toHaveCount(0)
    await expect(basic.getByRole('button', { name: 'Uložit', exact: true })).toHaveCount(0)
    await expect(basic.getByRole('button', { name: 'Blokovat termín' })).toHaveCount(0)
    await expect(basic.getByRole('button', { name: 'Odstranit blokaci' })).toHaveCount(0)

    await basic.goto(`/admin/pronajmy/${approvedId}`)
    await expect(basic.getByRole('heading', { name: `Pronájem č. ${approvedId}` })).toBeVisible()
    await expect(basic.getByText('Žadatel ke schválení', { exact: false })).toBeVisible()
    await expect(basic.getByText('Rezervaci můžete pouze prohlížet. Pro změny požádejte pověřeného správce.')).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Schválit žádost' })).toHaveCount(0)
    await expect(basic.getByRole('button', { name: 'Zamítnout žádost' })).toHaveCount(0)
    for (const action of ['approve', 'reject']) {
      const response = await basic.request.post(`/api/v1/admin/rentals/${approvedId}/${action}`, {
        headers: basicHeaders,
      })
      expect(response.status()).toBe(403)
    }
    expect((await (await basic.request.get(`/api/v1/admin/rentals/${approvedId}`)).json()).status).toBe('pending')

    // The account stays signed in while a second administrator grants and revokes access.
    const updatePermissions = async (canView: boolean) => page.request.put(`/api/v1/admin/accounts/${basicId}/permissions`, {
      headers, data: { current_password: primaryPassword, can_view_identity: canView,
        can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false },
    })
    expect((await updatePermissions(true)).status()).toBe(204)
    await basic.getByRole('link', { name: 'Zpět na pronájmy' }).click()
    await basic.getByRole('row').filter({ hasText: 'Žadatel ke schválení' }).getByRole('link', { name: 'Žadatel ke schválení' }).click()
    await expect(basic.getByRole('button', { name: 'Schválit žádost' })).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Zamítnout žádost' })).toBeVisible()
    expect((await updatePermissions(false)).status()).toBe(204)
    await basic.getByRole('link', { name: 'Zpět na pronájmy' }).click()
    await basic.getByRole('row').filter({ hasText: 'Žadatel ke schválení' }).getByRole('link', { name: 'Žadatel ke schválení' }).click()
    await expect(basic.getByText('Rezervaci můžete pouze prohlížet. Pro změny požádejte pověřeného správce.')).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Schválit žádost' })).toHaveCount(0)
    await expect(basic.getByRole('button', { name: 'Zamítnout žádost' })).toHaveCount(0)

    const allowed = await allowedContext.newPage()
    await login(allowed, permittedEmail, accountPassword)
    await allowed.goto(`/admin/pronajmy/${approvedId}`)
    const allowedSession = await (await allowed.request.get('/api/v1/admin/session')).json()
    const allowedHeaders = { 'x-csrf-token': allowedSession.csrf_token }
    const identity = await allowed.request.post(`/api/v1/admin/rentals/${approvedId}/identity`, {
      headers: allowedHeaders, data: { password: accountPassword },
    })
    expect(identity.status()).toBe(200)
    const audit = await (await allowed.request.get('/api/v1/admin/audit?limit=100')).json()
    expect(audit).toEqual(expect.arrayContaining([expect.objectContaining({
      operation: 'viewed_identity', actor_id: permittedId, actor_email: permittedEmail,
      entity_type: 'rental', entity_id: approvedId,
    })]))
    await expect(allowed.getByRole('button', { name: 'Schválit žádost' })).toBeVisible()
    await expect(allowed.getByRole('button', { name: 'Zamítnout žádost' })).toBeVisible()
    await allowed.getByRole('button', { name: 'Schválit žádost' }).click()
    const approve = allowed.getByRole('dialog', { name: 'Schválit žádost?' })
    await expect(approve).toContainText('Žadatel ke schválení')
    await expect(approve.getByRole('button', { name: 'Zrušit' })).toBeFocused()
    const approved = allowed.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${approvedId}/approve`) && response.request().method() === 'POST')
    await approve.getByRole('button', { name: 'Schválit žádost' }).click()
    expect((await approved).status()).toBe(200)
    expect((await (await allowed.request.get(`/api/v1/admin/rentals/${approvedId}`)).json()).status).toBe('approved')

    await basic.goto(`/admin/pronajmy/${approvedId}`)
    await expect(basic.getByText('Rezervaci můžete pouze prohlížet. Pro změny požádejte pověřeného správce.')).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Odeslat vyúčtování' })).toHaveCount(0)
    endRental(approvedId)
    const invoiceInput = { energy_kwh: '0', energy_cost: 0, extra_cost: 0,
      account_number: '19-2000145399', bank_code: '0800', account_iban: '', due_days: 7 }
    expect((await basic.request.post(`/api/v1/admin/rentals/${approvedId}/invoice`, { headers: basicHeaders, data: invoiceInput })).status()).toBe(403)
    expect((await allowed.request.post(`/api/v1/admin/rentals/${approvedId}/invoice`, { headers: allowedHeaders, data: invoiceInput })).status()).toBe(200)
    await basic.reload()
    await expect(basic.getByRole('heading', { name: 'Vyúčtování', exact: true })).toBeVisible()
    await expect(basic.getByRole('button', { name: 'Potvrdit úhradu a uzavřít nájem' })).toHaveCount(0)
    expect((await basic.request.post(`/api/v1/admin/rentals/${approvedId}/paid`, { headers: basicHeaders })).status()).toBe(403)

    await allowed.goto(`/admin/pronajmy/${rejectedId}`)
    await allowed.getByRole('button', { name: 'Zamítnout žádost' }).click()
    const reject = allowed.getByRole('dialog', { name: 'Zamítnout žádost?' })
    await expect(reject).toContainText('Žadatel k zamítnutí')
    const rejected = allowed.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${rejectedId}/reject`) && response.request().method() === 'POST')
    await reject.getByRole('button', { name: 'Zamítnout žádost' }).click()
    expect((await rejected).status()).toBe(200)
    expect((await (await allowed.request.get(`/api/v1/admin/rentals/${rejectedId}`)).json()).status).toBe('rejected')
  } finally {
    await basicContext.close()
    await allowedContext.close()
  }
})
