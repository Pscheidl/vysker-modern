import { test, expect, type Page } from '@playwright/test'

const PASSWORD = 'Disposable-browser-test-password!'
const environment = {
  provider: 'environment',
  username: '',
  sender_name: '',
  password_configured: false,
  smtp_host: 'mailpit',
  smtp_port: 1025,
  smtp_tls: 'none',
  email_from: 'Obec Vyskeř <obec@vysker.test>',
}
const google = {
  provider: 'google',
  username: 'obec@example.test',
  sender_name: 'Obec Vyskeř',
  password_configured: true,
  smtp_host: 'smtp.gmail.com',
  smtp_port: 587,
  smtp_tls: 'starttls',
  email_from: 'Obec Vyskeř <obec@example.test>',
}

async function openSettings(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  await page.getByRole('link', { name: 'Rozesílání', exact: true }).click()
  await page.getByRole('link', { name: 'Nastavení odesílání', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Nastavení odesílání', exact: true })).toBeVisible()
}

test('Google settings require an app password and only saved settings can be tested', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const saves: any[] = []
  const tests: any[] = []
  // Mock only mail settings so this browser test cannot contact a Google account.
  await page.route('**/api/v1/admin/mail-settings', async route => {
    if (route.request().method() === 'PUT') {
      saves.push(route.request().postDataJSON())
      await route.fulfill({ json: google })
    } else {
      await route.fulfill({ json: environment })
    }
  })
  await page.route('**/api/v1/admin/mail-settings/test', async route => {
    tests.push(route.request().postDataJSON())
    await route.fulfill({ json: { recipient: 'e2e-admin@vysker.test' } })
  })
  await openSettings(page)
  const provider = page.getByRole('combobox', { name: 'Způsob odesílání', exact: true })
  const save = page.getByRole('button', { name: 'Uložit nastavení', exact: true })
  const send = page.getByRole('button', { name: 'Odeslat zkušební e-mail', exact: true })
  await expect(save).toBeDisabled()
  await provider.selectOption('google')
  await expect(send).toBeDisabled()
  await expect(page.getByRole('link', { name: 'Vytvořit heslo aplikace u Googlu' })).toHaveAttribute('href', 'https://myaccount.google.com/apppasswords')
  await page.getByLabel('E-mail účtu Google', { exact: true }).fill(google.username)
  await page.getByLabel('Jméno odesílatele', { exact: true }).fill(google.sender_name)
  const appPassword = page.getByLabel('Heslo aplikace Google', { exact: true })
  await expect(appPassword).toHaveAttribute('type', 'password')
  await expect(appPassword).toHaveAttribute('required', '')
  await page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true }).fill(PASSWORD)
  await save.click()
  expect(saves).toHaveLength(0)
  await appPassword.fill('abcd efgh ijkl mnop')
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('link', { name: 'Zpět na rozesílání', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/posta\/nastaveni$/)
  await save.click()
  await expect(page.locator('.admin-flash')).toContainText('Nastavení odesílání bylo uloženo')
  expect(saves).toEqual([{
    provider: 'google',
    username: google.username,
    sender_name: google.sender_name,
    password: 'abcd efgh ijkl mnop',
    current_password: PASSWORD,
  }])
  await expect(appPassword).toHaveValue('')
  await expect(page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true })).toHaveValue('')
  await expect(save).toBeDisabled()
  await expect(send).toBeEnabled()
  await send.click()
  expect(tests).toHaveLength(0)
  await page.getByLabel('Vaše heslo do administrace pro test', { exact: true }).fill(PASSWORD)
  await send.click()
  await expect(page.getByRole('status').filter({ hasText: 'SMTP server přijal zkušební e-mail' })).toContainText('e2e-admin@vysker.test')
  expect(tests).toEqual([{ current_password: PASSWORD }])
  await expect(page.getByLabel('Vaše heslo do administrace pro test', { exact: true })).toHaveValue('')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(errors).toEqual([])
})

test('stored app passwords stay hidden and cannot be reused for another Google account', async ({ page }) => {
  const saves: any[] = []
  await page.route('**/api/v1/admin/mail-settings', async route => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON()
      saves.push(body)
      await route.fulfill({ json: body.provider === 'environment' ? environment : { ...google, sender_name: body.sender_name } })
    } else {
      await route.fulfill({ json: google })
    }
  })
  await openSettings(page)
  const appPassword = page.getByLabel('Heslo aplikace Google', { exact: true })
  const save = page.getByRole('button', { name: 'Uložit nastavení', exact: true })
  const username = page.getByLabel('E-mail účtu Google', { exact: true })
  await expect(appPassword).toHaveValue('')
  await expect(appPassword).not.toHaveAttribute('required', '')
  await expect(page.getByText('Heslo aplikace je uložené a nezobrazuje se.', { exact: false })).toBeVisible()
  await username.fill('jiny@example.test')
  await expect(appPassword).toHaveAttribute('required', '')
  await page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true }).fill(PASSWORD)
  await save.click()
  expect(saves).toHaveLength(0)
  await page.getByRole('button', { name: 'Zrušit změny', exact: true }).click()
  await expect(username).toHaveValue(google.username)
  await expect(save).toBeDisabled()
  await page.getByLabel('Jméno odesílatele', { exact: true }).fill('Nový název obce')
  await page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true }).fill(PASSWORD)
  await save.click()
  await expect(save).toBeDisabled()
  expect(saves[0].password).toBeNull()
  expect(saves[0].username).toBe(google.username)
  await page.getByRole('combobox', { name: 'Způsob odesílání', exact: true }).selectOption('environment')
  await expect(appPassword).toHaveCount(0)
  await expect(page.getByText('Uložením této volby se připojení k účtu Google odstraní.', { exact: false })).toBeVisible()
  await page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true }).fill(PASSWORD)
  await save.click()
  await expect(save).toBeDisabled()
  expect(saves[1]).toEqual({ provider: 'environment', username: '', sender_name: '', password: null, current_password: PASSWORD })
})

test('failed save preserves edits and a failed test does not report delivery', async ({ page }) => {
  await page.route('**/api/v1/admin/mail-settings', async route => {
    await route.fulfill(route.request().method() === 'PUT'
      ? { status: 403, json: { error: 'Současné heslo není správné.' } }
      : { json: google })
  })
  await page.route('**/api/v1/admin/mail-settings/test', async route => {
    await route.fulfill({ status: 502, json: { error: 'SMTP server zprávu nepřijal. Ověřte nastavení.' } })
  })
  await openSettings(page)
  await page.getByLabel('Jméno odesílatele', { exact: true }).fill('Neuložený název')
  await page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true }).fill('incorrect-password')
  await page.getByRole('button', { name: 'Uložit nastavení', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Současné heslo není správné.')
  await expect(page.getByLabel('Jméno odesílatele', { exact: true })).toHaveValue('Neuložený název')
  await expect(page.getByLabel('Vaše heslo do administrace pro uložení', { exact: true })).toHaveValue('')
  await expect(page.getByRole('button', { name: 'Odeslat zkušební e-mail', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Zrušit změny', exact: true }).click()
  await page.getByLabel('Vaše heslo do administrace pro test', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Odeslat zkušební e-mail', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('SMTP server zprávu nepřijal.')
  await expect(page.getByRole('status').filter({ hasText: 'SMTP server přijal zkušební e-mail' })).toHaveCount(0)
  await expect(page.getByLabel('Vaše heslo do administrace pro test', { exact: true })).toHaveValue('')
})
