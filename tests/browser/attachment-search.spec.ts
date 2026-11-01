import { test, expect } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

function fixture(term: string) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as c:',
    '    if not c.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"): raise RuntimeError("Expected disposable test data")',
    '    term=sys.argv[2]',
    '    title=c.execute("INSERT INTO documents(title,status,created_at,source_published_on) VALUES (%s,\'published\',\'2026-11-01\',\'2010-01-01\') RETURNING id",(term,)).fetchone()[0]',
    '    body=c.execute("INSERT INTO documents(title,status,created_at,source_published_on) VALUES (\'Zápis se shodou uvnitř přílohy\',\'published\',\'2026-11-01\',\'2026-10-01\') RETURNING id").fetchone()[0]',
    '    aid=c.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,\'zapis.pdf\',\'application/pdf\',8,%s) RETURNING id",(body,b"%PDF-1.4")).fetchone()[0]',
    '    c.execute("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method) SELECT id,sha256,name,1,\'ready\',\'text\' FROM attachments WHERE id=%s",(aid,))',
    '    c.execute("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES (%s,2,\'Strana 2\',%s)",(aid,"Úvod "*120+term+" <img src=x onerror=window.searchInjected=true> závěr"))',
    'print(json.dumps({"title":title,"body":body,"attachment":aid}))',
  ].join('\n'), database, term], { encoding: 'utf8' })) as { title: number, body: number, attachment: number }
}

for (const javaScriptEnabled of [true, false]) {
  test.describe(javaScriptEnabled ? 'Document content search' : 'Document content search without JavaScript', () => {
    test.use({ javaScriptEnabled })
    test('titles lead, attachment snippets are safe, and explicit sorting survives a form submit', async ({ page }) => {
      const term = `obsahovytest${Date.now()}${javaScriptEnabled ? 'a' : 'b'}`
      const ids = fixture(term)
      await page.setViewportSize({ width: 360, height: 780 })
      await page.goto(`/hledat?q=${term}`)
      const results = page.locator('.site-result')
      await expect(results).toHaveCount(2)
      await expect(results.first().getByRole('heading')).toHaveText(term)
      const content = results.nth(1)
      await expect(content.locator('.site-result-preview')).toContainText(`${term} <img src=x onerror=window.searchInjected=true>`)
      await expect(content.locator('.site-result-preview img')).toHaveCount(0)
      await expect(content.getByRole('link', { name: 'zapis.pdf · Strana 2', exact: true })).toHaveAttribute('href', `/api/v1/attachments/${ids.attachment}#page=2`)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      await page.getByLabel('Řazení', { exact: true }).selectOption('newest')
      await page.getByRole('button', { name: 'Hledat', exact: true }).click()
      await expect(results.first().getByRole('heading')).toHaveText('Zápis se shodou uvnitř přílohy')
      await expect(page.getByLabel('Řazení', { exact: true })).toHaveValue('newest')
      await page.getByLabel('Řazení', { exact: true }).selectOption('relevance')
      await page.getByRole('button', { name: 'Hledat', exact: true }).click()
      await expect(results.first().getByRole('heading')).toHaveText(term)
      const attachmentResponse = page.waitForResponse(response => response.url().includes(`/api/v1/attachments/${ids.attachment}`))
      await results.nth(1).getByRole('link', { name: 'zapis.pdf · Strana 2', exact: true }).click()
      expect((await attachmentResponse).status()).toBe(200)
    })
  })
}

test('search text and sorting edited before hydration are preserved', async ({ page }) => {
  let releaseHydration = () => {}
  const hydrationReady = new Promise<void>(resolve => { releaseHydration = resolve })
  await page.route(/\/pkg\/[^/]+\.wasm$/, async route => {
    await hydrationReady
    await route.continue()
  })
  await page.setViewportSize({ width: 360, height: 780 })
  await page.goto('/hledat', { waitUntil: 'commit' })
  await expect(page.locator('#mobile-menu-toggle')).toBeDisabled()
  const query = page.getByRole('searchbox', { name: 'Hledat v dokumentech', exact: true })
  const sort = page.getByLabel('Řazení', { exact: true })
  const term = `PredNactenim${Date.now()}`
  await query.fill(term)
  await sort.selectOption('oldest')
  releaseHydration()
  await expect(page.locator('#mobile-menu-toggle')).toBeEnabled()
  await expect(query).toHaveValue(term)
  await expect(sort).toHaveValue('oldest')
  await page.getByRole('button', { name: 'Hledat', exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`/hledat\\?q=${term}&razeni=oldest$`))
  await expect(query).toHaveValue(term)
  await expect(sort).toHaveValue('oldest')
})
