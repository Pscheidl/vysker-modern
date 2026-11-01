import { test, expect } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

const endpoint = '/api/v1/admin/attachment-index/status'

function databaseScript(script: string, ...args: string[]) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as c:',
    '    if not c.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"): raise RuntimeError("Expected disposable test data")',
    ...script.split('\n').map(line => `    ${line}`),
  ].join('\n'), database, ...args], { encoding: 'utf8' })
}

function fixture() {
  return JSON.parse(databaseScript([
    'doc=c.execute("INSERT INTO documents(title,status,created_at) VALUES (\'Průběh vyhledávání\',\'draft\',\'2026-11-01\') RETURNING id").fetchone()[0]',
    'ids={}',
    'for state in ["ready","pending","processing","failed"]:',
    '    name={"ready":"hotovo.csv","pending":"ceka.csv","processing":"zpracovava.csv","failed":"priloha-k-oprave.csv"}[state]',
    '    aid=c.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,%s,\'text/csv\',10,%s) RETURNING id",(doc,name,b"key,value\\n")).fetchone()[0]',
    '    ids[state]=aid',
    '    if state!="pending":',
    '        c.execute("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method,attempts,error,finished_at) SELECT id,sha256,name,1,%s,\'text\',1,\'Internal diagnostic\',now() FROM attachments WHERE id=%s",(state,aid))',
    'print(json.dumps({"document":doc,**ids}))',
  ].join('\n'))) as { document: number, ready: number, pending: number, processing: number, failed: number }
}

function finish(attachment: number) {
  databaseScript('c.execute("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method,finished_at) SELECT id,sha256,name,1,\'ready\',\'text\',now() FROM attachments WHERE id=%s",(int(sys.argv[2]),))', String(attachment))
}

test('attachment preparation shows real counts, refreshes automatically, and preserves its last state during errors', async ({ page }) => {
  const ids = fixture()
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const login = await page.request.post('/api/v1/admin/login', {
    headers: { Origin: 'http://127.0.0.1:3107' },
    data: { email: 'e2e-admin@vysker.test', password: 'Disposable-browser-test-password!' },
  })
  expect(login.status()).toBe(200)
  const before = await (await page.request.get(endpoint)).json()
  await page.clock.install()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/admin')
  const panel = page.getByRole('region', { name: 'Vyhledávání v přílohách', exact: true })
  const progress = panel.getByRole('progressbar', { name: 'Přílohy připravené pro vyhledávání', exact: true })
  await expect(progress).toHaveAttribute('max', String(before.total))
  await expect(progress).toHaveAttribute('value', String(before.ready))
  for (const state of ['ready', 'pending', 'processing', 'failed']) {
    await expect(panel.locator(`[data-state="${state}"]`)).toHaveText(String(before[state]))
  }
  await expect(panel.locator('.attachment-index-current')).toContainText('zpracovava.csv')
  await expect(panel.getByRole('link', { name: 'priloha-k-oprave.csv', exact: true })).toHaveAttribute('href', `/admin/dokumenty/${ids.document}`)
  await expect(panel).not.toContainText('Internal diagnostic')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)

  finish(ids.pending)
  await expect(progress).toHaveAttribute('value', String(before.ready + 1))
  await expect(panel.locator('[data-state="pending"]')).toHaveText(String(before.pending - 1))
  await expect(panel.locator('.attachment-index-summary')).toContainText(`${Math.floor((before.ready + 1) * 100 / before.total)} %`)

  await page.route(`**${endpoint}`, route => route.fulfill({ status: 503, json: { error: 'Temporary outage' } }))
  await panel.getByRole('button', { name: 'Obnovit stav', exact: true }).click()
  await expect(panel.getByText('Stav se nepodařilo obnovit. Zobrazen je poslední známý stav.', { exact: true })).toBeVisible()
  await expect(progress).toHaveAttribute('value', String(before.ready + 1))
  await page.unroute(`**${endpoint}`)
  await panel.getByRole('button', { name: 'Obnovit stav', exact: true }).click()
  await expect(panel.locator('.admin-error')).toHaveCount(0)

  await page.getByRole('navigation', { name: 'Administrace', exact: true }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty$/)
  await expect(progress).toHaveAttribute('value', String(before.ready + 1))

  let requests = 0
  let release = () => {}
  let requestStarted = () => {}
  const started = new Promise<void>(resolve => { requestStarted = resolve })
  const held = new Promise<void>(resolve => { release = resolve })
  await page.route(`**${endpoint}`, async route => {
    requests += 1
    requestStarted()
    await held
    await route.continue()
  })
  await panel.getByRole('button', { name: 'Obnovit stav', exact: true }).click()
  await started
  await expect(panel.getByRole('button', { name: 'Obnovit stav', exact: true })).toBeDisabled()
  await page.clock.fastForward(10_000)
  expect(requests).toBe(1)
  await page.getByRole('navigation', { name: 'Administrace', exact: true }).getByRole('link', { name: 'Stránky', exact: true }).click()
  await expect(panel).toHaveCount(0)
  release()
  await page.clock.fastForward(10_000)
  expect(requests).toBe(1)
  expect(errors).toEqual([])
})
