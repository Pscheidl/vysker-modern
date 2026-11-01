import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'

const password = 'Disposable-browser-test-password!'
const newPassword = 'Dlouhe-heslo-noveho-spravce-2026!'

async function login(page: Page, email: string, secret: string) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill(email)
  await page.getByLabel('Heslo', { exact: true }).fill(secret)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

async function createAccount(page: Page, email: string, allowIdentity: boolean, allowCreate = false, allowManage = false) {
  const section = page.locator('section').filter({ has: page.getByRole('heading', { name: 'Nový správce', exact: true }) })
  await section.getByLabel('E-mail nového správce *', { exact: true }).fill(email)
  await section.getByLabel(/Počáteční heslo/).fill(newPassword)
  const identity = section.getByLabel('Správa rezervací a přístup k dokladům')
  await expect(identity).not.toBeChecked()
  if (allowIdentity) await identity.check()
  const createAccounts = section.getByLabel('Vytváření účtů správců')
  const manage = section.getByLabel('Správa oprávnění ostatních správců')
  const mail = section.getByLabel('Změna nastavení odesílacího účtu')
  await expect(createAccounts).not.toBeChecked()
  await expect(manage).not.toBeChecked()
  await expect(mail).not.toBeChecked()
  if (allowCreate) await createAccounts.check()
  if (allowManage) await manage.check()
  await section.getByLabel('Vaše současné heslo pro potvrzení').fill(password)
  const created = page.waitForResponse(r => r.url().endsWith('/api/v1/admin/accounts') && r.request().method() === 'POST')
  await section.getByRole('button', { name: 'Vytvořit správce' }).click()
  expect((await created).status()).toBe(201)
}

