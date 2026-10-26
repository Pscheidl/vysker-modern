import { test, expect, type Page } from './fixtures'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'

// The browser suite supplies this disposable database, never the developer's data.
function fixtureQuery(sql: string, parameters: unknown[] = []): any[][] {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  const script = [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    name = connection.execute("SELECT current_database()").fetchone()[0]',
    '    if not name.startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    rows = connection.execute(sys.argv[2], json.loads(sys.argv[3])).fetchall()',
    'print(json.dumps(rows))',
  ].join('\n')
  return JSON.parse(execFileSync('python3', ['-c', script, database, sql, JSON.stringify(parameters)], { encoding: 'utf8' }))
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
  return { 'X-CSRF-Token': (await response.json()).csrf_token, Origin: new URL(page.url()).origin }
}

async function subscribe(page: Page, prefix: string, confirm = false) {
  const email = `${prefix}-${randomBytes(6).toString('hex')}@vysker.test`
  const notice = await (await page.request.get('/api/v1/privacy')).json()
  const response = await page.request.post('/api/v1/subscriptions', {
    data: { email, consent: { fingerprint: notice.fingerprint } },
  })
  expect(response.status()).toBe(202)
  const [[id, message]] = fixtureQuery("SELECT s.id,m.body FROM subscribers s JOIN mail_queue m ON m.subscriber_id=s.id WHERE s.email=%s AND m.purpose='verification' ORDER BY m.id DESC LIMIT 1", [email])
  const token = (message as string).match(/token=([a-f0-9]{64})/)![1]
  if (confirm) {
    const response = await page.request.post('/api/v1/subscriptions/verify', { data: { token } })
    expect(response.status()).toBe(204)
  }
  return { id: id as number, email, token, message: message as string, notice }
}

async function unsubscribeFixture(page: Page) {
  const subscriber = await subscribe(page, 'unsubscribe-design', true)
  const token = randomBytes(32).toString('hex')
  fixtureQuery("INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id) SELECT %s,subscriber_id,'unsubscribe',extract(epoch FROM now())::bigint+86400,id FROM subscription_consents WHERE subscriber_id=%s AND confirmed_at IS NOT NULL AND withdrawn_at IS NULL AND superseded_at IS NULL RETURNING hash", [createHash('sha256').update(token).digest('hex'), subscriber.id])
  return { ...subscriber, path: `/odber/odhlasit?token=${token}` }
}

async function expectSiteDesign(page: Page) {
  await expect(page.locator('.site-header .brand')).toBeVisible()
  await expect(page.locator('main.subscription-page .subscription-card')).toBeVisible()
  await expect(page.locator('.site-footer')).toBeVisible()
  // Verify an actually loaded common stylesheet, not just a link in the HTML.
  await expect.poll(() => page.locator('.site-header .header-inner').evaluate(header => getComputedStyle(header).display)).toBe('flex')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  const sheet = page.locator('link[rel="stylesheet"]').first()
  await expect(sheet).toHaveAttribute('href', /\/pkg\/obecni-web(?:\.[A-Za-z0-9_-]+)?\.css/)
  const themeScript = await page.request.get('/subscription-page.js')
  expect(themeScript.status()).toBe(200)
  expect(themeScript.headers()['cache-control']).toBe('no-cache')
}

