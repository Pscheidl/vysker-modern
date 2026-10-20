import { test, expect } from '@playwright/test'

test('new pages bypass old frontend URLs and keep desktop navigation in a row', async ({ page }) => {
  const oldRequests: string[] = []
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  // Model a visitor retaining the former release at its unversioned URLs.
  // A fresh-browser-only test missed this when the navigation markup changed.
  await page.route(/\/pkg\/obecni-web\.(css|js|wasm)$/, async route => {
    oldRequests.push(route.request().url())
    await route.fulfill({
      contentType: route.request().url().endsWith('.css') ? 'text/css' : 'application/javascript',
      body: route.request().url().endsWith('.css')
        ? '.desktop-nav .nav-list { display: block !important }'
        : 'throw new Error("A previous release asset was reused")',
    })
  })
  await page.setViewportSize({ width: 1440, height: 960 })
  const response = await page.goto('/')
  expect(response?.headers()['cache-control']).toBe('no-store')
  const assets = [
    await page.locator('link#leptos').getAttribute('href'),
    await page.locator('link[rel="modulepreload"]').getAttribute('href'),
    await page.locator('link[rel="preload"][as="fetch"][type="application/wasm"]').getAttribute('href'),
  ]
  for (const [index, extension] of ['css', 'js', 'wasm'].entries()) {
    expect(assets[index]).toMatch(new RegExp(`^/pkg/obecni-web\\.[A-Za-z0-9_-]+\\.${extension}$`))
    const asset = await page.request.get(assets[index]!)
    expect(asset.status()).toBe(200)
    expect(asset.headers()['cache-control']).toBe('no-cache')
  }

  const list = page.locator('.desktop-nav .nav-list')
  await expect(list).toHaveCSS('display', 'flex')
  const links = list.locator(':scope > .nav-item > .nav-parent > a')
  expect(await links.count()).toBeGreaterThanOrEqual(3)
  const positions = await links.evaluateAll(elements =>
    elements.map(element => {
      const bounds = element.getBoundingClientRect()
      return { x: bounds.x, y: bounds.y }
    }),
  )
  expect(Math.max(...positions.map(p => p.y)) - Math.min(...positions.map(p => p.y))).toBeLessThan(2)
  for (let index = 1; index < positions.length; index++) {
    expect(positions[index].x).toBeGreaterThan(positions[index - 1].x)
  }
  await page.getByRole('button', { name: 'Tmavý motiv', exact: true }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('button', { name: 'Hlavní menu', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Mobilní navigace', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390)
  expect(oldRequests).toEqual([])
  expect(errors).toEqual([])
})