test('first administrator can assign permissions and basic administrators cannot open identity copies', async ({ page, browser }) => {
  const suffix = Date.now()
  const basicEmail = `zakladni-${suffix}@vysker.test`
  const qualifiedEmail = `doklady-${suffix}@vysker.test`
  const creatorEmail = `zaklada-${suffix}@vysker.test`
  const managerEmail = `spravuje-${suffix}@vysker.test`
  await login(page, 'e2e-admin@vysker.test', password)
  await page.getByRole('link', { name: 'Účty a hesla', exact: true }).click()
  const primary = page.locator('details.admin-account').filter({ hasText: 'e2e-admin@vysker.test' })
  await primary.locator('summary').click()
  await expect(primary.getByLabel('Správa rezervací a přístup k dokladům')).toBeChecked()
  await expect(primary.getByLabel('Správa rezervací a přístup k dokladům')).toBeDisabled()
  await expect(primary.getByLabel('Správa oprávnění ostatních správců')).toBeChecked()
  await expect(primary.getByLabel('Správa oprávnění ostatních správců')).toBeDisabled()
  await expect(primary.getByLabel('Vytváření účtů správců')).toBeChecked()
  await expect(primary.getByLabel('Vytváření účtů správců')).toBeDisabled()
  await expect(primary.getByLabel('Změna nastavení odesílacího účtu')).toBeChecked()
  await expect(primary.getByLabel('Změna nastavení odesílacího účtu')).toBeDisabled()
  const createForm = page.locator('section').filter({ has: page.getByRole('heading', { name: 'Nový správce', exact: true }) })
  const viewIdentity = createForm.getByLabel('Správa rezervací a přístup k dokladům')
  const createAccounts = createForm.getByLabel('Vytváření účtů správců')
  const managePermissions = createForm.getByLabel('Správa oprávnění ostatních správců')
  const manageMail = createForm.getByLabel('Změna nastavení odesílacího účtu')
  await expect(manageMail).toBeDisabled()
  await viewIdentity.check()
  await createAccounts.check()
  await managePermissions.check()
  await expect(manageMail).toBeEnabled()
  await manageMail.check()
  await viewIdentity.uncheck()
  await expect(manageMail).not.toBeChecked()
  await expect(manageMail).toBeDisabled()
  await createAccounts.uncheck()
  await managePermissions.uncheck()
  await createAccount(page, basicEmail, false)
  await createAccount(page, qualifiedEmail, true)
  await createAccount(page, creatorEmail, false, true)
  await createAccount(page, managerEmail, false, false, true)
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const accounts = await (await page.request.get('/api/v1/admin/accounts')).json()
  const basic = accounts.find((a: { email: string }) => a.email === basicEmail)
  const qualified = accounts.find((a: { email: string }) => a.email === qualifiedEmail)
  expect(basic).toMatchObject({ is_primary: false, can_view_identity: false, can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false })
  expect(qualified).toMatchObject({ is_primary: false, can_view_identity: true, can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false })
  const basicContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  const basicPage = await basicContext.newPage()
  await login(basicPage, basicEmail, newPassword)
  expect((await (await basicPage.request.get('/api/v1/admin/session')).json()).can_view_identity).toBe(false)
  await basicPage.goto('/admin/posta/nastaveni')
  await expect(basicPage.getByText('Nemáte oprávnění měnit odesílací účet.', { exact: false })).toBeVisible()
  await expect(basicPage.getByLabel('Způsob odesílání')).toBeDisabled()
  await expect(basicPage.getByLabel('Vaše heslo do administrace pro test')).toBeEnabled()
  await expect(basicPage.getByRole('button', { name: 'Odeslat zkušební e-mail' })).toBeEnabled()
  await basicPage.goto('/admin/ucty')
  await expect(basicPage.getByText('Se základním přístupem můžete spravovat web a prohlížet rezervace', { exact: false })).toBeVisible()
  await expect(basicPage.getByRole('heading', { name: 'Nový správce' })).toHaveCount(0)
  let basicRow = basicPage.locator('details.admin-account').filter({ hasText: basicEmail })
  await basicRow.locator('summary').click()
  await expect(basicRow).toContainText('Správa rezervací a přístup k dokladům: ne')

  const row = page.locator('details.admin-account').filter({ hasText: basicEmail })
  await row.locator('summary').click()
  await row.getByLabel('Správa rezervací a přístup k dokladům').check()
  await row.getByLabel('Vaše současné heslo pro potvrzení').fill(password)
  const save = page.waitForResponse(r => r.url().endsWith(`/api/v1/admin/accounts/${basic.id}/permissions`) && r.request().method() === 'PUT')
  await row.getByRole('button', { name: 'Uložit oprávnění' }).click()
  expect((await save).status()).toBe(204)
  await expect(page.locator('.admin-flash')).toContainText('Oprávnění správce byla uložena.')
  const changed = await (await page.request.get('/api/v1/admin/accounts')).json()
  expect(changed.find((a: { email: string }) => a.email === basicEmail).can_view_identity).toBe(true)
  await basicPage.reload()
  await expect(basicPage.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  expect((await (await basicPage.request.get('/api/v1/admin/session')).json()).can_view_identity).toBe(true)
  basicRow = basicPage.locator('details.admin-account').filter({ hasText: basicEmail })
  await basicRow.locator('summary').click()
  await expect(basicRow).toContainText('Správa rezervací a přístup k dokladům: ano')

  // Revoke again to verify that a basic account cannot see or promote itself.
  const refreshed = page.locator('details.admin-account').filter({ hasText: basicEmail })
  if (await refreshed.getAttribute('open') === null) await refreshed.locator('summary').click()
  await refreshed.getByLabel('Správa rezervací a přístup k dokladům').uncheck()
  await refreshed.getByLabel('Vaše současné heslo pro potvrzení').fill(password)
  const revoked = page.waitForResponse(r => r.url().endsWith(`/api/v1/admin/accounts/${basic.id}/permissions`) && r.request().method() === 'PUT')
  await refreshed.getByRole('button', { name: 'Uložit oprávnění' }).click()
  expect((await revoked).status()).toBe(204)

  const asset = await (await page.request.post('/api/v1/admin/rental-assets', { headers: { 'x-csrf-token': session.csrf_token }, data: {
    name: `Testovací sál ${suffix}`, description: '', rates: testRates(),
    notification_email: 'urad@vysker.test', require_meter: false, active: true,
  } })).json()
  const future = new Date(Date.now() + 14 * 86_400_000)
  future.setUTCDate(future.getUTCDate() + (1 - future.getUTCDay() + 7) % 7)
  const day = future.toISOString().slice(0, 10)
  const createdRental = await page.request.post('/api/v1/rentals', { multipart: {
    asset_id: String(asset.id), start_date: day, end_date: day, start_time: '10:00', end_time: '11:00', event_type: 'standard', name: 'Testovací žadatel', email: 'najimatel@vysker.test',
    phone: '+420 777 123 456', address: 'Vyskeř 1', consent: 'true',
    identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
  } })
  expect(createdRental.status()).toBe(201)
  const rental = await createdRental.json()
  try {
    await basicPage.reload()
    await expect(basicPage.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
    basicRow = basicPage.locator('details.admin-account').filter({ hasText: basicEmail })
    await basicRow.locator('summary').click()
    await expect(basicRow).toContainText('Správa rezervací a přístup k dokladům: ne')
    const basicSession = await (await basicPage.request.get('/api/v1/admin/session')).json()
    expect(basicSession.can_view_identity).toBe(false)
    expect(basicSession.can_manage_permissions).toBe(false)
    await basicPage.goto(`/admin/pronajmy/${rental.id}`)
    await expect(basicPage.getByText('Pro zobrazení kopie dokladu nemáte oprávnění.')).toBeVisible()
    await expect(basicPage.getByRole('button', { name: 'Zobrazit kopii dokladu' })).toHaveCount(0)
    expect((await basicPage.request.post(`/api/v1/admin/rentals/${rental.id}/identity`, {
      headers: { 'x-csrf-token': basicSession.csrf_token }, data: { password: newPassword },
    })).status()).toBe(403)
    expect((await basicPage.request.put(`/api/v1/admin/accounts/${basic.id}/permissions`, {
      headers: { 'x-csrf-token': basicSession.csrf_token }, data: { current_password: newPassword, can_view_identity: true, can_manage_permissions: true, can_create_accounts: true, can_manage_mail_settings: true },
    })).status()).toBe(403)
  } finally {
    await basicContext.close()
  }
  const creatorContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const creatorPage = await creatorContext.newPage()
    await login(creatorPage, creatorEmail, newPassword)
    await creatorPage.goto('/admin/ucty')
    const form = creatorPage.locator('section').filter({ has: creatorPage.getByRole('heading', { name: 'Nový správce', exact: true }) })
    await expect(form.getByLabel('Správa rezervací a přístup k dokladům')).toBeDisabled()
    await expect(form.getByLabel('Správa oprávnění ostatních správců')).toBeDisabled()
    await expect(form.getByLabel('Změna nastavení odesílacího účtu')).toBeDisabled()
    await expect(form.getByLabel('Vytváření účtů správců')).toBeEnabled()
    const creatorSession = await (await creatorPage.request.get('/api/v1/admin/session')).json()
    const attempt = await creatorPage.request.post('/api/v1/admin/accounts', { headers: { 'x-csrf-token': creatorSession.csrf_token }, data: {
      email: `nepovolene-${suffix}@vysker.test`, password: newPassword, current_password: newPassword,
      can_view_identity: true, can_create_accounts: false, can_manage_permissions: false, can_manage_mail_settings: false,
    } })
    expect(attempt.status()).toBe(403)
  } finally {
    await creatorContext.close()
  }
  const managerContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
  try {
    const managerPage = await managerContext.newPage()
    await login(managerPage, managerEmail, newPassword)
    await managerPage.goto('/admin/ucty')
    const target = managerPage.locator('details.admin-account').filter({ hasText: qualifiedEmail })
    await target.locator('summary').click()
    await expect(target.getByLabel('Správa rezervací a přístup k dokladům')).toBeChecked()
    await expect(target.getByLabel('Správa rezervací a přístup k dokladům')).toBeDisabled()
    await expect(target.getByLabel('Vytváření účtů správců')).toBeDisabled()
    await expect(target.getByLabel('Změna nastavení odesílacího účtu')).toBeDisabled()
    await target.getByLabel('Správa oprávnění ostatních správců').check()
    await target.getByLabel('Vaše současné heslo pro potvrzení').fill(newPassword)
    const saved = managerPage.waitForResponse(r => r.url().endsWith(`/api/v1/admin/accounts/${qualified.id}/permissions`) && r.request().method() === 'PUT')
    await target.getByRole('button', { name: 'Uložit oprávnění' }).click()
    expect((await saved).status()).toBe(204)
    const updated = await (await managerPage.request.get('/api/v1/admin/accounts')).json()
    expect(updated.find((a: { email: string }) => a.email === qualifiedEmail)).toMatchObject({ can_view_identity: true, can_manage_permissions: true })
  } finally {
    await managerContext.close()
  }
})