test('admin edits privacy content publicly without rewriting previous subscription consent', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const pending = await subscribe(page, 'old-privacy')
  const oldEvidence = fixtureQuery('SELECT n.consent_text,n.privacy_json FROM subscription_consents c JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE c.subscriber_id=%s', [pending.id])
  await login(page)
  const original = await (await page.request.get('/api/v1/admin/privacy')).json()
  const marker = `Zpracovatel prohlížečového testu ${Date.now()}`
  const wording = 'Přihlášením souhlasím s novým zněním odběru pro tento test. Odvolat souhlas mohu kdykoli.'
  try {
    await page.goto('/admin/ochrana-udaju')
    await expect(page.getByRole('heading', { name: 'Ochrana údajů a podmínky odběru', exact: true })).toBeVisible()
    await page.getByLabel('Verze informací', { exact: false }).fill(`browser-${Date.now()}`)
    await page.getByLabel('Zpracovatelé a příjemci údajů', { exact: false }).fill(marker)
    await page.getByLabel('Text souhlasu u formuláře', { exact: false }).fill(wording)
    await page.getByLabel('Podmínky odběru', { exact: false }).fill('**Upravené podmínky odběru** pro tento test.\n\nOdběr lze kdykoli ukončit.')
    await page.getByLabel('Práva odběratelů', { exact: false }).fill('**Upravená práva odběratelů** pro tento test.')
    const saved = page.waitForResponse(response => response.url().endsWith('/api/v1/admin/privacy') && response.request().method() === 'PUT')
    await page.getByRole('button', { name: 'Uložit informace', exact: true }).click()
    expect((await saved).status()).toBe(200)
    await expect(page.getByRole('status')).toContainText('Informace o ochraně údajů a podmínky odběru byly uloženy.')
    const current = await (await page.request.get('/api/v1/privacy')).json()
    expect(current.fingerprint).not.toBe(original.fingerprint)
    expect(current.policy.processors).toBe(marker)
    await page.reload()
    await expect(page.getByLabel('Zpracovatelé a příjemci údajů', { exact: false })).toHaveValue(marker)
    await page.screenshot({ path: test.info().outputPath('privacy-admin-desktop.png'), fullPage: true })
    await page.setViewportSize({ width: 390, height: 844 })
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await page.screenshot({ path: test.info().outputPath('privacy-admin-mobile.png'), fullPage: true })
    await page.setViewportSize({ width: 1280, height: 844 })
    await page.goto('/ochrana-udaju')
    await expect(page.locator('main')).toContainText(marker)
    await expect(page.locator('main strong').filter({ hasText: 'Upravené podmínky odběru' })).toBeVisible()
    await expect(page.locator('main strong').filter({ hasText: 'Upravená práva odběratelů' })).toBeVisible()
    await page.goto('/odber')
    await expect(page.locator('.newsletter-form')).toContainText(wording)

    const stale = await page.request.post('/api/v1/subscriptions', {
      data: { email: `stale-${Date.now()}@vysker.test`, consent: { fingerprint: original.fingerprint } },
    })
    expect(stale.status()).toBe(400)
    expect(await stale.text()).toContain('Informace o odběru se změnily')
    expect(fixtureQuery('SELECT n.consent_text,n.privacy_json FROM subscription_consents c JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE c.subscriber_id=%s', [pending.id])).toEqual(oldEvidence)
    await page.goto(`/odber/potvrdit?token=${pending.token}`)
    await page.getByText('Informace platné při žádosti', { exact: true }).click()
    await expect(page.locator('main')).toContainText(original.policy.processors)
    await expect(page.locator('main')).not.toContainText(marker)
    expect(errors).toEqual([])
  } finally {
    const current = await (await page.request.get('/api/v1/admin/privacy')).json()
    const restored = await page.request.put('/api/v1/admin/privacy', {
      headers: await mutationHeaders(page),
      data: { policy: original.policy, fingerprint: current.fingerprint },
    })
    expect(restored.status()).toBe(200)
  }
})

