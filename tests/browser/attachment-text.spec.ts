import { test, expect } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

function fixture(name = 'zápis obce.pdf') {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as c:',
    '    if not c.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"): raise RuntimeError("Expected disposable test data")',
    '    doc=c.execute("INSERT INTO documents(title,status,created_at) VALUES (\'Dokument s textovým přepisem\',\'published\',\'2026-11-01\') RETURNING id").fetchone()[0]',
    '    aid=c.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,%s,\'application/pdf\',8,%s) RETURNING id",(doc,sys.argv[2],b"%PDF-1.4")).fetchone()[0]',
    '    c.execute("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method) SELECT id,sha256,name,1,\'ready\',\'ocr\' FROM attachments WHERE id=%s",(aid,))',
    '    c.execute("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES (%s,1,\'Strana 1\',%s),(%s,2,\'Strana 2\',%s)",(aid,"Zastupitelstvo hlasovalo o pronájmu hospody.",aid,"Žluťoučký kůň <script>window.textInjected=true</script> Konec druhé strany."))',
    'print(json.dumps({"document":doc,"attachment":aid}))',
  ].join('\n'), database, name], { encoding: 'utf8' })) as { document: number, attachment: number }
}

for (const javaScriptEnabled of [true, false]) {
  test.describe(javaScriptEnabled ? 'Public text copies' : 'Public text copies without JavaScript', () => {
    test.use({ javaScriptEnabled })
    test('a document downloads its original from the title and main action and offers its full transcript', async ({ page }) => {
      const ids = fixture()
      await page.setViewportSize({ width: 360, height: 800 })
      await page.goto(`/dokumenty/${ids.document}`)
      await expect(page.getByRole('heading', { name: 'Dokument s textovým přepisem', exact: true })).toBeVisible()
      const originalTitle = page.getByRole('link', { name: 'zápis obce.pdf (stáhnout původní přílohu)', exact: true })
      const originalDownload = page.getByRole('link', { name: 'Stáhnout původní přílohu zápis obce.pdf', exact: true })
      const preview = page.getByRole('link', { name: 'Zobrazit přepis textu přílohy zápis obce.pdf (otevře se v novém okně)', exact: true })
      const download = page.getByRole('link', { name: 'Stáhnout přepis textu přílohy zápis obce.pdf', exact: true })
      await expect(originalDownload).toHaveText('Stáhnout původní přílohu')
      await expect(preview).toHaveText('Zobrazit přepis textu')
      await expect(download).toContainText('Stáhnout přepis textu')
      for (const link of [originalTitle, originalDownload]) {
        await expect(link).toHaveAttribute('href', `/api/v1/attachments/${ids.attachment}`)
        await expect(link).toHaveAttribute('download', '')
        const downloaded = page.waitForEvent('download')
        await link.click()
        const original = await downloaded
        expect(original.suggestedFilename()).toBe('zápis obce.pdf')
        expect(readFileSync((await original.path())!, 'utf8')).toBe('%PDF-1.4')
        await expect(page).toHaveURL(new RegExp(`/dokumenty/${ids.document}$`))
      }
      await expect(preview).toHaveAttribute('href', `/api/v1/attachments/${ids.attachment}/text`)
      await expect(preview).toHaveAttribute('target', '_blank')
      await expect(download).toHaveAttribute('href', `/api/v1/attachments/${ids.attachment}/text/download`)
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

    for (const width of [320, 390, 1280]) {
      test(`the original precedes the transcript with a long filename at ${width}px`, async ({ page }) => {
        const name = 'Zápis-ze-zasedání-zastupitelstva-obce-Vyskeř-včetně-všech-schválených-příloh-a-doprovodných-podkladů-pro-veřejnost-2026-11-03.pdf'
        const ids = fixture(name)
        await page.setViewportSize({ width, height: 900 })
        await page.goto(`/dokumenty/${ids.document}`)
        const originalTitle = page.getByRole('link', { name: `${name} (stáhnout původní přílohu)`, exact: true })
        const originalDownload = page.getByRole('link', { name: `Stáhnout původní přílohu ${name}`, exact: true })
        const preview = page.getByRole('link', { name: `Zobrazit přepis textu přílohy ${name} (otevře se v novém okně)`, exact: true })
        const download = page.getByRole('link', { name: `Stáhnout přepis textu přílohy ${name}`, exact: true })
        for (const link of [originalTitle, originalDownload, preview, download]) {
          await expect(link).toBeVisible()
          const bounds = await link.boundingBox()
          expect(bounds).not.toBeNull()
          expect(bounds!.x).toBeGreaterThanOrEqual(0)
          expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width)
        }
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)

        const originalBounds = (await originalDownload.boundingBox())!
        const previewBounds = (await preview.boundingBox())!
        const downloadBounds = (await download.boundingBox())!
        expect(originalBounds.y + originalBounds.height).toBeLessThanOrEqual(previewBounds.y)
        if (width <= 390) {
          expect(previewBounds.height).toBeGreaterThanOrEqual(44)
          expect(downloadBounds.height).toBeGreaterThanOrEqual(44)
          expect(previewBounds.y + previewBounds.height).toBeLessThanOrEqual(downloadBounds.y)
        }

        await originalTitle.focus()
        await page.keyboard.press('Tab')
        await expect(originalDownload).toBeFocused()
        await page.keyboard.press('Tab')
        await expect(preview).toBeFocused()
        await page.keyboard.press('Tab')
        await expect(download).toBeFocused()
      })
    }
  })
}
