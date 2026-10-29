import { test, expect, type Page } from './fixtures'

type NavigationItem = {
  id: number
  label: string
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

async function navigationItems(page: Page): Promise<NavigationItem[]> {
  const response = await page.request.get('/api/v1/admin/navigation')
  expect(response.status()).toBe(200)
  return response.json()
}

async function mutationHeaders(page: Page) {
  const response = await page.request.get('/api/v1/admin/session')
  expect(response.status()).toBe(200)
  const session = await response.json()
  return {
    'X-CSRF-Token': session.csrf_token,
    Origin: new URL(page.url()).origin,
  }
}

async function createItem(page: Page, label: string, path: string, parentId: number | null = null, order = 1) {
  const response = await page.request.post('/api/v1/admin/navigation', {
    headers: await mutationHeaders(page),
    data: { label, path, parent_id: parentId, sort_order: order, visible: true },
  })
  expect(response.status()).toBe(201)
  return (await response.json()).id as number
}

async function cleanUpItems(page: Page, marker: string) {
  const items = (await navigationItems(page)).filter(item => item.label.includes(marker))
  const headers = await mutationHeaders(page)
  // Children must be removed before their parent, using their current versions.
  items.sort((left, right) => Number(right.parent_id !== null) - Number(left.parent_id !== null))
  for (const item of items) {
    const response = await page.request.delete(`/api/v1/admin/navigation/${item.id}`, {
      headers,
      data: { expected_version: item.version },
    })
    expect(response.status()).toBe(204)
  }
}

function adminRow(page: Page, label: string) {
  return page.locator('.admin-navigation-row').filter({
    has: page.getByText(label, { exact: true }),
  })
}

test('municipality address shows office contacts and old contact links keep query and anchor', async ({ page, browser }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto('/obec')
  await expect(page.getByRole('heading', { name: 'Kontakt', exact: true })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Obecní úřad Vyskeř', exact: true })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Úřední hodiny', exact: true })).toBeVisible()
  await expect(page.getByText('Obec, ve které žijeme', { exact: true })).toHaveCount(0)
  await expect(page.locator('header a.contact-link')).toHaveAttribute('href', '/obec')
  await expect(page.getByRole('navigation', { name: 'Patička' }).getByRole('link', { name: 'Kontakt', exact: true })).toHaveAttribute('href', '/obec')

  for (const path of ['/kontakt', '/stranky/kontakt']) {
    const response = await page.request.get(`${path}?zdroj=test`, { maxRedirects: 0 })
    expect(response.status()).toBe(301)
    expect(response.headers().location).toBe('/obec?zdroj=test')
    await page.goto(`${path}?zdroj=test#uredni-hodiny`)
    await expect(page).toHaveURL(/\/obec\?zdroj=test#uredni-hodiny$/)
    await expect(page.locator('#uredni-hodiny')).toBeVisible()
  }

  await page.goto('/')
  await page.getByRole('link', { name: 'Úřední hodiny', exact: true }).click()
  await expect(page).toHaveURL(/\/obec#uredni-hodiny$/)
  await expect(page.locator('#uredni-hodiny')).toBeVisible()

  const context = await browser.newContext({ javaScriptEnabled: false })
  try {
    const publicPage = await context.newPage()
    await publicPage.goto(new URL('/kontakt#uredni-hodiny', page.url()).href)
    await expect(publicPage).toHaveURL(/\/obec#uredni-hodiny$/)
    await expect(publicPage.getByRole('heading', { name: 'Obecní úřad Vyskeř', exact: true })).toBeVisible()
  } finally {
    await context.close()
  }
  expect(errors).toEqual([])
})

test('administrator creates a submenu, changes its placement and sees the saved hierarchy publicly', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const marker = Date.now().toString()
  const parent = `Rozcestník ${marker}`
  const child = `Kontakty ${marker}`
  await login(page)
  try {
    await page.goto('/admin/navigace')
    await page.getByRole('button', { name: 'Přidat položku', exact: true }).click()
    const placement = page.getByLabel('Umístění v menu', { exact: false })
    await expect(placement.locator('option:checked')).toHaveText('Hlavní menu')
    await page.getByLabel('Název', { exact: false }).fill(parent)
    await page.getByLabel('Adresa odkazu', { exact: false }).fill('/obec')
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(adminRow(page, parent)).toBeVisible()
    const parentId = (await navigationItems(page)).find(item => item.label === parent)!.id

    await adminRow(page, parent).getByRole('button', { name: `Přidat podpoložku pod ${parent}`, exact: true }).click()
    await expect(placement).toHaveValue(String(parentId))
    await page.getByLabel('Název', { exact: false }).fill(child)
    await page.getByLabel('Adresa odkazu', { exact: false }).fill('/kontakt')
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(adminRow(page, child)).toBeVisible()
    expect((await navigationItems(page)).find(item => item.label === child)!.parent_id).toBe(parentId)

    await page.reload()
    await adminRow(page, child).getByRole('button', { name: `Upravit položku ${child}`, exact: true }).click()
    await expect(placement).toHaveValue(String(parentId))
    await placement.selectOption({ label: 'Hlavní menu' })
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(placement).toHaveCount(0)
    expect((await navigationItems(page)).find(item => item.label === child)!.parent_id).toBeNull()

    await adminRow(page, child).getByRole('button', { name: `Upravit položku ${child}`, exact: true }).click()
    await expect(placement.locator('option:checked')).toHaveText('Hlavní menu')
    await placement.selectOption({ label: parent })
    await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
    await expect(placement).toHaveCount(0)
    expect((await navigationItems(page)).find(item => item.label === child)!.parent_id).toBe(parentId)

    await page.setViewportSize({ width: 1440, height: 960 })
    await page.goto('/')
    const nav = page.getByRole('navigation', { name: 'Hlavní navigace', exact: true })
    await expect(nav.getByRole('link', { name: parent, exact: true })).toHaveAttribute('href', '/obec')
    const childLink = nav.getByRole('link', { name: child, exact: true, includeHidden: true })
    await expect(childLink).toBeHidden()
    await nav.getByRole('button', { name: `Podmenu: ${parent}`, exact: true }).click()
    await expect(childLink).toBeVisible()
    await childLink.click()
    await expect(page).toHaveURL(/\/obec$/)
    await expect(page.getByRole('heading', { name: 'Kontakt', exact: true })).toBeVisible()
    await page.goBack()
    await expect(page).toHaveURL(/\/$/)
    expect(errors).toEqual([])
  } finally {
    await cleanUpItems(page, marker)
  }
})

test('desktop submenus support keyboard, a single open group, outside clicks and current-page selection', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const marker = Date.now().toString()
  const parent = `Obec ${marker}`
  const otherParent = `Služby ${marker}`
  const home = `Úvod ${marker}`
  const contact = `Spojení ${marker}`
  await login(page)
  try {
    const parentId = await createItem(page, parent, '/obec')
    await createItem(page, home, '/', parentId, 1)
    await createItem(page, contact, '/kontakt', parentId, 2)
    const otherParentId = await createItem(page, otherParent, '/dokumenty', null, 2)
    await createItem(page, `Listiny ${marker}`, '/dokumenty', otherParentId)
    await page.setViewportSize({ width: 1440, height: 960 })
    await page.goto('/')
    const nav = page.getByRole('navigation', { name: 'Hlavní navigace', exact: true })
    const toggle = nav.getByRole('button', { name: `Podmenu: ${parent}`, exact: true })
    const otherToggle = nav.getByRole('button', { name: `Podmenu: ${otherParent}`, exact: true })
    const homeLink = nav.getByRole('link', { name: home, exact: true, includeHidden: true })
    const contactLink = nav.getByRole('link', { name: contact, exact: true, includeHidden: true })
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    const submenuId = await toggle.getAttribute('aria-controls')
    expect(submenuId).toBeTruthy()
    const submenu = nav.locator(`ul[id="${submenuId}"]`)
    await expect(submenu).toHaveClass(/nav-submenu/)
    await expect(submenu).toBeHidden()
    await toggle.focus()
    await page.keyboard.press('Enter')
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')
    await expect(submenu).toBeVisible()
    await page.keyboard.press('Tab')
    await expect(homeLink).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(contactLink).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(submenu).toBeHidden()
    await expect(toggle).toBeFocused()
    await page.keyboard.press('Space')
    await expect(submenu).toBeVisible()

    await otherToggle.click()
    await expect(otherToggle).toHaveAttribute('aria-expanded', 'true')
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await page.locator('main').click({ position: { x: 5, y: 5 } })
    await expect(otherToggle).toHaveAttribute('aria-expanded', 'false')

    await toggle.click()
    await homeLink.click()
    await expect(page).toHaveURL(/\/$/)
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await expect(homeLink).toBeHidden()
    await expect(toggle).toBeFocused()
    await toggle.click()
    await nav.getByRole('link', { name: parent, exact: true }).click()
    await expect(page).toHaveURL(/\/obec$/)
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')

    for (const width of [1440, 1051]) {
      await page.setViewportSize({ width, height: 960 })
      await page.goto('/')
      await toggle.click()
      await expect(submenu).toBeVisible()
      const submenuBounds = await submenu.boundingBox()
      const childBounds = await contactLink.boundingBox()
      expect(submenuBounds).not.toBeNull()
      expect(childBounds).not.toBeNull()
      expect(submenuBounds!.x).toBeGreaterThanOrEqual(0)
      expect(submenuBounds!.x + submenuBounds!.width).toBeLessThanOrEqual(width)
      expect(childBounds!.y).toBeGreaterThanOrEqual(submenuBounds!.y)
      expect(childBounds!.y + childBounds!.height).toBeLessThanOrEqual(submenuBounds!.y + submenuBounds!.height)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
      // Visibility alone does not detect clipping by the navigation container.
      expect(await contactLink.evaluate(link => {
        const bounds = link.getBoundingClientRect()
        return document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)?.closest('a') === link
      })).toBe(true)
      // Full-page capture temporarily changes the viewport near the mobile breakpoint.
      // Keep the real viewport for the subsequent coordinate click.
      await page.screenshot({ path: test.info().outputPath(`submenu-desktop-${width}.png`) })
      await page.mouse.click(childBounds!.x + childBounds!.width / 2, childBounds!.y + childBounds!.height / 2)
      await expect(page).toHaveURL(/\/obec$/)
      await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    }
    expect(errors).toEqual([])
  } finally {
    await cleanUpItems(page, marker)
  }
})

