import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'

const adminPassword = 'Disposable-browser-test-password!'
const limitedPassword = 'Dlouhe-heslo-noveho-spravce-2026!'

async function login(page: Page, email: string, password: string) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill(email)
  await page.getByLabel('Heslo', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('payment defaults validate account and IBAN, prefill billing, and restrict editing', async ({ page, browser }) => {
  await login(page, 'e2e-admin@vysker.test', adminPassword)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const headers = { 'X-CSRF-Token': session.csrf_token }
  const current = await (await page.request.get('/api/v1/admin/rental-payment-settings')).json()
  expect(current.due_days).toBe(7)
  try {
    await page.goto('/admin/pronajmy/platby')
    await expect(page.getByRole('heading', { name: 'Výchozí platební údaje' })).toBeVisible()
    await expect(page.getByLabel('Výchozí splatnost ve dnech')).toHaveValue('7')
    await page.getByLabel('Číslo účtu obce').fill('19-2000145399')
    await page.getByLabel('Kód banky', { exact: true }).fill('0100')
    await page.getByLabel('IBAN pro zahraniční platbu (nepovinné)').fill('CZ6508000000192000145399')
    const mismatch = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rental-payment-settings') && response.request().method() === 'PUT')
    await page.getByRole('button', { name: 'Uložit platební údaje' }).click()
    expect((await mismatch).status()).toBe(400)
    await expect(page.getByLabel('Číslo účtu obce')).toHaveValue('19-2000145399')
    await page.getByLabel('Kód banky', { exact: true }).fill('0800')
    await page.getByLabel('IBAN pro zahraniční platbu (nepovinné)').clear()
    await page.getByLabel('Výchozí splatnost ve dnech').fill('12')
    const saved = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/rental-payment-settings') && response.request().method() === 'PUT')
    await page.getByRole('button', { name: 'Uložit platební údaje' }).click()
    expect((await saved).status()).toBe(200)
    await expect(page.locator('.admin-flash')).toContainText('Platební údaje byly uloženy.')
    await expect(page.getByRole('button', { name: 'Uložit platební údaje' })).toBeEnabled()
    await page.reload()
    await expect(page.getByLabel('Číslo účtu obce')).toHaveValue('19-2000145399')
    await expect(page.getByLabel('Kód banky', { exact: true })).toHaveValue('0800')
    await expect(page.getByLabel('IBAN pro zahraniční platbu (nepovinné)')).toHaveValue('')
    await expect(page.getByLabel('Výchozí splatnost ve dnech')).toHaveValue('12')

    const suffix = Date.now()
    const limitedEmail = `platby-cist-${suffix}@vysker.test`
    const createdAccount = await page.request.post('/api/v1/admin/accounts', { headers, data: {
      email: limitedEmail, password: limitedPassword, current_password: adminPassword,
      can_view_identity: false, can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false,
    } })
    expect(createdAccount.status()).toBe(201)
    const limitedContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
    try {
      const limitedPage = await limitedContext.newPage()
      await login(limitedPage, limitedEmail, limitedPassword)
      await limitedPage.goto('/admin/pronajmy/platby')
      await expect(limitedPage.getByText('Platební údaje můžete prohlížet.', { exact: false })).toBeVisible()
      await expect(limitedPage.getByLabel('Číslo účtu obce')).toHaveValue('19-2000145399')
      await expect(limitedPage.getByLabel('Číslo účtu obce')).toBeDisabled()
      await expect(limitedPage.getByRole('button', { name: 'Uložit platební údaje' })).toHaveCount(0)
      const limitedSession = await (await limitedPage.request.get('/api/v1/admin/session')).json()
      expect((await limitedPage.request.put('/api/v1/admin/rental-payment-settings', {
        headers: { 'X-CSRF-Token': limitedSession.csrf_token },
        data: { account_number: '', bank_code: '', iban: '', due_days: 7 },
      })).status()).toBe(403)
    } finally {
      await limitedContext.close()
    }

    const assetResponse = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
      name: `Sál s platebními údaji ${suffix}`, description: '', rates: testRates(),
      notification_email: 'office@vysker.test', require_meter: false, active: true,
    } })
    expect(assetResponse.status()).toBe(200)
    const asset = await assetResponse.json()
    const monday = new Date(Date.now() + 14 * 86_400_000)
    monday.setUTCDate(monday.getUTCDate() + (1 - monday.getUTCDay() + 7) % 7)
    const date = monday.toISOString().slice(0, 10)
    const application = await page.request.post('/api/v1/rentals', { multipart: {
      asset_id: String(asset.id), start_date: date, end_date: date, start_time: '10:00', end_time: '11:00', event_type: 'standard', name: 'Žadatel o sál',
      email: 'zadatel@vysker.test', phone: '+420 777 123 456', address: 'Vyskeř 1', consent: 'true',
      identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
    } })
    expect(application.status()).toBe(201)
    const rental = await application.json()
    expect((await page.request.post(`/api/v1/admin/rentals/${rental.id}/approve`, { headers })).status()).toBe(200)
    await page.goto(`/admin/pronajmy/${rental.id}`)
    await expect(page.getByRole('heading', { name: 'Připravit vyúčtování' })).toBeVisible()
    await expect(page.getByLabel('Číslo účtu včetně případného předčíslí')).toHaveValue('19-2000145399')
    await expect(page.getByLabel('Kód banky', { exact: true })).toHaveValue('0800')
    await expect(page.getByLabel('IBAN pro zahraniční platby (nepovinný)')).toHaveValue('')
    await expect(page.getByLabel('Splatnost ve dnech')).toHaveValue('12')
  } finally {
    const restored = await page.request.put('/api/v1/admin/rental-payment-settings', { headers, data: {
      account_number: current.account_number, bank_code: current.bank_code, iban: current.iban, due_days: current.due_days,
    } })
    expect(restored.status()).toBe(200)
  }
})
