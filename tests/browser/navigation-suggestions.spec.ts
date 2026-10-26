import { test, expect, type Page } from './fixtures'

type TestPage = {
  id: number
  slug: string
  title: string
  content: string
  published: boolean
  version: number
}

type NavigationItem = {
  id: number
  label: string
  path: string
  parent_id: number | null
  version: number
}

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

async function mutationHeaders(page: Page) {
  const response = await page.request.get('/api/v1/admin/session')
  expect(response.status()).toBe(200)
  const session = await response.json()
  return { 'X-CSRF-Token': session.csrf_token, Origin: new URL(page.url()).origin }
}

async function createPage(page: Page, slug: string, title: string, published: boolean): Promise<TestPage> {
  const data = { slug, title, content: 'Obsah zkušební stránky.', published }
  const response = await page.request.post('/api/v1/admin/pages', {
    headers: await mutationHeaders(page),
    data,
  })
  expect(response.status()).toBe(201)
  return { ...data, id: (await response.json()).id, version: 1 }
}

async function setPublished(page: Page, fixture: TestPage, published: boolean) {
  const response = await page.request.put(`/api/v1/admin/pages/${fixture.id}`, {
    headers: await mutationHeaders(page),
    data: {
      slug: fixture.slug,
      title: fixture.title,
      content: fixture.content,
      published,
      expected_version: fixture.version,
    },
  })
  expect(response.status()).toBe(204)
  fixture.published = published
  fixture.version += 1
}

async function navigationItems(page: Page): Promise<NavigationItem[]> {
  const response = await page.request.get('/api/v1/admin/navigation')
  expect(response.status()).toBe(200)
  return response.json()
}

async function cleanUp(page: Page, marker: string, pages: TestPage[]) {
  const items = (await navigationItems(page)).filter(item => item.label.includes(marker))
  items.sort((left, right) => Number(right.parent_id !== null) - Number(left.parent_id !== null))
  const headers = await mutationHeaders(page)
  for (const item of items) {
    const response = await page.request.delete(`/api/v1/admin/navigation/${item.id}`, {
      headers,
      data: { expected_version: item.version },
    })
    expect(response.status()).toBe(204)
  }
  // Preserve immutable page history. The server removes the disposable database after the suite.
  for (const fixture of pages) {
    if (fixture.published) await setPublished(page, fixture, false)
  }
}

function adminRow(page: Page, label: string) {
  return page.locator('.admin-navigation-row').filter({ has: page.getByText(label, { exact: true }) })
}

test('navigation suggests published pages by title without accents and saves the selected link', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const marker = Date.now().toString()
  const pages: TestPage[] = []
  await login(page)
  try {
    const published = await createPage(page, `suggestion-public-${marker}`, `Žluťoučký spolek ${marker}`, true)
    pages.push(published)
    const draft = await createPage(page, `suggestion-draft-${marker}`, `Žluťoučký koncept ${marker}`, false)
    pages.push(draft)
    await page.goto('/admin/navigace')
    await page.getByRole('button', { name: 'Přidat položku', exact: true }).click()
    const address = page.getByRole('combobox', { name: 'Adresa odkazu', exact: false })
    const list = page.getByRole('listbox', { name: 'Zveřejněné stránky', exact: true })
    await address.fill('ZLUTOUCKY')
    await expect(list.getByRole('option').filter({ hasText: published.title })).toBeVisible()
    await expect(list.getByRole('option').filter({ hasText: draft.title })).toHaveCount(0)
    await list.getByRole('option').filter({ hasText: published.title }).click()
    await expect(address).toHaveValue(`/stranky/${published.slug}`)
    await expect(page.getByLabel('Název', { exact: false })).toHaveValue(published.title)
    await expect(address).toHaveAttribute('aria-expanded', 'false')
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(adminRow(page, published.title)).toBeVisible()
    expect((await navigationItems(page)).find(item => item.label === published.title)?.path).toBe(`/stranky/${published.slug}`)
    expect(errors).toEqual([])
  } finally {
    await cleanUp(page, marker, pages)
  }
})

