import { test, expect } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

const baseURL = `http://127.0.0.1:${process.env.E2E_PORT || '3107'}`

function fixtureQuery(sql: string, parameters: unknown[] = []) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c', [
    'import json, psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    name = connection.execute("SELECT current_database()").fetchone()[0]',
    '    if not name.startswith("vysker_test_"): raise RuntimeError("Expected disposable browser data")',
    '    rows = connection.execute(sys.argv[2], json.loads(sys.argv[3])).fetchall()',
    'print(json.dumps(rows))',
  ].join('\n'), database, sql, JSON.stringify(parameters)], { encoding: 'utf8' }))
}

test('hiding a posted document requires a reason and confirmation while preserving its content and posting history', async ({ page, browser }) => {
  const title = 'Syntetický dokument ke skrytí'
  const fileContent = 'Syntetická příloha musí zůstat zachovaná správci.\n'
  const reason = 'Oprávněný důvod dočasného skrytí syntetického dokumentu.'
  const id = fixtureQuery(`INSERT INTO documents(title,description,created_at,published_on,board_status,
      retain_attachments,posting_after_action)
    VALUES (%s,'Syntetický obsah před skrytím.',to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
      (now() AT TIME ZONE 'Europe/Prague')::date,'draft',TRUE,'retain') RETURNING id`, [title])[0][0]
  const attachmentId = fixtureQuery(`INSERT INTO attachments(document_id,name,content_type,size_bytes,data)
    VALUES (%s,'zachovana-priloha.txt','text/plain',%s,convert_to(%s,'UTF8')) RETURNING id`, [id, Buffer.byteLength(fileContent), fileContent])[0][0]
  fixtureQuery(`UPDATE documents SET lock_state='locked',status='published',public_access=TRUE,
      board_status='published',published_at=created_at,board_published_at=created_at
    WHERE id=%s RETURNING id`, [id])
  fixtureQuery(`INSERT INTO document_postings(document_id,origin,certainty,initial_state,started_on,started_at,
      after_action,snapshot_sha256)
    SELECT id,'local','documented','current',published_on,created_at::timestamptz,'retain',document_fingerprint(id)
    FROM documents WHERE id=%s RETURNING id`, [id])
  const snapshotSql = `SELECT to_jsonb(d)-ARRAY['public_access','access_blocked','status'],
      (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM document_postings p WHERE p.document_id=d.id),
      (SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM attachments a WHERE a.document_id=d.id)
    FROM documents d WHERE id=%s`
  const snapshot = fixtureQuery(snapshotSql, [id])
  const visitor = await browser.newContext()
  try {
    const publicPage = await visitor.newPage()
    await publicPage.goto(`${baseURL}/dokumenty/${id}`)
    await expect(publicPage.getByRole('heading', { name: title, exact: true })).toBeVisible()
    expect((await visitor.request.get(`${baseURL}/api/v1/attachments/${attachmentId}`)).status()).toBe(200)

    await page.goto('/admin')
    await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
    await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
    await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
    await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
    await page.goto(`/admin/dokumenty/${id}`)
    await expect(page.locator('.document-state-facts')).toContainText('Veřejně dostupný')
    await page.getByText('Skrytí dokumentu', { exact: true }).click()
    const hide = page.getByRole('button', { name: 'Skrýt dokument', exact: true })
    const reasonField = page.getByLabel('Důvod skrytí', { exact: false })
    await expect(hide).toBeDisabled()
    await reasonField.fill('   ')
    await expect(hide).toBeDisabled()
    await reasonField.fill(reason)
    await expect(hide).toBeEnabled()
    await hide.click()
    const confirmation = page.getByRole('dialog', { name: 'Skrýt dokument veřejnosti?', exact: true })
    await expect(confirmation).toContainText('Původní obsah a historie zůstanou zachované v administraci.')
    expect((await visitor.request.get(`${baseURL}/api/v1/documents/${id}`)).status()).toBe(200)
    await confirmation.getByRole('button', { name: 'Zrušit', exact: true }).click()
    await expect(confirmation).not.toBeVisible()
    expect(fixtureQuery('SELECT public_access FROM documents WHERE id=%s', [id])[0][0]).toBe(true)

    await hide.click()
    const completed = page.waitForResponse(response => response.url() === `${baseURL}/api/v1/admin/documents/${id}/visibility` && response.request().method() === 'POST')
    await confirmation.getByRole('button', { name: 'Skrýt dokument', exact: true }).click()
    expect((await completed).status()).toBe(204)
    await expect(page.locator('.admin-flash')).toContainText('Dokument je skrytý')
    await expect(page.locator('.document-state-facts')).toContainText('Neveřejný')
    await expect(page.getByText('Skrytí dokumentu', { exact: true })).toHaveCount(0)
    expect(fixtureQuery(snapshotSql, [id])).toEqual(snapshot)
    expect(fixtureQuery('SELECT public_access,access_blocked,board_status,has_current_posting FROM documents WHERE id=%s', [id])[0]).toEqual([false, true, 'published', true])
    expect(fixtureQuery("SELECT reason,payload->>'new' FROM document_events WHERE document_id=%s AND kind='visibility_changed'", [id])).toEqual([[reason, 'false']])

    for (const path of [`/api/v1/documents/${id}`, `/api/v1/notices/${id}`, `/api/v1/attachments/${attachmentId}`]) {
      expect((await visitor.request.get(`${baseURL}${path}`)).status()).toBe(404)
    }
    await publicPage.reload()
    await expect(publicPage.getByRole('heading', { name: 'Tudy cesta nevede.', exact: true })).toBeVisible()
    for (const route of ['/dokumenty', '/uredni-deska', '/hledat']) {
      await publicPage.goto(`${baseURL}${route}?q=${encodeURIComponent(title)}`)
      await expect(publicPage.locator('.site-result')).toHaveCount(0)
    }
    await page.reload()
    await expect(page.getByLabel('Název', { exact: false })).toHaveValue(title)
    await expect(page.locator('.admin-files li')).toHaveCount(1)
    await expect(page.getByText('Zaznamenat sejmutí nebo změnit plán', { exact: true })).toBeVisible()
    const retained = await page.request.get(`${baseURL}/api/v1/admin/attachments/${attachmentId}`)
    expect(retained.status()).toBe(200)
    expect(await retained.body()).toEqual(Buffer.from(fileContent))
  } finally {
    await visitor.close()
  }
})
