import { test, expect } from './fixtures'
import { testRates } from './rental-test-data'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

// Move only disposable test records into the past to exercise the real worker.
function startInPast(id: number) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  execFileSync('python3', ['-c', [
    'import psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    if not connection.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    connection.execute("UPDATE rentals SET start_date=CURRENT_DATE-1, end_date=CURRENT_DATE-1 WHERE id=%s", (int(sys.argv[2]),))',
  ].join('\n'), database, String(id)])
}

test('unapproved applications expire, disappear from pending and cannot be approved late', async ({ page }) => {
  test.setTimeout(90_000)
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const headers = { 'X-CSRF-Token': session.csrf_token }
  const asset = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
    name: `Sál pro propadlé žádosti ${Date.now()}`, description: '', rates: testRates(),
    notification_email: 'office@vysker.test', require_meter: false, active: true,
  } })
  expect(asset.status()).toBe(200)
  const assetId = (await asset.json()).id
  const monday = new Date(Date.now() + 14 * 86_400_000)
  monday.setUTCDate(monday.getUTCDate() + (1 - monday.getUTCDay() + 7) % 7)
  const day = monday.toISOString().slice(0, 10)
  async function create(name: string, start: string, end: string) {
    const response = await page.request.post('/api/v1/rentals', { multipart: {
      asset_id: String(assetId), start_date: day, end_date: day, start_time: start, end_time: end,
      event_type: 'standard', name, email: 'zadatel@example.test', phone: '+420 777 123 456',
      address: 'Vyskeř 1', consent: 'true',
      identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
    } })
    expect(response.status()).toBe(201)
    return (await response.json()).id as number
  }
  const lateId = await create('Opožděné schválení', '10:00', '11:00')
  const automaticId = await create('Automaticky zrušená žádost', '11:00', '12:00')

  await page.goto(`/admin/pronajmy/${lateId}`)
  await expect(page.getByRole('button', { name: 'Schválit žádost' })).toBeVisible()
  startInPast(lateId)
  await page.getByRole('button', { name: 'Schválit žádost' }).click()
  const lateApproval = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${lateId}/approve`) && response.request().method() === 'POST')
  await page.getByRole('dialog', { name: 'Schválit žádost?' }).getByRole('button', { name: 'Schválit žádost' }).click()
  expect((await lateApproval).status()).toBe(409)
  expect((await (await page.request.get(`/api/v1/admin/rentals/${lateId}`)).json()).status).toBe('cancelled')
  await page.reload()
  await expect(page.getByText('Zrušeno', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Schválit žádost' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Zamítnout žádost' })).toHaveCount(0)

  startInPast(automaticId)
  await expect.poll(async () => (await (await page.request.get(`/api/v1/admin/rentals/${automaticId}`)).json()).status,
    { timeout: 45_000, intervals: [500, 1000, 2000] }).toBe('cancelled')
  const expired = await (await page.request.get(`/api/v1/admin/rentals/${automaticId}`)).json()
  expect(expired.identity_uploaded).toBe(false)
  expect((await page.request.post(`/api/v1/admin/rentals/${automaticId}/identity`, {
    headers, data: { password: 'Disposable-browser-test-password!' },
  })).status()).toBe(404)
  await page.goto('/admin/pronajmy')
  await page.getByLabel('Stav pronájmu').selectOption('pending')
  await expect(page.getByRole('row').filter({ hasText: 'Automaticky zrušená žádost' })).toHaveCount(0)
  await page.getByLabel('Stav pronájmu').selectOption('cancelled')
  const row = page.getByRole('row').filter({ hasText: 'Automaticky zrušená žádost' })
  await expect(row).toContainText('Zrušeno')
  await row.getByRole('link', { name: 'Automaticky zrušená žádost' }).click()
  await expect(page.getByText('Zrušeno', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Schválit žádost' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Zamítnout žádost' })).toHaveCount(0)
})