test('submenu suggestions support keyboard selection, preserve custom names and keep manual links usable', async ({ page }) => {
  const marker = Date.now().toString()
  const pages: TestPage[] = []
  await login(page)
  try {
    const published = await createPage(page, `suggestion-keyboard-${marker}`, `Obecní knihovna ${marker}`, true)
    pages.push(published)
    const parentLabel = `Rozcestník ${marker}`
    const response = await page.request.post('/api/v1/admin/navigation', {
      headers: await mutationHeaders(page),
      data: { label: parentLabel, path: '/obec', sort_order: 1, visible: true },
    })
    expect(response.status()).toBe(201)
    const parentId = (await response.json()).id
    await page.goto('/admin/navigace')
    await adminRow(page, parentLabel).getByRole('button', { name: `Přidat podpoložku pod ${parentLabel}`, exact: true }).click()
    const name = page.getByLabel('Název', { exact: false })
    const childLabel = `Knihy ${marker}`
    await name.fill(childLabel)
    const address = page.getByRole('combobox', { name: 'Adresa odkazu', exact: false })
    const list = page.getByRole('listbox', { name: 'Zveřejněné stránky', exact: true })
    await address.fill(`/stranky/${published.slug}`)
    await expect(list.getByRole('option')).toHaveCount(1)
    await address.press('Escape')
    await expect(address).toHaveAttribute('aria-expanded', 'false')
    await address.press('ArrowDown')
    await expect(address).toHaveAttribute('aria-expanded', 'true')
    await expect(list.getByRole('option')).toHaveAttribute('aria-selected', 'true')
    await address.press('Enter')
    await expect(name).toHaveValue(childLabel)
    await expect(address).toHaveValue(`/stranky/${published.slug}`)
    await expect(page.getByRole('heading', { name: 'Nová podpoložka', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(adminRow(page, childLabel)).toBeVisible()
    expect((await navigationItems(page)).find(item => item.label === childLabel)).toMatchObject({
      path: `/stranky/${published.slug}`,
      parent_id: parentId,
    })

    await adminRow(page, childLabel).getByRole('button', { name: `Upravit položku ${childLabel}`, exact: true }).click()
    await address.fill('knihovna')
    await expect(list.getByRole('option').filter({ hasText: published.title })).toBeVisible()
    await name.focus()
    await expect(address).toHaveAttribute('aria-expanded', 'false')
    await address.fill('/kontakt')
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(address).toHaveCount(0)
    expect((await navigationItems(page)).find(item => item.label === childLabel)).toMatchObject({
      label: childLabel,
      path: '/kontakt',
      parent_id: parentId,
    })
  } finally {
    await cleanUp(page, marker, pages)
  }
})

test('opening the navigation editor refreshes publication changes from another editor', async ({ page }) => {
  const marker = Date.now().toString()
  const pages: TestPage[] = []
  await login(page)
  try {
    const published = await createPage(page, `suggestion-refresh-${marker}`, `Nová stránka ${marker}`, true)
    pages.push(published)
    const draft = await createPage(page, `suggestion-new-${marker}`, `Další stránka ${marker}`, false)
    pages.push(draft)
    await page.goto('/admin/navigace')
    await expect(page.getByRole('button', { name: 'Přidat položku', exact: true })).toBeEnabled()
    await setPublished(page, published, false)
    await setPublished(page, draft, true)
    await page.getByRole('button', { name: 'Přidat položku', exact: true }).click()
    const address = page.getByRole('combobox', { name: 'Adresa odkazu', exact: false })
    const list = page.getByRole('listbox', { name: 'Zveřejněné stránky', exact: true })
    await address.fill(marker)
    await expect(list.getByRole('option').filter({ hasText: draft.title })).toBeVisible()
    await expect(list.getByRole('option').filter({ hasText: published.title })).toHaveCount(0)
  } finally {
    await cleanUp(page, marker, pages)
  }
})

test('mobile touch selection fits the screen and shortens a long page title to a valid menu name', async ({ page, browser }, testInfo) => {
  const marker = Date.now().toString()
  const pages: TestPage[] = []
  const errors: string[] = []
  await login(page)
  const context = await browser.newContext({
    baseURL: new URL(page.url()).origin,
    storageState: await page.context().storageState(),
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  })
  const mobile = await context.newPage()
  mobile.on('pageerror', error => errors.push(error.message))
  try {
    const published = await createPage(
      page,
      `suggestion-mobile-${marker}-knihovna-citarna-pro-obcany`,
      `${marker} Obecní knihovna s čítárnou pro všechny občany a návštěvníky obce`,
      true,
    )
    pages.push(published)
    await mobile.goto('/admin/navigace')
    await mobile.getByRole('button', { name: 'Přidat položku', exact: true }).tap()
    const address = mobile.getByRole('combobox', { name: 'Adresa odkazu', exact: false })
    await address.tap()
    await address.fill(marker)
    const option = mobile.getByRole('listbox', { name: 'Zveřejněné stránky', exact: true })
      .getByRole('option').filter({ hasText: published.title })
    await expect(option).toBeVisible()
    await option.scrollIntoViewIfNeeded()
    expect(await mobile.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    await mobile.screenshot({ path: testInfo.outputPath('navigation-suggestions-mobile.png'), fullPage: true })
    await option.tap()
    await expect(address).toHaveValue(`/stranky/${published.slug}`)
    const expectedName = Array.from(published.title).slice(0, 40).join('')
    await expect(mobile.getByLabel('Název', { exact: false })).toHaveValue(expectedName)
    await expect(address).toHaveAttribute('aria-expanded', 'false')
    await mobile.getByRole('button', { name: 'Uložit položku', exact: true }).tap()
    await expect(adminRow(mobile, expectedName)).toBeVisible()
    expect((await navigationItems(page)).find(item => item.label === expectedName)?.path).toBe(`/stranky/${published.slug}`)
    expect(await mobile.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    expect(errors).toEqual([])
  } finally {
    await context.close()
    await cleanUp(page, marker, pages)
  }
})
