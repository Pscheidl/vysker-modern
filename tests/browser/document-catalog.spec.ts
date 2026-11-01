import { test, expect } from './fixtures'

test('limited searches state their scope and preserve the query in full-site fallback', async ({ page }) => {
  await page.goto('/uredni-deska?stav=archiv&q=synteticky-nenalezeny-vyraz')
  await expect(page).toHaveURL(/\/dokumenty\?state=archive/)
  await expect(page.getByText('Hledáte ve společném veřejném archivu dokumentů a sejmutých písemností. Aktuálně vyvěšené dokumenty zde nejsou.', { exact: true })).toBeVisible()
  await expect(page.getByRole('link', { name: 'Prohledat celý web', exact: true })).toHaveAttribute('href', '/hledat?q=synteticky-nenalezeny-vyraz')
  await page.goto('/uredni-deska?q=synteticky-nenalezeny-vyraz')
  await expect(page.getByRole('searchbox', { name: 'Hledat v dokumentech' })).toHaveValue('synteticky-nenalezeny-vyraz')
  await expect(page.locator('#board-search-scope')).toContainText('Hledáte pouze mezi aktuálně vyvěšenými písemnostmi.')
  await expect(page.getByRole('link', { name: 'Společný veřejný archiv dokumentů', exact: true })).toHaveAttribute('href', '/dokumenty?state=archive&q=synteticky-nenalezeny-vyraz&kategorie=')
  await page.getByRole('link', { name: 'Prohledat celý web', exact: true }).click()
  await expect(page).toHaveURL(/\/hledat\?q=synteticky-nenalezeny-vyraz$/)
})

test('catalog and board use the same canonical document details without duplicate search results', async ({ page }) => {
  await page.goto('/dokumenty?q=zkusebni+01')
  const catalog = page.locator('.site-result').filter({ has: page.getByRole('link', { name: 'Zkušební vyhláška 01', exact: true }) })
  await expect(catalog).toHaveCount(1)
  const canonical = await catalog.getByRole('link').getAttribute('href')
  expect(canonical).toMatch(/^\/dokumenty\/\d+$/)
  await page.goto('/uredni-deska?q=zkusebni+01')
  await expect(page.locator('.site-result').getByRole('link', { name: 'Zkušební vyhláška 01', exact: true })).toHaveAttribute('href', canonical!)
  await page.goto('/hledat?q=zkusebni+01')
  await expect(page.locator('.site-result').getByRole('link', { name: 'Zkušební vyhláška 01', exact: true })).toHaveCount(1)
})

test('catalog search and empty results fit a narrow viewport and keyboard navigation', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 780 })
  await page.goto('/dokumenty?q=synteticky-nenalezeny-vyraz')
  await expect(page.getByLabel('Téma', { exact: true })).toBeVisible()
  await page.getByRole('searchbox', { name: 'Hledat v dokumentech' }).focus()
  await page.keyboard.press('Tab')
  await expect(page.getByLabel('Téma', { exact: true })).toBeFocused()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('a migrated restricted title never exposes original metadata through the shared detail or search', async ({ page }) => {
  const { readFileSync } = await import('node:fs')
  const { execFileSync } = await import('node:child_process')
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  const query = (sql: string, parameters: unknown[] = []) => JSON.parse(execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    name = connection.execute("SELECT current_database()").fetchone()[0]',
    '    if not name.startswith("vysker_test_"): raise RuntimeError("Expected disposable test data")',
    '    rows = connection.execute(sys.argv[2], json.loads(sys.argv[3])).fetchall()',
    'print(json.dumps(rows))',
  ].join('\n'), database, sql, JSON.stringify(parameters)], { encoding: 'utf8' }))
  const title = 'Zkušební vyhláška 05'
  const id = query('SELECT id FROM documents WHERE title=%s', [title])[0][0]
  query("UPDATE documents SET metadata_restricted=TRUE,public_title='Veřejný anonymní záznam' WHERE id=%s RETURNING id", [id])
  try {
    await page.goto(`/dokumenty/${id}`)
    await expect(page.getByRole('heading', { name: 'Veřejný anonymní záznam', exact: true })).toBeVisible()
    await expect(page.getByText(title, { exact: true })).toHaveCount(0)
    await expect(page.getByText('Záznam pro test stránkování.', { exact: true })).toHaveCount(0)
    await page.goto(`/hledat?q=${encodeURIComponent(title)}`)
    await expect(page.locator('.site-result')).toHaveCount(0)
  } finally {
    query('UPDATE documents SET metadata_restricted=FALSE,public_title=NULL WHERE id=%s RETURNING id', [id])
  }
})
