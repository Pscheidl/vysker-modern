import { test, expect, type Page } from './fixtures'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'

const PASSWORD = 'Disposable-browser-test-password!'

// Only the disposable database created by e2e-server.py supplies synthetic fixtures.
function fixtureQuery(sql: string, parameters: unknown[] = []): any[][] {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  const script = [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    rows = connection.execute(sys.argv[2], json.loads(sys.argv[3])).fetchall()',
    'print(json.dumps(rows))',
  ].join('\n')
  return JSON.parse(execFileSync('python3', ['-c', script, database, sql, JSON.stringify(parameters)], { encoding: 'utf8' }))
}

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  await page.goto('/admin/odberatele')
  await expect(page.getByRole('heading', { name: 'Odběratelé novinek', exact: true })).toBeVisible()
}

async function search(page: Page, query: string) {
  await page.getByRole('searchbox', { name: 'Hledat podle e-mailu', exact: true }).fill(query)
  await page.getByRole('button', { name: 'Hledat', exact: true }).click()
}

async function subscribe(page: Page, email: string, confirm: boolean) {
  const notice = await (await page.request.get('/api/v1/privacy')).json()
  const response = await page.request.post('/api/v1/subscriptions', {
    data: { email, consent: { fingerprint: notice.fingerprint } },
  })
  expect(response.status()).toBe(202)
  const [[id, message]] = fixtureQuery("SELECT s.id,m.body FROM subscribers s JOIN mail_queue m ON m.subscriber_id=s.id WHERE s.email=%s AND m.purpose='verification' ORDER BY m.id DESC LIMIT 1", [email])
  const token = message.match(/token=([a-f0-9]{64})/)[1]
  if (confirm) {
    const response = await page.request.post('/api/v1/subscriptions/verify', { data: { token } })
    expect(response.status()).toBe(204)
  }
  return { id: id as number, token: token as string }
}