test('mobile submenus stay inline and usable in both themes and close after selecting a link', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const marker = Date.now().toString()
  const parent = `Informace ${marker}`
  const home = `Domů ${marker}`
  const contact = `Obecní kontakty ${marker}`
  await login(page)
  try {
    const parentId = await createItem(page, parent, '/obec')
    await createItem(page, home, '/', parentId, 1)
    await createItem(page, contact, '/kontakt', parentId, 2)
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/')
    const menu = page.getByRole('button', { name: 'Hlavní menu', exact: true })
    const nav = page.getByRole('navigation', { name: 'Mobilní navigace', exact: true, includeHidden: true })
    const toggle = nav.getByRole('button', { name: `Podmenu: ${parent}`, exact: true, includeHidden: true })
    const homeLink = nav.getByRole('link', { name: home, exact: true, includeHidden: true })
    const contactLink = nav.getByRole('link', { name: contact, exact: true, includeHidden: true })
    await expect(menu).toHaveAttribute('aria-expanded', 'false')
    for (const theme of ['light', 'dark']) {
      await page.getByRole('button', { name: theme === 'light' ? 'Světlý motiv' : 'Tmavý motiv', exact: true }).click()
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
      await menu.click()
      await expect(nav).toBeVisible()
      await expect(contactLink).toBeHidden()
      await toggle.click()
      await expect(contactLink).toBeVisible()
      const parentBounds = await toggle.boundingBox()
      const childBounds = await contactLink.boundingBox()
      expect(parentBounds).not.toBeNull()
      expect(childBounds).not.toBeNull()
      expect(parentBounds!.height).toBeGreaterThanOrEqual(44)
      expect(childBounds!.height).toBeGreaterThanOrEqual(44)
      expect(childBounds!.y).toBeGreaterThanOrEqual(parentBounds!.y + parentBounds!.height)
      expect(childBounds!.x).toBeGreaterThanOrEqual(0)
      expect(childBounds!.x + childBounds!.width).toBeLessThanOrEqual(390)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
      await page.screenshot({ path: test.info().outputPath(`submenu-mobile-${theme}.png`), fullPage: true })
      await homeLink.click()
      await expect(page).toHaveURL(/\/$/)
      await expect(menu).toHaveAttribute('aria-expanded', 'false')
      await expect(nav).toBeHidden()
      await expect(menu).toBeFocused()
    }
    await menu.click()
    await toggle.click()
    await contactLink.click()
    await expect(page).toHaveURL(/\/obec$/)
    await expect(menu).toHaveAttribute('aria-expanded', 'false')
    await expect(nav).toBeHidden()
    expect(errors).toEqual([])
  } finally {
    await cleanUpItems(page, marker)
  }
})

