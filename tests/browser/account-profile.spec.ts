import { test, expect } from './fixtures'

test('saving a public name preserves other account drafts and navigation protection', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  const original = await (await page.request.get('/api/v1/admin/session')).json()
  try {
    await page.getByRole('link', { name: 'Účty a hesla', exact: true }).click()
    const name = page.getByLabel('Výchozí jméno autora', { exact: true })
    const password = page.getByLabel('Současné heslo', { exact: true })
    const newEmail = page.getByLabel('E-mail nového správce *', { exact: true })
    const save = page.getByRole('button', { name: 'Uložit výchozí jméno', exact: true })
    await name.fill('Jana Nováková')
    await password.fill('Rozpracované heslo')
    await newEmail.fill('rozpracovany@example.test')
    await save.click()
    await expect(page.locator('.admin-flash')).toContainText('Výchozí jméno u komentářů bylo uloženo.')
    await expect(password).toHaveValue('Rozpracované heslo')
    await expect(newEmail).toHaveValue('rozpracovany@example.test')
    await expect(name).toHaveValue('Jana Nováková')
    let dialogs = 0
    page.on('dialog', async dialog => {
      dialogs += 1
      expect(dialog.message()).toContain('neuložené změny')
      await dialog.dismiss()
    })
    const overview = page.getByRole('link', { name: 'Přehled', exact: true })
    await overview.click()
    await expect(page).toHaveURL(/\/admin\/ucty$/)
    expect(dialogs).toBe(1)
    await password.fill('')
    await newEmail.fill('')
    await name.fill('Neuložené jméno')
    await overview.click()
    await expect(page).toHaveURL(/\/admin\/ucty$/)
    expect(dialogs).toBe(2)
    await page.route('**/api/v1/admin/profile', route => route.fulfill({ status: 503, json: { error: 'Uložení nyní není dostupné.' } }), { times: 1 })
    await save.click()
    await expect(page.getByRole('alert')).toContainText('Uložení nyní není dostupné.')
    await expect(name).toHaveValue('Neuložené jméno')
    await overview.click()
    await expect(page).toHaveURL(/\/admin\/ucty$/)
    expect(dialogs).toBe(3)
    await name.fill('')
    await save.click()
    await expect(page.getByRole('alert')).toHaveCount(0)
    await expect(save).toBeEnabled()
    await overview.click()
    await expect(page).toHaveURL(/\/admin$/)
    expect(dialogs).toBe(3)
    expect(errors).toEqual([])
  } finally {
    await page.request.put('/api/v1/admin/profile', {
      headers: { 'x-csrf-token': original.csrf_token },
      data: { display_name: original.display_name },
    }).catch(() => {})
  }
})