test('newsletter saves selected categories, confirms them and allows changes from a notification link', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const email = `selected-topics-${Date.now()}@vysker.test`
  await page.goto('/odber')
  const form = page.locator('.newsletter-form')
  const submit = form.getByRole('button', { name: 'Přihlásit k odběru', exact: true })
  await expect(submit).toBeEnabled()
  await form.locator('summary').click()
  await form.getByLabel('Všechny kategorie úřední desky (i budoucí)', { exact: true }).uncheck()
  await form.getByLabel('Úřední deska bez kategorie', { exact: true }).uncheck()
  const budget = form.getByLabel('Rozpočet', { exact: true })
  await budget.check()
  const categoryId = Number(await budget.inputValue())
  await form.getByLabel('E-mailová adresa', { exact: true }).fill(email)
  await submit.click()
  await expect(form.getByRole('status')).toContainText('e-mail')
  const savedPreferences = () => fixtureQuery('SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents FROM subscribers WHERE email=%s', [email])[0]
  expect(savedPreferences()).toEqual([true, [], true, true])
  expect(fixtureQuery('SELECT c.all_notice_categories,c.notice_category_ids,c.uncategorized_notices,c.documents FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id WHERE s.email=%s AND c.confirmed_at IS NULL', [email])).toEqual([[false, [categoryId], false, true]])
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  const verification = fixtureQuery("SELECT m.body FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.purpose='verification' ORDER BY m.id DESC LIMIT 1", [email])[0][0] as string
  const confirmation = verification.match(/http:\/\/127\.0\.0\.1:3107\/odber\/potvrdit\?token=[a-f0-9]+/)![0]
  await page.goto(confirmation)
  await expect(page.locator('main')).toContainText('Rozpočet')
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  await page.getByRole('button', { name: 'Potvrdit odběr novinek', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Odběr je potvrzený.', exact: true })).toBeVisible()
  expect(savedPreferences()).toEqual([false, [categoryId], false, true])
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).not.toBeNull()

  // Publish through the real UI to obtain the settings link generated in mail.
  await login(page)
  await page.getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  await page.getByLabel('Název', { exact: false }).fill('Dokument pro test výběru odběru')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  await page.getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Archivovat dokument', exact: true })).toBeVisible()
  const notification = () => fixtureQuery("SELECT m.body FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.purpose='document' ORDER BY m.id DESC LIMIT 1", [email])
  // The durable publication outbox is prepared by the worker every 30 seconds.
  await expect.poll(() => notification().length, { timeout: 45_000 }).toBe(1)
  const message = notification()[0][0] as string
  const management = message.match(/http:\/\/127\.0\.0\.1:3107\/odber\/nastaveni\?token=[a-f0-9]+/)![0]
  await page.context().clearCookies()
  await page.goto(management)
  await expect(page.getByRole('heading', { name: 'Nastavení odběru', exact: true })).toBeVisible()
  expect(savedPreferences()).toEqual([false, [categoryId], false, true])
  await expect(page.getByLabel('Rozpočet', { exact: true })).toBeChecked()
  await expect(page.getByLabel('Všechny kategorie úřední desky, i nově přidané', { exact: true })).not.toBeChecked()
  await expect(page.getByLabel('Úřední deska bez kategorie', { exact: true })).not.toBeChecked()
  await expect(page.getByLabel('Obecné dokumenty', { exact: true })).toBeChecked()
  await page.getByLabel('Rozpočet', { exact: true }).uncheck()
  await page.getByRole('button', { name: 'Uložit výběr', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Výběr je uložený.')
  expect(savedPreferences()).toEqual([false, [], false, true])
  await page.goto(management)
  await expect(page.getByLabel('Rozpočet', { exact: true })).not.toBeChecked()
  await expect(page.getByLabel('Obecné dokumenty', { exact: true })).toBeChecked()
  // Later settings must not rewrite what the subscriber originally confirmed.
  expect(fixtureQuery('SELECT c.all_notice_categories,c.notice_category_ids,c.uncategorized_notices,c.documents FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id WHERE s.email=%s AND c.confirmed_at IS NOT NULL', [email])).toEqual([[false, [categoryId], false, true]])
  expect(errors).toEqual([])
})

test('newsletter server accepts the default selection and documents only with no category IDs', async ({ page }) => {
  for (const documentsOnly of [false, true]) {
    const email = `empty-category-list-${documentsOnly}-${Date.now()}@vysker.test`
    await page.goto('/odber')
    const form = page.locator('.newsletter-form')
    const submit = form.getByRole('button', { name: 'Přihlásit k odběru', exact: true })
    await expect(submit).toBeEnabled()
    if (documentsOnly) {
      await form.locator('summary').click()
      await form.getByLabel('Všechny kategorie úřední desky (i budoucí)', { exact: true }).uncheck()
      await form.getByLabel('Úřední deska bez kategorie', { exact: true }).uncheck()
    }
    await form.getByLabel('E-mailová adresa', { exact: true }).fill(email)
    await submit.click()
    await expect(form.getByRole('status')).toContainText('e-mail')
    expect(fixtureQuery('SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents FROM subscribers WHERE email=%s', [email])).toEqual([[true, [], true, true]])
    expect(fixtureQuery('SELECT c.all_notice_categories,c.notice_category_ids,c.uncategorized_notices,c.documents FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id WHERE s.email=%s AND c.confirmed_at IS NULL', [email])).toEqual([[!documentsOnly, [], !documentsOnly, true]])
  }
})

test('subscriber search is literal and normalized, filters use consent and pagination has a total', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const requestedOffsets: string[] = []
  page.on('request', request => {
    const url = new URL(request.url())
    if (url.pathname === '/api/v1/admin/subscribers') {
      requestedOffsets.push(url.searchParams.get('offset') ?? '')
    }
  })
  const prefix = `admin-list-${Date.now()}`
  fixtureQuery("INSERT INTO subscribers(email,retention_started_at) SELECT %s || '-' || lpad(n::text,2,'0') || '@vysker.test',extract(epoch FROM now())::bigint FROM generate_series(1,23) n RETURNING id", [prefix])
  for (const suffix of ['%literal', '_literal', '-withdrawn']) {
    fixtureQuery('INSERT INTO subscribers(email,retention_started_at) VALUES (%s,extract(epoch FROM now())::bigint) RETURNING id', [`${prefix}${suffix}@vysker.test`])
  }
  fixtureQuery('UPDATE subscribers SET unsubscribed_at=extract(epoch FROM now())::bigint WHERE email=%s RETURNING id', [`${prefix}-withdrawn@vysker.test`])
  const active = `${prefix}-active@vysker.test`
  await subscribe(page, active, true)
  await login(page)
  // Rendering the pager must never invoke its click handler or fetch another page.
  await expect(page.getByRole('table', { name: 'Odběratelé novinek' })).toBeVisible()
  expect(requestedOffsets.length).toBeGreaterThan(0)
  expect(requestedOffsets.every(offset => offset === '0')).toBe(true)
  await search(page, prefix)
  await expect(page.getByRole('status').filter({ hasText: 'Celkem 27 odběratelů.' })).toBeVisible()
  await expect(page.getByRole('table', { name: 'Odběratelé novinek' }).locator('tbody tr')).toHaveCount(20)
  const pager = page.getByRole('navigation', { name: 'Stránkování odběratelů' })
  await expect(pager.getByRole('button', { name: 'Předchozí' })).toBeDisabled()
  expect(requestedOffsets.every(offset => offset === '0')).toBe(true)
  await pager.getByRole('button', { name: 'Další' }).click()
  await expect(pager).toContainText('Strana 2')
  await expect(page.getByRole('table', { name: 'Odběratelé novinek' }).locator('tbody tr')).toHaveCount(7)
  await expect(pager.getByRole('button', { name: 'Další' })).toBeDisabled()
  await pager.getByRole('button', { name: 'Předchozí' }).click()
  await expect(pager).toContainText('Strana 1')
  await page.getByRole('combobox', { name: 'Stav odběru' }).selectOption('active')
  await expect(page.getByRole('status').filter({ hasText: 'Celkem 1 odběratelů.' })).toBeVisible()
  await expect(page.getByRole('table')).toContainText(active)
  await page.getByRole('combobox', { name: 'Stav odběru' }).selectOption('pending')
  await expect(page.getByRole('status').filter({ hasText: 'Celkem 25 odběratelů.' })).toBeVisible()
  await page.getByRole('combobox', { name: 'Stav odběru' }).selectOption('unsubscribed')
  await expect(page.getByRole('status').filter({ hasText: 'Celkem 1 odběratelů.' })).toBeVisible()
  await expect(page.getByRole('table')).toContainText(`${prefix}-withdrawn@vysker.test`)
  await page.getByRole('combobox', { name: 'Stav odběru' }).selectOption('')
  for (const suffix of ['%', '_']) {
    await search(page, `${prefix}${suffix}`)
    await expect(page.getByRole('status').filter({ hasText: 'Celkem 1 odběratelů.' })).toBeVisible()
    await expect(page.getByRole('table')).toContainText(`${prefix}${suffix}literal@vysker.test`)
  }
  await search(page, `  ${active.toUpperCase()}  `)
  await expect(page.getByRole('status').filter({ hasText: 'Celkem 1 odběratelů.' })).toBeVisible()
  await expect(page.getByRole('table')).toContainText(active)
  expect(requestedOffsets.every(offset => offset === '0' || offset === '20')).toBe(true)
  expect(errors).toEqual([])
})

test('subscriber evidence is inspectable and downloads as an audited private JSON file', async ({ page, playwright }) => {
  const email = `admin-evidence-${Date.now()}@vysker.test`
  const { id, token } = await subscribe(page, email, false)
  const [[wording, privacy]] = fixtureQuery('SELECT n.consent_text,n.privacy_json FROM subscription_consents c JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE c.subscriber_id=%s', [id])
  await login(page)
  await search(page, email)
  await page.getByRole('button', { name: 'Důkazy a správa', exact: true }).click()
  const detail = page.getByRole('region', { name: 'Důkazy a správa odběratele' })
  await expect(detail).toContainText(email)
  await detail.getByText('Zobrazit úplné důkazy v JSON', { exact: true }).click()
  const evidence = JSON.parse((await detail.locator('pre').textContent())!)
  expect(evidence.subscriber.id).toBe(id)
  expect(evidence.consents[0].consent_text).toBe(wording)
  expect(evidence.consents[0].privacy_json).toBe(privacy)
  expect(evidence.mail_history).toHaveLength(1)
  expect(evidence.subscriber.verified_at).toBeNull()
  const downloadPromise = page.waitForEvent('download')
  await detail.getByRole('link', { name: 'Stáhnout soukromý export JSON', exact: true }).click()
  const download = await downloadPromise
  expect(download.suggestedFilename()).toBe(`odberatel-${id}.json`)
  const text = readFileSync((await download.path())!, 'utf8')
  const downloaded = JSON.parse(text)
  expect(downloaded.subscriber).toEqual(evidence.subscriber)
  expect(downloaded.consents).toEqual(evidence.consents)
  for (const forbidden of [token, createHash('sha256').update(token).digest('hex'), '"body"', '"lock_token"', '"password_hash"']) {
    expect(text).not.toContain(forbidden)
  }
  const audits = fixtureQuery("SELECT entity_id,details,ip_address FROM audit_log WHERE operation='subscriber_exported' AND entity_id=%s ORDER BY id", [id])
  expect(audits.length).toBeGreaterThanOrEqual(2)
  expect(audits.every(row => row[0] === id && row[1] === null && row[2] === null)).toBe(true)
  const response = await page.request.get(`/api/v1/admin/subscribers/${id}/export`)
  expect(response.headers()['cache-control']).toContain('no-store')
  expect(response.headers()['referrer-policy']).toBe('no-referrer')
  const anonymous = await playwright.request.newContext({ baseURL: 'http://127.0.0.1:3107' })
  const denied = await anonymous.get(`/api/v1/admin/subscribers/${id}/export?download=true`)
  expect(denied.status()).toBe(401)
  expect(await denied.text()).not.toContain(email)
  await anonymous.dispose()
})

test('withdrawal requires own password and confirmation, cancels mail and protects leaving a filled form', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const email = `admin-withdraw-${Date.now()}@vysker.test`
  const { id } = await subscribe(page, email, true)
  const [[consent]] = fixtureQuery('SELECT id FROM subscription_consents WHERE subscriber_id=%s AND confirmed_at IS NOT NULL', [id])
  const unsubscribe = 'c'.repeat(64)
  fixtureQuery("INSERT INTO subscription_tokens(hash,subscriber_id,consent_id,purpose,expires_at) VALUES (%s,%s,%s,'unsubscribe',9223372036854775807) RETURNING subscriber_id", [createHash('sha256').update(unsubscribe).digest('hex'), id, consent])
  // Schedule synthetic mail tomorrow so the background SMTP worker cannot claim it.
  fixtureQuery("INSERT INTO mail_queue(subscriber_id,consent_id,purpose,subject,body,next_attempt_at,created_at) VALUES (%s,%s,'document','Synthetic queued message','Private fixture body',extract(epoch FROM now())::bigint+86400,extract(epoch FROM now())::bigint) RETURNING id", [id, consent])
  await login(page)
  await search(page, email)
  await page.getByRole('button', { name: 'Důkazy a správa', exact: true }).click()
  const detail = page.getByRole('region', { name: 'Důkazy a správa odběratele' })
  await expect(detail).toContainText('může ještě dorazit')
  await expect(detail.getByRole('button', { name: 'Odvolat odběr', exact: true })).toBeDisabled()
  const password = detail.getByLabel('Vaše současné heslo pro potvrzení', { exact: true })
  await password.fill('incorrect-password')
  page.once('dialog', dialog => dialog.dismiss())
  await detail.getByRole('button', { name: 'Zavřít detail', exact: true }).click()
  await expect(password).toHaveValue('incorrect-password')
  await detail.getByRole('button', { name: 'Odvolat odběr', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zrušit', exact: true }).click()
  expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE id=%s', [id])[0][0]).toBeNull()
  await detail.getByRole('button', { name: 'Odvolat odběr', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Odvolat odběr', exact: true }).click()
  await expect(detail.getByRole('alert')).toBeVisible()
  await expect(password).toHaveValue('')
  expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE id=%s', [id])[0][0]).toBeNull()
  await password.fill(PASSWORD)
  await detail.getByRole('button', { name: 'Odvolat odběr', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Odvolat odběr', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Odběr byl odvolán')
  await expect(detail).toContainText('Odhlášený')
  await expect(password).toHaveValue('')
  const [[withdrawnAt, consentWithdrawn]] = fixtureQuery('SELECT s.unsubscribed_at,c.withdrawn_at FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.id=%s AND c.id=%s', [id, consent])
  expect(withdrawnAt).not.toBeNull()
  expect(consentWithdrawn).toBe(withdrawnAt)
  expect(fixtureQuery('SELECT count(*) FROM subscription_tokens WHERE subscriber_id=%s', [id])[0][0]).toBe(0)
  expect(fixtureQuery('SELECT count(*) FROM mail_queue WHERE subscriber_id=%s AND sent_at IS NULL AND (cancelled=FALSE OR body IS NOT NULL)', [id])[0][0]).toBe(0)
  const response = await page.request.post('/api/v1/subscriptions/unsubscribe', { data: { token: unsubscribe } })
  expect(response.status()).toBe(400)
  expect(fixtureQuery("SELECT entity_id,details FROM audit_log WHERE operation='subscriber_withdrawn' AND entity_id=%s", [id])).toEqual([[id, null]])
  expect(errors).toEqual([])
})
