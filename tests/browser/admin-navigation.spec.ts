import { test, expect, type Page } from './fixtures'

const photograph = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg==', 'base64')
const diagnostics = new WeakMap<Page, {
  errors: string[]
  consoleErrors: string[]
  sessions: { method: string, status: number }[]
  documents: string[]
}>()
const publicPages = [
  { label: 'homepage', path: '/' },
  { label: 'report list', path: '/hlaseni-zavad' },
  { label: 'report form', path: '/hlaseni-zavad/nove' },
]

test.beforeEach(async ({ page }) => {
  const record = { errors: [] as string[], consoleErrors: [] as string[], sessions: [] as { method: string, status: number }[], documents: [] as string[] }
  diagnostics.set(page, record)
  page.on('pageerror', error => record.errors.push(error.message))
  page.on('console', message => {
    if (message.type() === 'error') record.consoleErrors.push(message.text())
  })
  page.on('response', response => {
    if (new URL(response.url()).pathname === '/api/v1/admin/session') {
      record.sessions.push({ method: response.request().method(), status: response.status() })
    }
  })
  page.on('request', request => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) {
      record.documents.push(new URL(request.url()).pathname)
    }
  })
  await page.route('https://tile.openstreetmap.org/**', route => route.fulfill({ contentType: 'image/png', body: photograph }))
  await page.route('**/api/v1/report-addresses?*', route => route.fulfill({ json: [] }))
})

test.afterEach(async ({ page }, testInfo) => {
  const record = diagnostics.get(page)!
  await testInfo.attach('administration-navigation-diagnostics', {
    body: JSON.stringify(record, null, 2), contentType: 'application/json',
  })
  expect.soft(record.errors, 'Crossing between the public site and administration must not panic').toEqual([])
  expect.soft(record.documents, 'Layout transitions must stay in the current document').toHaveLength(1)
})

async function openPublicPage(page: Page, path: string) {
  await page.goto(path)
  // The regression occurs when the hydrated public navigation is disposed.
  await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
  if (path === '/hlaseni-zavad/nove') {
    await expect(page.getByRole('button', { name: 'Odeslat hlášení', exact: true })).toBeEnabled()
  }
  const heading = await page.getByRole('main').getByRole('heading', { level: 1 }).textContent()
  expect(heading).toBeTruthy()
  await page.evaluate(() => Reflect.set(window, 'adminNavigationMarker', 'original-document'))
  return heading!
}

async function clickAdministration(page: Page) {
  await page.getByRole('navigation', { name: 'Patička', exact: true })
    .getByRole('link', { name: 'Správa webu', exact: true }).click()
  await expect(page).toHaveURL('http://127.0.0.1:3107/admin')
}

async function expectLogin(page: Page) {
  await expect(page.getByRole('heading', { name: 'Dobrý den.', exact: true })).toBeVisible()
  await expect(page.getByLabel('E-mail', { exact: true })).toBeEditable()
  await expect(page.getByLabel('Heslo', { exact: true })).toBeEditable()
  await expect(page.getByRole('button', { name: 'Přihlásit se', exact: true })).toBeEnabled()
  await expect(page.locator('.site-header')).toHaveCount(0)
  await expect(page.getByRole('navigation', { name: 'Administrace', exact: true })).toHaveCount(0)
  expect(await page.evaluate(() => Reflect.get(window, 'adminNavigationMarker'))).toBe('original-document')
}

async function expectDashboard(page: Page) {
  await expect(page.getByRole('navigation', { name: 'Administrace', exact: true })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Vše důležité na jednom místě.', exact: true })).toBeVisible()
  await expect(page.getByLabel('Heslo', { exact: true })).toHaveCount(0)
  await expect(page.locator('.site-header')).toHaveCount(0)
  expect(await page.evaluate(() => Reflect.get(window, 'adminNavigationMarker'))).toBe('original-document')
}

test('a resident can cancel leaving a draft through the administration link and then confirm the transition', async ({ page }) => {
  await openPublicPage(page, '/hlaseni-zavad/nove')
  const description = page.getByLabel('Popis problému', { exact: false })
  await description.fill('Rozpracované hlášení před otevřením správy webu')
  let allowDeparture = false
  let prompts = 0
  page.on('dialog', async dialog => {
    prompts += 1
    expect(dialog.type()).toBe('confirm')
    expect(dialog.message()).toContain('Máte rozpracované hlášení.')
    if (allowDeparture) await dialog.accept()
    else await dialog.dismiss()
  })
  await page.getByRole('navigation', { name: 'Patička', exact: true })
    .getByRole('link', { name: 'Správa webu', exact: true }).click()
  expect(prompts).toBe(1)
  await expect(page).toHaveURL('http://127.0.0.1:3107/hlaseni-zavad/nove')
  await expect(description).toHaveValue('Rozpracované hlášení před otevřením správy webu')
  allowDeparture = true
  await clickAdministration(page)
  await expectLogin(page)
  expect(prompts).toBe(2)
})

for (const source of publicPages) {
  test(`signed-out visitor opens administration from the ${source.label} footer and follows browser history`, async ({ page }) => {
    const publicHeading = await openPublicPage(page, source.path)
    await clickAdministration(page)
    await expectLogin(page)
    expect(diagnostics.get(page)!.sessions).toContainEqual({ method: 'GET', status: 401 })
    await page.goBack()
    await expect(page).toHaveURL(`http://127.0.0.1:3107${source.path}`)
    await expect(page.getByRole('main').getByRole('heading', { level: 1 })).toHaveText(publicHeading)
    await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
    await page.goForward()
    await expectLogin(page)
    await page.getByRole('link', { name: 'Zpět na web obce', exact: true }).click()
    await expect(page).toHaveURL('http://127.0.0.1:3107/')
    await expect(page.locator('.site-header')).toBeVisible()
    await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
    await expect(page.getByLabel('Heslo', { exact: true })).toHaveCount(0)
    // Re-enter after returning to the public layout to catch stale subscriptions.
    await clickAdministration(page)
    await expectLogin(page)
  })

  test(`signed-in administrator opens the dashboard from the ${source.label} footer without reloading`, async ({ page }) => {
    const login = await page.request.post('/api/v1/admin/login', {
      headers: { Origin: 'http://127.0.0.1:3107' },
      data: { email: 'e2e-admin@vysker.test', password: 'Disposable-browser-test-password!' },
    })
    expect(login.status()).toBe(200)
    const publicHeading = await openPublicPage(page, source.path)
    await clickAdministration(page)
    await expectDashboard(page)
    expect(diagnostics.get(page)!.sessions).toContainEqual({ method: 'GET', status: 200 })
    await page.goBack()
    await expect(page).toHaveURL(`http://127.0.0.1:3107${source.path}`)
    await expect(page.getByRole('main').getByRole('heading', { level: 1 })).toHaveText(publicHeading)
    await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
    await page.goForward()
    await expectDashboard(page)
    await page.getByRole('button', { name: 'Odhlásit se', exact: true }).click()
    await expectLogin(page)
    await page.getByRole('link', { name: 'Zpět na web obce', exact: true }).click()
    await expect(page).toHaveURL('http://127.0.0.1:3107/')
    await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
    await clickAdministration(page)
    await expectLogin(page)
  })
}
