import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

const sample = 'public/images/vysker-znak-app-192.png'

async function createMeterLink(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const headers = { 'X-CSRF-Token': session.csrf_token }
  const asset = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
    name: `Elektroměr ${Date.now()}`, description: '', rates: testRates(),
    notification_email: 'office@vysker.test', require_meter: true, active: true,
  } })
  expect(asset.status()).toBe(200)
  const date = new Date(Date.now() + 14 * 86400000).toISOString().slice(0, 10)
  const created = await page.request.post('/api/v1/rentals', { multipart: {
    asset_id: String((await asset.json()).id), start_date: date, end_date: date,
    event_type: 'standard', name: 'Testovací nájemník', email: 'meter@example.test',
    phone: '+420 777 123 456', address: 'Testovací adresa 1', consent: 'true',
    identity: { name: 'test.png', mimeType: 'image/png', buffer: readFileSync(sample) },
  } })
  expect(created.status()).toBe(201)
  const id = (await created.json()).id
  expect((await page.request.post(`/api/v1/admin/rentals/${id}/approve`, { headers })).status()).toBe(200)
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return execFileSync('python3', ['-c', [
    'import psycopg, re, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    if not connection.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    body = connection.execute("SELECT body FROM rental_mail_queue WHERE rental_id=%s AND purpose=\'approval\'", (int(sys.argv[2]),)).fetchone()[0]',
    '    print(re.search(r"/pronajmy/meridlo/[a-f0-9]{64}", body).group(0))',
  ].join('\n'), database, String(id)], { encoding: 'utf8' }).trim()
}

async function expectPhoto(page: Page, kind: 'before' | 'after') {
  const image = page.locator(`[data-meter-card="${kind}"] img`).first()
  await expect(image).toBeVisible()
  await expect.poll(() => image.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0)
}

test('tenant uploads photos separately, sees saved photos and reuses the same link after reload', async ({ page, browser }) => {
  const link = await createMeterLink(page)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const tenant = await context.newPage()
  try {
    await tenant.goto(link)
    const before = tenant.getByLabel('Fotografie před pronájmem', { exact: true })
    const after = tenant.getByLabel('Fotografie po pronájmu', { exact: true })
    const submit = tenant.getByRole('button', { name: 'Uložit fotografie', exact: true })
    const beforeCard = tenant.locator('[data-meter-card="before"]')
    const afterCard = tenant.locator('[data-meter-card="after"]')
    await expect(before).toBeEnabled()
    await expect(beforeCard).toContainText('Zatím nenahráno')
    await expect(afterCard).toContainText('Zatím nenahráno')
    await before.setInputFiles(sample)
    await expectPhoto(tenant, 'before')
    const first = tenant.waitForResponse(response => response.url().endsWith('/photos') && response.request().method() === 'POST')
    await submit.click()
    expect((await first).status()).toBe(200)
    expect(await (await first).json()).toMatchObject({ before_uploaded: true, after_uploaded: false })
    await expect(beforeCard.locator('[data-meter-status]')).toContainText('Nahráno')
    await expect(before).toHaveValue('')
    await expectPhoto(tenant, 'before')
    const preview = await tenant.request.get(link.replace('/pronajmy/meridlo/', '/api/v1/rental-access/') + '/photos/before')
    expect(preview.status()).toBe(200)
    expect(preview.headers()['cache-control']).toBe('private, no-store')
    expect(preview.headers()['referrer-policy']).toBe('no-referrer')
    const savedBefore = await beforeCard.locator('img').getAttribute('src')
    await before.setInputFiles(sample)
    await expect(beforeCard.locator('[data-meter-status]')).toContainText('Vybráno, zatím neuloženo')
    const selectedBefore = await beforeCard.locator('img').getAttribute('src')
    expect(selectedBefore).not.toBe(savedBefore)
    await beforeCard.getByRole('button', { name: 'Zrušit výběr', exact: true }).click()
    await expect(before).toHaveValue('')
    await expect(beforeCard.locator('img')).toHaveAttribute('src', savedBefore!)
    expect(await tenant.evaluate(async url => {
      try { return (await fetch(url!)).ok } catch { return false }
    }, selectedBefore)).toBe(false)
    await tenant.reload()
    await expectPhoto(tenant, 'before')
    await expect(afterCard).toContainText('Zatím nenahráno')
    await after.setInputFiles(sample)
    const second = tenant.waitForResponse(response => response.url().endsWith('/photos') && response.request().method() === 'POST')
    await submit.click()
    expect((await second).status()).toBe(200)
    expect(await (await second).json()).toMatchObject({ before_uploaded: true, after_uploaded: true })
    await expect(afterCard.locator('[data-meter-status]')).toContainText('Nahráno')
    await expectPhoto(tenant, 'after')
    await tenant.reload()
    await expectPhoto(tenant, 'before')
    await expectPhoto(tenant, 'after')
    await before.setInputFiles(sample)
    const replacement = tenant.waitForResponse(response => response.url().endsWith('/photos') && response.request().method() === 'POST')
    await submit.click()
    expect((await replacement).status()).toBe(200)
    await expect(before).toHaveValue('')
    await expectPhoto(tenant, 'after')
    await tenant.setViewportSize({ width: 390, height: 844 })
    expect(await tenant.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    await tenant.locator('.rental-meter').screenshot({ path: 'target/rental-meter-mobile-sample.png' })
  } finally {
    await context.close()
  }
})

test('both meter photos submit together once, survive a failed upload and retry successfully', async ({ page, browser }) => {
  const link = await createMeterLink(page)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const tenant = await context.newPage()
  let release!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  let posts = 0
  await tenant.route('**/api/v1/rental-access/*/photos', async route => {
    posts++
    if (posts === 1) {
      await gate
      await route.fulfill({ status: 503, json: { error: 'Zkušební výpadek, zkuste to znovu.' } })
    } else await route.continue()
  })
  try {
    await tenant.goto(link)
    const before = tenant.getByLabel('Fotografie před pronájmem', { exact: true })
    const after = tenant.getByLabel('Fotografie po pronájmu', { exact: true })
    const submit = tenant.getByRole('button', { name: 'Uložit fotografie', exact: true })
    await expect(before).toBeEnabled()
    await before.setInputFiles(sample)
    await after.setInputFiles(sample)
    await expectPhoto(tenant, 'before')
    await expectPhoto(tenant, 'after')
    await submit.click()
    await expect.poll(() => posts).toBe(1)
    await expect(submit).toBeDisabled()
    await expect(before).toBeDisabled()
    await tenant.locator('#meter-form').dispatchEvent('submit')
    expect(posts).toBe(1)
    release()
    await expect(tenant.locator('#meter-message')).toContainText('Zkušební výpadek')
    await expect(before).not.toHaveValue('')
    await expect(after).not.toHaveValue('')
    await expectPhoto(tenant, 'before')
    await expectPhoto(tenant, 'after')
    const success = tenant.waitForResponse(response => response.url().endsWith('/photos') && response.request().method() === 'POST')
    await submit.click()
    expect((await success).status()).toBe(200)
    expect(await (await success).json()).toMatchObject({ before_uploaded: true, after_uploaded: true })
    expect(posts).toBe(2)
    await expect(before).toHaveValue('')
    await expect(after).toHaveValue('')
    await expectPhoto(tenant, 'before')
    await expectPhoto(tenant, 'after')
    await tenant.locator('#meter-form').dispatchEvent('submit')
    expect(posts).toBe(2)
    await tenant.locator('.rental-meter').screenshot({ path: 'target/rental-meter-desktop-sample.png' })
  } finally {
    release()
    await context.close()
  }
})
