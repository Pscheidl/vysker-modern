import { test, expect, type Page } from './fixtures'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

const baseURL = `http://127.0.0.1:${process.env.E2E_PORT || '3107'}`

// This helper reads synthetic mail from the isolated test database. No real SMTP
// recipient, developer database or production credentials are used by this suite.
function fixtureQuery(sql: string, parameters: unknown[] = []) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c',
    [
      'import json, psycopg, sys',
      'with psycopg.connect(sys.argv[1]) as connection:',
      '    name = connection.execute("SELECT current_database()").fetchone()[0]',
      '    if not name.startswith("vysker_test_"): raise RuntimeError("Expected disposable browser data")',
      '    rows = connection.execute(sys.argv[2], json.loads(sys.argv[3])).fetchall()',
      'print(json.dumps(rows))',
    ].join('\n'),
    database, sql, JSON.stringify(parameters)], { encoding: 'utf8' }))
}
async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

async function operations(page: Page) {
  if (await page.locator('details.document-operations').getAttribute('open') === null) {
    await page.getByText('Další úkony s dokumentem', { exact: true }).click()
  }
}

async function publishDocument(page: Page) {
  await page.getByRole('button', { name: 'Zveřejnit dokument', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit dokument', exact: true }).click()
  await expect(page.locator('.document-state-facts')).toContainText('Veřejně dostupný')
}

async function publishPosting(page: Page, date: string, action = 'hide', redactedId?: string) {
  const board = page.locator('#document-board-enabled')
  const editable = await board.count() > 0 && await board.isEnabled()
  if (editable) {
    await board.check()
  } else {
    await page.getByText('Nové vyvěšení na úřední desce', { exact: true }).click()
    await expect(page.locator('#document-days')).toHaveValue('')
    await expect(page.locator('#document-attachment-access')).toHaveValue('')
  }
  await page.locator('#document-posted').fill(date)
  await page.locator('#document-days').fill('20')
  await page.locator('#document-attachment-access').selectOption(action === 'retain' ? 'public' : 'private')
  if (editable) {
    await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
    await expect(page.locator('.admin-flash')).toContainText('uložen')
  }
  if (redactedId) {
    await page.getByText('Další možnosti vyvěšení', { exact: true }).click()
    await page.getByLabel('Po sejmutí zpřístupnit samostatné anonymizované znění', { exact: true }).check()
    await page.locator('#posting-redacted').fill(redactedId)
  }
  await expect(page.locator('#posting-rule,#posting-basis,#posting-original')).toHaveCount(0)
  await page.getByRole('button', { name: 'Zkontrolovat finální náhled', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Finální náhled dokumentu' })).toContainText('Po vyvěšení již obsah dokumentu nelze upravit.')
  if (redactedId) await expect(page.getByRole('region', { name: 'Finální náhled anonymizovaného znění', exact: true })).toContainText('jejíž bajty tím nejsou označeny za původně vyvěšené')
  await page.getByLabel('Zkontroloval/a jsem uvedený obsah a všechny přílohy.', { exact: true }).check()
  await page.getByRole('button', { name: 'Potvrdit vyvěšení této podoby', exact: true }).click()
  const id = Number(page.url().split('/').pop())
  await expect.poll(() => fixtureQuery('SELECT withdraw_on=published_on+21 FROM documents WHERE id=%s', [id])[0][0]).toBe(true)
}

test('keyboard navigation, diacritic-insensitive search and pagination', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto('/uredni-deska')
  await page.keyboard.press('Tab')
  await expect(page.getByRole('link', { name: 'Přeskočit na obsah' })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(page.locator('main')).toBeFocused()
  await expect(page.getByRole('navigation', { name: 'Stránkování výsledků' })).toContainText('Počet výsledků: 25')
  await expect(page.locator('.site-result')).toHaveCount(20)
  await page.getByRole('link', { name: 'Další', exact: true }).click()
  await expect(page).toHaveURL(/strana=2/)
  await expect(page.locator('.site-result')).toHaveCount(5)
  await page.getByRole('searchbox', { name: 'Hledat v dokumentech' }).fill('zkusebni 01')
  await page.getByRole('searchbox', { name: 'Hledat v dokumentech' }).press('Enter')
  await expect(page.getByRole('navigation', { name: 'Stránkování výsledků' })).toContainText('Počet výsledků: 1')
  await page.getByRole('link', { name: 'Zkušební vyhláška 01', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Zkušební vyhláška 01', exact: true })).toBeVisible()
  expect(errors).toEqual([])
})

test('one document editor publishes multiple files and expiry enters the shared archive with private files', async ({ page, browser }) => {
  test.setTimeout(120_000)
  await login(page)
  const navigation = page.getByRole('navigation', { name: 'Administrace' })
  await expect(navigation.getByRole('link', { name: 'Dokumenty', exact: true })).toHaveCount(1)
  await expect(navigation.getByRole('link', { name: 'Úřední deska', exact: true })).toHaveCount(0)
  await navigation.getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  await page.getByLabel('Název', { exact: false }).first().fill('Oznámení z browser testu')
  await page.getByLabel('Popis', { exact: true }).fill('Zveřejnění ověřené automatickým testem.')
  await expect(page.locator('#document-board-enabled')).not.toBeChecked()
  await expect(page.locator('#document-days')).toHaveCount(0)
  await expect(page.locator('#document-attachment-access')).toHaveCount(0)
  await expect(page.getByLabel('Číslo jednací (nepovinné)', { exact: true })).not.toHaveAttribute('required')
  await expect(page.getByLabel('Datum vytvoření dokumentu (nepovinné)', { exact: true })).not.toHaveAttribute('required')
  await expect(page.locator('#notice-issuer,#posting-rule,#posting-basis,#posting-original')).toHaveCount(0)
  await page.locator('#document-board-enabled').check()
  await expect(page.locator('#document-days')).toHaveValue('')
  await expect(page.locator('#document-days')).toHaveAttribute('required')
  await expect(page.locator('#document-attachment-access')).toHaveValue('')
  await expect(page.locator('#document-attachment-access')).toHaveAttribute('required')
  await page.setViewportSize({ width: 390, height: 844 })
  const help = page.locator('summary[aria-label="Nápověda: Počet celých dní vyvěšení"]')
  await help.focus()
  await page.keyboard.press('Enter')
  await expect(help.locator('xpath=..')).toHaveAttribute('open')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.keyboard.press('Escape')
  await expect(help.locator('xpath=..')).not.toHaveAttribute('open')
  await page.setViewportSize({ width: 1280, height: 720 })
  await page.locator('#document-days').fill('20')
  await page.locator('#document-posted').fill(fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date, 'YYYY-MM-DD')")[0][0])
  const planned = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + 21, 'FMDD. FMMM. YYYY')")[0][0]
  await expect(page.getByText(`Plánované sejmutí: ${planned}.`, { exact: false })).toBeVisible()
  await page.locator('#document-attachment-access').selectOption('private')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  const id = page.url().split('/').pop()
  // A pre-migration notice ID may now belong to a different canonical document.
  const oldId = fixtureQuery('SELECT id FROM documents WHERE id<>%s ORDER BY id LIMIT 1', [Number(id)])[0][0]
  fixtureQuery("INSERT INTO document_legacy_ids(old_model,old_id,document_id) VALUES ('notice',%s,%s) RETURNING document_id", [oldId, Number(id)])
  await page.goto(`/admin/uredni-deska/${oldId}`)
  await expect(page).toHaveURL(new RegExp(`/admin/dokumenty/${id}$`))
  await expect(page.locator('#entry-name')).toHaveValue('Oznámení z browser testu')
  const files = [
    { name: 'oznameni.txt', mimeType: 'text/plain', buffer: Buffer.from('Testovací příloha obecního webu.\n') },
    { name: 'priloha.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4\nDruhá testovací příloha.\n%%EOF\n') },
  ]
  for (const [index, file] of files.entries()) {
    await page.locator('#upload-file').setInputFiles(file)
    await page.getByRole('button', { name: 'Nahrát soubor', exact: true }).click()
    await expect(page.locator('.admin-files li')).toHaveCount(index + 1)
    await expect(page.locator('.admin-files')).toContainText(file.name)
  }
  await page.reload()
  await expect(page.locator('.admin-files li')).toHaveCount(files.length)
  for (const file of files) {
    await expect(page.locator('.admin-files')).toContainText(file.name)
  }
  const publicContext = await browser.newContext()
  const publicPage = await publicContext.newPage()
  const adminLinks = await page.locator('.admin-files a').evaluateAll(links => links.map(link => link.getAttribute('href')!))
  expect(adminLinks).toHaveLength(files.length)
  for (const link of adminLinks) {
    const response = await publicContext.request.get(`${baseURL}${link.replace('/admin', '')}`)
    expect(response.status()).toBe(404)
  }
  const today = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date, 'YYYY-MM-DD')")[0][0]
  await publishPosting(page, today)
  await expect(page.locator('.document-state-facts')).toContainText('Uzamčeno po vyvěšení')
  await publicPage.goto(`${baseURL}/dokumenty/${id}`)
  await expect(publicPage.getByRole('heading', { name: 'Oznámení z browser testu', exact: true })).toBeVisible()
  await expect(publicPage.locator('.attachment')).toHaveCount(files.length)
  for (const file of files) {
    const attachment = publicPage.locator('.attachment').filter({ has: publicPage.getByText(file.name, { exact: true }) })
    const downloadPromise = publicPage.waitForEvent('download')
    await attachment.getByRole('link', { name: 'Stáhnout', exact: true }).click()
    const download = await downloadPromise
    expect(download.suggestedFilename()).toBe(file.name)
    expect(readFileSync((await download.path())!)).toEqual(file.buffer)
  }
  // Make the stored plan due, then let the actual worker record removal.
  fixtureQuery("UPDATE document_postings SET planned_removal_on=(CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date WHERE document_id=%s AND removed_at IS NULL RETURNING id", [Number(id)])
  fixtureQuery("UPDATE documents SET withdraw_on=(CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date WHERE id=%s RETURNING id", [Number(id)])
  await expect.poll(() => fixtureQuery('SELECT board_status FROM documents WHERE id=%s', [Number(id)])[0][0], { timeout: 45_000 }).toBe('archived')
  expect(fixtureQuery('SELECT removed_at IS NOT NULL FROM document_postings WHERE document_id=%s', [Number(id)])[0][0]).toBe(true)
  await page.reload()
  await expect(page.locator('.document-state-facts')).toContainText('Sejmuto')
  await expect(page.locator('#entry-name')).toBeDisabled()
  for (const [index, link] of adminLinks.entries()) {
    expect((await publicContext.request.get(`${baseURL}${link.replace('/admin', '')}`)).status()).toBe(404)
    const internal = await page.request.get(link)
    expect(internal.status()).toBe(200)
    expect(await internal.body()).toEqual(files[index].buffer)
  }
  await publicPage.reload()
  await expect(publicPage.getByRole('heading', { name: 'Oznámení z browser testu', exact: true })).toBeVisible()
  await expect(publicPage.locator('.attachment')).toHaveCount(0)
  await publicPage.goto(`${baseURL}/dokumenty?state=archive&q=Ozn%C3%A1men%C3%AD%20z%20browser%20testu`)
  await expect(publicPage.locator('.site-result').getByRole('link', { name: 'Oznámení z browser testu', exact: true })).toHaveAttribute('href', `/dokumenty/${id}`)
  await publicPage.goto(`${baseURL}/uredni-deska?state=archive&q=Ozn%C3%A1men%C3%AD%20z%20browser%20testu`)
  await expect(publicPage).toHaveURL(/\/dokumenty\?state=archive/)
  await expect(publicPage.locator('.site-result')).toHaveCount(1)
  await publicContext.close()
})

test('email opt-in requires confirmation and opt-out cancels future deliveries', async ({ page }) => {
  const email = 'browser-subscriber@vysker.test'
  await page.goto('/odber')
  await page.getByLabel('E-mailová adresa', { exact: true }).fill(email)
  await page.getByRole('button', { name: 'Přihlásit k odběru', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('e-mail')
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  const verification = fixtureQuery("SELECT m.body FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.purpose='verification' ORDER BY m.id DESC LIMIT 1", [email])[0][0]
  const confirmation = verification.match(/http:\/\/127\.0\.0\.1:\d+\/odber\/potvrdit\?token=[a-f0-9]+/)[0]
  await page.goto(confirmation)
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  await page.getByRole('button', { name: 'Potvrdit odběr novinek', exact: true }).click()
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).not.toBeNull()
  // Hold this synthetic recipient across a possible wall-clock hour boundary.
  fixtureQuery("UPDATE subscribers SET next_digest_at=extract(epoch FROM clock_timestamp())::bigint+7200 WHERE email=%s RETURNING id", [email])
  // Two real UI publications must become one digest, including its opt-out link.
  await login(page)
  const titles = ['První novinka pro odběratele', 'Druhá novinka pro odběratele']
  for (const title of titles) {
    await page.getByRole('navigation', { name: 'Administrace' }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
    await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
    await page.getByLabel('Název', { exact: false }).fill(title)
    await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
    await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
    await publishDocument(page)
  }
  const notification = () => fixtureQuery("SELECT m.body FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.purpose='document' ORDER BY m.id", [email])
  expect(notification()).toHaveLength(0)
  // Advance only persisted timestamps in the disposable fixture, not the server clock.
  expect(fixtureQuery("UPDATE publication_outbox o SET created_at=(floor(extract(epoch FROM clock_timestamp())/3600)::bigint*3600)-1 FROM subscribers s WHERE s.id=o.subscriber_id AND s.email=%s RETURNING o.id", [email])).toHaveLength(2)
  fixtureQuery("UPDATE subscribers SET next_digest_at=0 WHERE email=%s RETURNING id", [email])
  await expect.poll(() => notification().length, { timeout: 45_000 }).toBe(1)
  const message = notification()[0][0]
  for (const title of titles) expect(message).toContain(title)
  expect(fixtureQuery("SELECT count(*) FROM mail_publication_items i JOIN subscribers s ON s.id=i.subscriber_id WHERE s.email=%s", [email])[0][0]).toBe(2)
  const unsubscribe = message.match(/http:\/\/127\.0\.0\.1:\d+\/odber\/odhlasit\?token=[a-f0-9]+/)[0]
  await page.goto(unsubscribe)
  expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  await page.getByRole('button', { name: 'Odhlásit odběr novinek', exact: true }).click()
  expect(fixtureQuery('SELECT unsubscribed_at FROM subscribers WHERE email=%s', [email])[0][0]).not.toBeNull()
  expect(fixtureQuery('SELECT count(*) FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.cancelled=FALSE AND m.sent_at IS NULL', [email])[0][0]).toBe(0)
})

test('account management loads and mobile pages fit the viewport', async ({ page }) => {
  await login(page)
  await page.getByRole('link', { name: 'Účty a hesla', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Účty a hesla', exact: true })).toBeVisible()
  await expect(page.getByLabel('Současné heslo', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Nové heslo (alespoň 24 znaků)', { exact: true })).toHaveAttribute('minlength', '24')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.goto('/hledat?q=zkusebni')
  await expect(page.getByRole('heading', { name: 'Co hledáte?', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('cancelling a scheduled notice preserves private files and rejects a stale editor', async ({ page, browser }) => {
  await login(page)
  await page.getByRole('navigation', { name: 'Administrace' }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  await page.getByLabel('Název', { exact: false }).first().fill('Zrušené plánované oznámení')
  const tomorrow = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + 1, 'YYYY-MM-DD')")[0][0]
  await page.locator('#document-board-enabled').check()
  await page.locator('#document-posted').fill(tomorrow)
  await page.locator('#document-days').fill('20')
  await page.locator('#document-attachment-access').selectOption('private')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  const id = page.url().split('/').pop()
  const contents = Buffer.from('Soukromá příloha plánovaného oznámení.\n')
  await page.locator('#upload-file').setInputFiles({
    name: 'soukroma-priloha.txt', mimeType: 'text/plain', buffer: contents,
  })
  await page.getByRole('button', { name: 'Nahrát soubor', exact: true }).click()
  await expect(page.locator('.admin-files li')).toHaveCount(1)
  const attachment = await page.locator('.admin-files a').getAttribute('href')
  expect(attachment).toBeTruthy()
  await publishPosting(page, tomorrow)
  const cancel = page.getByRole('button', { name: 'Zrušit plánované vyvěšení', exact: true })
  await expect(cancel).toBeVisible()
  await expect(page.getByRole('button', { name: 'Zaznamenat sejmutí', exact: true })).toHaveCount(0)
  await page.getByLabel('Důvod zrušení plánovaného vyvěšení', { exact: true }).fill('Termín vyžaduje další kontrolu.')
  await cancel.click()
  await expect(page.getByRole('dialog')).toContainText('Přílohy zůstanou zachované')
  await page.getByRole('dialog').getByRole('button', { name: 'Zrušit plánované vyvěšení', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Přílohy zůstávají zachované')
  await page.reload()
  await expect(page.getByRole('button', { name: 'Zkontrolovat finální náhled', exact: true })).toBeVisible()
  await expect(page.locator('.admin-files li')).toHaveCount(1)
  const privateFile = await page.request.get(attachment!)
  expect(privateFile.status()).toBe(200)
  expect(await privateFile.body()).toEqual(contents)
  const publicContext = await browser.newContext()
  try {
    for (const path of [`/api/v1/notices/${id}`, attachment!.replace('/admin', '')]) {
      expect((await publicContext.request.get(`${baseURL}${path}`)).status()).toBe(404)
    }
  } finally {
    await publicContext.close()
  }
  await publishPosting(page, tomorrow)
  await expect(cancel).toBeVisible()
  // Simulate publication by the maintenance worker while the scheduled editor is open.
  expect(fixtureQuery(`
    WITH published AS (
      UPDATE documents SET lock_state='locked',status='published',public_access=TRUE,
        board_status='published',published_on=(CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date,
        published_at=to_char(CURRENT_TIMESTAMP AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
        board_published_at=to_char(CURRENT_TIMESTAMP AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
      WHERE id=%s RETURNING id
    )
    INSERT INTO document_postings(document_id,origin,certainty,started_on,started_at,after_action,snapshot_sha256)
      SELECT id,'local','documented',(CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date,
        CURRENT_TIMESTAMP,'hide',document_fingerprint(id) FROM published RETURNING document_id
  `, [Number(id)])).toEqual([[Number(id)]])
  expect(fixtureQuery('SELECT status FROM notices WHERE id=%s', [Number(id)])[0][0]).toBe('published')
  await page.getByLabel('Důvod zrušení plánovaného vyvěšení', { exact: true }).fill('Ověření zastaralého plánovaného stavu.')
  await cancel.click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zrušit plánované vyvěšení', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Stav dokumentu se mezitím změnil')
  expect(fixtureQuery('SELECT status FROM notices WHERE id=%s', [Number(id)])[0][0]).toBe('published')
  const preservedFile = await page.request.get(attachment!)
  expect(preservedFile.status()).toBe(200)
  expect(await preservedFile.body()).toEqual(contents)
})

test('an ordinary catalogue document can be posted with the same identity and corrected as a new draft', async ({ page, browser }) => {
  await login(page)
  await page.getByRole('navigation', { name: 'Administrace' }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  const title = 'Společný katalog a vyvěšení'
  await page.getByLabel('Název', { exact: false }).fill(title)
  await page.getByLabel('Popis', { exact: true }).fill('Syntetický běžný dokument před prvním vyvěšením.')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  const id = page.url().split('/').pop()!
  await publishDocument(page)
  const visitor = await browser.newContext()
  const publicPage = await visitor.newPage()
  try {
    await publicPage.goto(`${baseURL}/dokumenty?q=${encodeURIComponent(title)}`)
    await expect(publicPage.locator('.site-result').getByRole('link', { name: title, exact: true })).toHaveAttribute('href', `/dokumenty/${id}`)
    await publicPage.goto(`${baseURL}/uredni-deska?q=${encodeURIComponent(title)}`)
    await expect(publicPage.locator('.site-result')).toHaveCount(0)
    const today = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date, 'YYYY-MM-DD')")[0][0]
    await publishPosting(page, today, 'retain')
    await expect(page.locator('.document-state-facts')).toContainText('Uzamčeno po vyvěšení')
    await publicPage.reload()
    await expect(publicPage.locator('.site-result').getByRole('link', { name: title, exact: true })).toHaveAttribute('href', `/dokumenty/${id}`)
    await publicPage.goto(`${baseURL}/hledat?q=${encodeURIComponent(title)}`)
    await expect(publicPage.locator('.site-result')).toHaveCount(1)
    await publicPage.goto(`${baseURL}/dokumenty/${id}`)
    await publicPage.setViewportSize({ width: 390, height: 844 })
    expect(await publicPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await publicPage.screenshot({ path: 'target/public-document-mobile.png', fullPage: true })
    await page.setViewportSize({ width: 390, height: 844 })
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await page.screenshot({ path: 'target/admin-document-mobile.png', fullPage: true })
    await operations(page)
    await page.getByText('Vytvořit opravu nebo navazující dokument', { exact: true }).click()
    await page.locator('#correction-reason').fill('Oprava syntetického textu.')
    await page.getByRole('button', { name: 'Vytvořit navazující koncept', exact: true }).click()
    await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
    await expect.poll(() => page.url().split('/').pop()).not.toBe(id)
    await expect(page.locator('.document-state-facts')).toContainText('Koncept')
    await expect(page.locator('#entry-name')).toBeEnabled()
    await expect(page.locator('#document-board-enabled')).not.toBeChecked()
    await expect(page.locator('#document-attachment-access')).toHaveCount(0)
  } finally {
    await visitor.close()
  }
})

test('an anonymized copy is previewed and frozen without claiming the original posting', async ({ page, browser }) => {
  await login(page)
  await page.getByRole('navigation', { name: 'Administrace' }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  await page.getByLabel('Název', { exact: false }).fill('Originál pro odvozené veřejné znění')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  const original = page.url().split('/').pop()!
  await operations(page)
  await page.getByText('Vytvořit opravu nebo navazující dokument', { exact: true }).click()
  await page.locator('#correction-kind').selectOption('anonymized')
  await page.locator('#correction-reason').fill('Syntetická oddělená veřejná kopie.')
  await page.getByRole('button', { name: 'Vytvořit navazující koncept', exact: true }).click()
  await expect.poll(() => page.url().split('/').pop()).not.toBe(original)
  const derived = page.url().split('/').pop()!
  await page.getByLabel('Název', { exact: false }).fill('Samostatné anonymizované znění')
  await page.getByLabel('Popis', { exact: true }).fill('Veřejné samostatné znění bez osobních údajů.')
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('uložen')
  await page.goto(`/admin/dokumenty/${original}`)
  const today = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date, 'YYYY-MM-DD')")[0][0]
  await publishPosting(page, today, 'anonymized', derived)
  await expect(page.locator('.document-state-facts')).toContainText('Uzamčeno po vyvěšení')
  const visitor = await browser.newContext()
  try {
    expect((await visitor.request.get(`${baseURL}/api/v1/documents/${derived}`)).status()).toBe(404)
    const posting = page.locator('.posting-history > li').first()
    await posting.getByText('Zaznamenat sejmutí nebo změnit plán', { exact: true }).click()
    await posting.getByLabel('Interní důvod *', { exact: true }).fill('Přechod na schválenou veřejnou kopii.')
    await posting.getByRole('button', { name: 'Zaznamenat sejmutí', exact: true }).click()
    await page.getByRole('dialog').getByRole('button', { name: 'Zaznamenat sejmutí', exact: true }).click()
    await expect(page.locator('.document-state-facts')).toContainText('Sejmuto')
    const publicPage = await visitor.newPage()
    await publicPage.goto(`${baseURL}/dokumenty/${derived}`)
    await expect(publicPage.getByRole('heading', { name: 'Samostatné anonymizované znění', exact: true })).toBeVisible()
    await expect(publicPage.locator('.document-postings')).toHaveCount(0)
    await page.goto(`/admin/dokumenty/${derived}`)
    await expect(page.locator('.document-state-facts')).toContainText('Uzamčeno')
    await expect(page.locator('.document-state-facts')).not.toContainText('Uzamčeno po vyvěšení')
    await expect(page.locator('#entry-name')).toBeDisabled()
  } finally {
    await visitor.close()
  }
})

test('ordinary document archive preserves public metadata and files in the same archive', async ({ page, browser }) => {
  await login(page)
  await page.getByRole('navigation', { name: 'Administrace' }).getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  const title = 'Běžný dokument ve společném archivu'
  await page.getByLabel('Název', { exact: false }).fill(title)
  await page.getByLabel('Datum vytvoření dokumentu (nepovinné)', { exact: true }).fill('2020-01-02')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  const id = page.url().split('/').pop()!
  const bytes = Buffer.from('Anonymní běžný archivní dokument.\n')
  await page.locator('#upload-file').setInputFiles({ name: 'archiv.txt', mimeType: 'text/plain', buffer: bytes })
  await page.getByRole('button', { name: 'Nahrát soubor', exact: true }).click()
  await expect(page.locator('.admin-files li')).toHaveCount(1)
  const file = (await page.locator('.admin-files a').getAttribute('href'))!.replace('/admin', '')
  await publishDocument(page)
  await operations(page)
  await page.getByText('Přesunout do archivu', { exact: true }).first().click()
  await page.getByLabel('Důvod archivace *', { exact: true }).fill('Ukončení aktuálnosti běžného syntetického dokumentu.')
  await page.getByRole('button', { name: 'Přesunout do archivu', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Přesunout do archivu', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('společného archivu')
  expect(fixtureQuery('SELECT status,public_access,board_status,has_removed_posting FROM documents WHERE id=%s', [Number(id)])[0]).toEqual(['archived', true, null, false])
  const visitor = await browser.newContext()
  try {
    const publicPage = await visitor.newPage()
    await publicPage.goto(`${baseURL}/dokumenty?state=archive&q=${encodeURIComponent(title)}`)
    await expect(publicPage.locator('.site-result').getByRole('link', { name: title, exact: true })).toHaveAttribute('href', `/dokumenty/${id}`)
    await publicPage.getByRole('link', { name: title, exact: true }).click()
    await expect(publicPage.getByText('Datum vytvoření dokumentu', { exact: true })).toBeVisible()
    await expect(publicPage.locator('.document-postings')).toHaveCount(0)
    const download = await visitor.request.get(`${baseURL}${file}`)
    expect(download.status()).toBe(200)
    expect(await download.body()).toEqual(bytes)
    await publicPage.goto(`${baseURL}/uredni-deska?q=${encodeURIComponent(title)}`)
    await expect(publicPage.locator('.site-result')).toHaveCount(0)
    await publicPage.getByRole('link', { name: 'Společný veřejný archiv dokumentů', exact: true }).click()
    await expect(publicPage).toHaveURL(/\/dokumenty\?state=archive/)
    await expect(publicPage.locator('.site-result')).toHaveCount(1)
  } finally {
    await visitor.close()
  }
})
