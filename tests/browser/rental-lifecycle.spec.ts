import { test, expect } from './fixtures'

test('leaving reservations while permissions are loading does not crash the application', async ({ page }) => {
  const errors: string[] = []
  const consoleErrors: string[] = []
  page.on('pageerror', error => errors.push(error.stack || error.message))
  page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()) })
  await page.goto('/')
  await page.getByRole('link', { name: 'Správa webu', exact: true }).click()
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()

  let release!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  let held = false
  await page.route('**/api/v1/admin/session', async route => {
    held = true
    await gate
    await route.continue().catch(() => {})
  })
  try {
    await page.getByRole('link', { name: 'Pronájmy', exact: true }).click()
    await expect.poll(() => held).toBe(true)
    await page.goBack()
    await expect(page).toHaveURL(/\/admin\/?$/)
    await page.goBack()
    await expect(page).toHaveURL(/\/$/)
  } finally {
    release()
  }
  await page.waitForTimeout(500)
  expect({ errors, consoleErrors: errors.length ? consoleErrors : [] }).toEqual({ errors: [], consoleErrors: [] })
})
