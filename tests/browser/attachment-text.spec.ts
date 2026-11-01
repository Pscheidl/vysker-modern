import { test, expect } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

function fixture() {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as c:',
    '    if not c.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"): raise RuntimeError("Expected disposable test data")',
    '    doc=c.execute("INSERT INTO documents(title,status,created_at) VALUES (\'Dokument s textovým přepisem\',\'published\',\'2026-11-01\') RETURNING id").fetchone()[0]',
    '    aid=c.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,\'zápis obce.pdf\',\'application/pdf\',8,%s) RETURNING id",(doc,b"%PDF-1.4")).fetchone()[0]',
    '    c.execute("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method) SELECT id,sha256,name,1,\'ready\',\'ocr\' FROM attachments WHERE id=%s",(aid,))',
    '    c.execute("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES (%s,1,\'Strana 1\',%s),(%s,2,\'Strana 2\',%s)",(aid,"Zastupitelstvo hlasovalo o pronájmu hospody.",aid,"Žluťoučký kůň <script>window.textInjected=true</script> Konec druhé strany."))',
    'print(json.dumps({"document":doc,"attachment":aid}))',
  ].join('\n'), database], { encoding: 'utf8' })) as { document: number, attachment: number }
}

for (const javaScriptEnabled of [true, false]) {
  test.describe(javaScriptEnabled ? 'Public text copies' : 'Public text copies without JavaScript', () => {
    test.use({ javaScriptEnabled })
    test('a document offers its full text for reading and download', async ({ page }) => {
      const ids = fixture()
      await page.setViewportSize({ width: 360, height: 800 })
      await page.goto(`/dokumenty/${ids.document}`)
      await expect(page.getByRole('heading', { name: 'Dokument s textovým přepisem', exact: true })).toBeVisible()
      const preview = page.getByRole('link', { name: 'Zobrazit text přílohy zápis obce.pdf (otevře se v novém okně)', exact: true })
      const download = page.getByRole('link', { name: 'Stáhnout text přílohy zápis obce.pdf', exact: true })
      await expect(preview).toHaveAttribute('href', `/api/v1/attachments/${ids.attachment}/text`)
      await expect(preview).toHaveAttribute('target', '_blank')
      await expect(download).toBeVisible()
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      const downloaded = page.waitForEvent('download')
      await download.click()
      const file = await downloaded
      expect(file.suggestedFilename()).toBe('zápis obce.pdf.txt')
      const text = readFileSync((await file.path())!, 'utf8')
      expect(text).toContain('Automatický přepis OCR může obsahovat chyby')
      expect(text).toContain('=== Strana 1 ===')
      expect(text).toContain('=== Strana 2 ===')
      expect(text).toContain('Konec druhé strany.')
      const originalUrl = page.url()
      const opened = page.context().waitForEvent('page')
      const response = page.context().waitForEvent('response', {
        predicate: r => r.url().endsWith(`/api/v1/attachments/${ids.attachment}/text`),
      })
      await preview.click()
      const popup = await opened
      await popup.waitForLoadState('domcontentloaded')
      const loaded = await response
      expect(loaded.headers()['content-type']).toBe('text/plain; charset=utf-8')
      await expect(page).toHaveURL(originalUrl)
      await expect(page.getByRole('heading', { name: 'Dokument s textovým přepisem', exact: true })).toBeVisible()
      await expect(popup).toHaveURL(new RegExp(`/api/v1/attachments/${ids.attachment}/text$`))
      await expect(popup.locator('body')).toContainText('Žluťoučký kůň <script>window.textInjected=true</script>')
      await expect(popup.locator('script')).toHaveCount(0)
      expect(await popup.locator('body').innerText()).toContain('Konec druhé strany.')
      await popup.close()
    })
  })
}
