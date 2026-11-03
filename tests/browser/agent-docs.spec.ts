import { test, expect, type Page } from './fixtures'

async function navigateToMachineDocument(page: Page, path: string, activate: () => Promise<unknown>) {
  const url = new URL(path, page.url()).href
  const [response] = await Promise.all([
    page.waitForResponse(response => {
      const request = response.request()
      return response.url() === url && request.isNavigationRequest()
        && request.resourceType() === 'document' && request.frame() === page.mainFrame()
    }),
    activate(),
  ])
  expect(response.status(), `${path} must return an actual server document`).toBe(200)
  return response
}

test('agent guidance remains readable in both themes and keyboard navigation', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()) })
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme })
    for (const width of [390, 1440]) {
      await page.setViewportSize({ width, height: 900 })
      await page.goto('/for-agents')
      await expect(page.getByRole('heading', { name: 'For agents', exact: true, level: 1 })).toBeVisible()
      await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
      await expect(page.getByRole('heading', { name: 'Source evidence and uncertainty' })).toBeVisible()
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    }
  }
  const openapi = page.locator('article').getByRole('link', { name: 'OpenAPI description', exact: true })
  await openapi.focus()
  await expect(openapi).toBeFocused()
  const response = await navigateToMachineDocument(page, '/openapi.json', () => openapi.press('Enter'))
  await expect(page).toHaveURL(/\/openapi\.json$/)
  expect(response.headers()['content-type']?.split(';')[0]).toBe('application/json')
  const specification = await response.json()
  expect(specification.openapi).toBe('3.1.0')
  expect(specification.paths['/api/v1/search'].get.operationId).toBe('searchPublicContent')
  expect(errors).toEqual([])
})

test('the English agent guide and its discovery links work without JavaScript on mobile', async ({ browser, baseURL }) => {
  const context = await browser.newContext({ baseURL, javaScriptEnabled: false, viewport: { width: 390, height: 844 } })
  const page = await context.newPage()
  await page.goto('/for-agents')
  await expect(page.getByRole('heading', { name: 'For agents', exact: true, level: 1 })).toBeVisible()
  await expect(page.locator('article[lang="en"]')).toContainText('experimental discovery proposal')
  await expect(page.locator('article[lang="en"]')).toContainText('do not count votes')
  await expect(page.locator('article[lang="en"]')).toContainText('Operating mode: preview')
  await expect(page.locator('head link[rel="describedby"]')).toHaveAttribute('href', '/llms.txt')
  await expect(page.locator('head link[rel="alternate"][type="text/markdown"]')).toHaveAttribute('href', '/for-agents.md')
  await expect(page.locator('.site-footer').getByRole('link', { name: 'For agents', exact: true })).toHaveAttribute('href', '/for-agents')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  const markdown = await context.request.get('/for-agents.md')
  expect(markdown.ok()).toBe(true)
  expect(markdown.headers()['content-type']).toContain('text/markdown')
  expect(await markdown.text()).toContain('read_attachment_text')
  await context.close()
})

for (const javaScriptEnabled of [true, false]) {
  test(`guide machine-readable links request server documents ${javaScriptEnabled ? 'after hydration' : 'without JavaScript'}`, async ({ browser, baseURL }) => {
    const context = await browser.newContext({ baseURL, javaScriptEnabled, viewport: { width: 390, height: 844 } })
    const page = await context.newPage()
    const documents = [
      { path: '/llms.txt', mediaType: 'text/plain' },
      { path: '/for-agents.md', mediaType: 'text/markdown' },
      { path: '/openapi.json', mediaType: 'application/json' },
      { path: '/mcp/server-card', mediaType: 'application/mcp-server-card+json' },
      { path: '/.well-known/ai-catalog.json', mediaType: 'application/ai-catalog+json' },
    ]
    try {
      for (const document of documents) {
        await test.step(document.path, async () => {
          await page.goto('/for-agents')
          await expect(page.getByRole('heading', { name: 'For agents', exact: true, level: 1 })).toBeVisible()
          if (javaScriptEnabled) await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
          const link = page.locator(`article[lang="en"] a[href="${document.path}"]`)
          // A client-side 404 can have the correct URL. Require a main-frame HTTP
          // response, including for MIME types Chromium handles as a download.
          const response = await navigateToMachineDocument(page, document.path, () => link.click())
          expect(response.headers()['content-type']?.split(';')[0]).toBe(document.mediaType)
          if (document.path === '/llms.txt') {
            await expect(page.locator('body')).toBeVisible()
            await expect(page.locator('body')).toContainText('# Vyskeř public information')
            await expect(page.locator('body')).toContainText('/mcp')
            await page.goBack()
            await expect(page.getByRole('heading', { name: 'For agents', exact: true, level: 1 })).toBeVisible()
          }
        })
      }
    } finally {
      await context.close()
    }
  })
}
