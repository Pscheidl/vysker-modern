import { test, expect } from './fixtures'

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
      await expect(page.getByRole('heading', { name: 'Source evidence and uncertainty' })).toBeVisible()
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    }
  }
  const openapi = page.locator('article').getByRole('link', { name: 'OpenAPI description', exact: true })
  await openapi.focus()
  await expect(openapi).toBeFocused()
  await openapi.press('Enter')
  await expect(page).toHaveURL(/\/openapi\.json$/)
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