test('server-rendered submenus work without JavaScript and mark only the exact current page', async ({ page, browser }) => {
  const marker = Date.now().toString()
  const parent = `Stránky ${marker}`
  const child = `Spolek ${marker}`
  const title = `Zkušební spolek ${marker}`
  const slug = `browser-menu-${marker}`
  await login(page)
  const origin = new URL(page.url()).origin
  const publicContext = await browser.newContext({ javaScriptEnabled: false })
  const publicPage = await publicContext.newPage()
  const errors: string[] = []
  publicPage.on('pageerror', error => errors.push(error.message))
  try {
    const response = await page.request.post('/api/v1/admin/pages', {
      headers: await mutationHeaders(page),
      data: { slug, title, content: 'Veřejná stránka dostupná i bez JavaScriptu.', published: true },
    })
    expect(response.status()).toBe(201)
    const parentId = await createItem(page, parent, '/stranky')
    await createItem(page, child, `/stranky/${slug}`, parentId)

    for (const width of [1440, 390]) {
      await publicPage.setViewportSize({ width, height: 960 })
      await publicPage.goto(`${origin}/stranky/${slug}`)
      await expect(publicPage.getByRole('heading', { name: title, exact: true })).toBeVisible()
      const nav = publicPage.getByRole('navigation', {
        name: width === 1440 ? 'Hlavní navigace' : 'Mobilní navigace',
        exact: true,
        includeHidden: true,
      })
      const parentLink = nav.getByRole('link', { name: parent, exact: true, includeHidden: true })
      const childLink = nav.getByRole('link', { name: child, exact: true, includeHidden: true })
      await expect(parentLink).toBeVisible()
      await expect(childLink).toBeVisible()
      await expect(parentLink).not.toHaveAttribute('aria-current', 'page')
      await expect(childLink).toHaveAttribute('aria-current', 'page')
      await expect(nav.locator('.nav-item').filter({
        has: publicPage.getByRole('link', { name: parent, exact: true, includeHidden: true }),
      })).toHaveClass(/nav-section-active/)
      await expect(nav.getByRole('button', { name: `Podmenu: ${parent}`, exact: true, includeHidden: true })).toBeHidden()
      expect(await publicPage.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)

      await parentLink.click()
      await expect(publicPage).toHaveURL(`${origin}/stranky`)
      await expect(parentLink).toHaveAttribute('aria-current', 'page')
      await expect(childLink).not.toHaveAttribute('aria-current', 'page')
      await childLink.click()
      await expect(publicPage).toHaveURL(`${origin}/stranky/${slug}`)
      await expect(publicPage.getByRole('heading', { name: title, exact: true })).toBeVisible()
    }
    expect(errors).toEqual([])
  } finally {
    await publicContext.close()
    await cleanUpItems(page, marker)
  }
})