test('verification and publication emails end with useful links and no personal signature', async ({ page }) => {
  const subscriber = await subscribe(page, 'unsigned-mail', true)
  expect(subscriber.message.trim()).toMatch(/Informace o soukromí: http:\/\/127\.0\.0\.1:3107\/ochrana-udaju$/)
  expect(subscriber.message.trim().split('\n').at(-1)).not.toBe(subscriber.notice.policy.controller_name)
  await login(page)
  const headers = await mutationHeaders(page)
  const created = await page.request.post('/api/v1/admin/documents', { headers, data: { title: 'Dokument pro kontrolu e-mailu bez podpisu' } })
  expect(created.status()).toBe(201)
  const id = (await created.json()).id
  const published = await page.request.post(`/api/v1/admin/documents/${id}/publish`, { headers })
  expect(published.status()).toBe(204)
  const notification = () => fixtureQuery("SELECT body FROM mail_queue WHERE subscriber_id=%s AND purpose='document' ORDER BY id DESC LIMIT 1", [subscriber.id])
  await expect.poll(() => notification().length, { timeout: 45_000 }).toBe(1)
  const body = notification()[0][0] as string
  expect(body.trim()).toMatch(/http:\/\/127\.0\.0\.1:3107\/odber\/nastaveni\?token=[a-f0-9]{64}$/)
  expect(body.trim().split('\n').at(-1)).not.toBe(subscriber.notice.policy.controller_name)
  expect(body).not.toContain('\n\nPavel Pscheidl')
})

test('unsubscribe uses the common design in both themes and GET never changes the subscription', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const subscriber = await unsubscribeFixture(page)
  await page.goto(subscriber.path)
  for (const width of [1280, 375]) {
    await page.setViewportSize({ width, height: 844 })
    for (const theme of ['light', 'dark'] as const) {
      await page.evaluate(theme => localStorage.setItem('vysker-theme', theme), theme)
      const response = await page.goto(subscriber.path)
      expect(response?.status()).toBe(200)
      expect(response?.headers()['cache-control']).toBe('no-store')
      expect(response?.headers()['referrer-policy']).toBe('no-referrer')
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
      await expectSiteDesign(page)
      await expect(page.getByRole('heading', { name: 'Odhlásit odběr novinek', exact: true })).toBeVisible()
      await expect(page.locator('main')).not.toContainText('Přihlášení k účtu není potřeba.')
      expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE id=%s', [subscriber.id])).toEqual([[null]])
      await page.screenshot({ path: test.info().outputPath(`unsubscribe-${width}-${theme}.png`), fullPage: true })
    }
  }
  const submit = page.getByRole('button', { name: 'Odhlásit odběr novinek', exact: true })
  await submit.focus()
  await expect(submit).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('heading', { name: 'Odběr je odhlášený.', exact: true })).toBeVisible()
  await expectSiteDesign(page)
  expect(fixtureQuery('SELECT unsubscribed_at IS NOT NULL FROM subscribers WHERE id=%s', [subscriber.id])).toEqual([[true]])
  expect(errors).toEqual([])
})

test('unsubscribe works without JavaScript and keeps invalid-link pages styled', async ({ browser, page }) => {
  const subscriber = await unsubscribeFixture(page)
  const context = await browser.newContext({ javaScriptEnabled: false, viewport: { width: 375, height: 844 } })
  const noScript = await context.newPage()
  try {
    await noScript.goto(`http://127.0.0.1:3107${subscriber.path}`)
    await expectSiteDesign(noScript)
    expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE id=%s', [subscriber.id])).toEqual([[null]])
    await noScript.getByRole('button', { name: 'Odhlásit odběr novinek', exact: true }).click()
    await expect(noScript.getByRole('heading', { name: 'Odběr je odhlášený.', exact: true })).toBeVisible()
    await expectSiteDesign(noScript)
    expect(fixtureQuery('SELECT unsubscribed_at IS NOT NULL FROM subscribers WHERE id=%s', [subscriber.id])).toEqual([[true]])
    const response = await noScript.goto(`http://127.0.0.1:3107${subscriber.path}`)
    expect(response?.status()).toBe(400)
    await expect(noScript.getByRole('heading', { name: 'Odkaz není dostupný', exact: true })).toBeVisible()
    await expectSiteDesign(noScript)
  } finally {
    await context.close()
  }
})
