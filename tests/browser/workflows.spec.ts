import { test, expect, type Page } from './fixtures'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

// This helper reads synthetic mail from the isolated test database. No real SMTP
// recipient, developer database or production credentials are used by this suite.
function fixtureQuery(sql: string, parameters: unknown[] = []) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c',
    [
      'import json, psycopg, sys',
      'with psycopg.connect(sys.argv[1]) as connection:',
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

test('keyboard navigation, diacritic-insensitive search and pagination', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.goto('/uredni-deska')
  await page.keyboard.press('Tab')
  await expect(page.getByRole('link', { name: 'Přeskočit na obsah' })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(page.locator('main')).toBeFocused()
  await expect(page.getByRole('navigation', { name: 'Stránkování výsledků' })).toContainText('Počet výsledků: 25')
  await expect(page.locator('.document-row')).toHaveCount(20)
  await page.getByRole('link', { name: 'Další', exact: true }).click()
  await expect(page).toHaveURL(/strana=2/)
  await expect(page.locator('.document-row')).toHaveCount(5)
  await page.getByRole('searchbox', { name: 'Hledat v dokumentech' }).fill('zkusebni 01')
  await page.getByRole('searchbox', { name: 'Hledat v dokumentech' }).press('Enter')
  await expect(page.getByRole('navigation', { name: 'Stránkování výsledků' })).toContainText('Počet výsledků: 1')
  await page.getByRole('link', { name: 'Zkušební vyhláška 01', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Zkušební vyhláška 01', exact: true })).toBeVisible()
  expect(errors).toEqual([])
})

test('publish a notice with multiple attachments and download each publicly', async ({ page, browser }) => {
  await login(page)
  await page.getByRole('link', { name: 'Úřední deska', exact: true }).click()
  await page.getByRole('link', { name: 'Nové vyvěšení' }).click()
  await page.getByLabel('Název', { exact: false }).first().fill('Oznámení z browser testu')
  await page.getByLabel('Popis', { exact: true }).fill('Zveřejnění ověřené automatickým testem.')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/uredni-deska\/\d+$/)
  const id = page.url().split('/').pop()
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
    const response = await publicContext.request.get(`http://127.0.0.1:3107${link.replace('/admin', '')}`)
    expect(response.status()).toBe(404)
  }
  await page.getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Sejmout do archivu', exact: true })).toBeVisible()
  await publicPage.goto(`http://127.0.0.1:3107/uredni-deska/${id}`)
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
  const confirmation = verification.match(/http:\/\/127\.0\.0\.1:3107\/odber\/potvrdit\?token=[a-f0-9]+/)[0]
  await page.goto(confirmation)
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).toBeNull()
  await page.getByRole('button', { name: 'Potvrdit odběr novinek', exact: true }).click()
  expect(fixtureQuery('SELECT verified_at FROM subscribers WHERE email=%s', [email])[0][0]).not.toBeNull()
  // A real UI publication creates the notification, including its opt-out link.
  await login(page)
  await page.getByRole('link', { name: 'Dokumenty', exact: true }).click()
  await page.getByRole('link', { name: 'Nový dokument', exact: true }).click()
  await page.getByLabel('Název', { exact: false }).fill('Novinka pro odběratele')
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/dokumenty\/\d+$/)
  await page.getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Archivovat dokument', exact: true })).toBeVisible()
  const notification = () => fixtureQuery("SELECT m.body FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id WHERE s.email=%s AND m.purpose='document' ORDER BY m.id DESC LIMIT 1", [email])
  // Publication is committed to the outbox before the 30-second mail worker runs.
  await expect.poll(() => notification().length, { timeout: 45_000 }).toBe(1)
  const message = notification()[0][0]
  const unsubscribe = message.match(/http:\/\/127\.0\.0\.1:3107\/odber\/odhlasit\?token=[a-f0-9]+/)[0]
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
  await page.getByRole('link', { name: 'Úřední deska', exact: true }).click()
  await page.getByRole('link', { name: 'Nové vyvěšení' }).click()
  await page.getByLabel('Název', { exact: false }).first().fill('Zrušené plánované oznámení')
  const tomorrow = fixtureQuery("SELECT to_char((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + 1, 'YYYY-MM-DD')")[0][0]
  await page.getByLabel('Datum vyvěšení', { exact: false }).fill(tomorrow)
  await page.getByRole('button', { name: 'Uložit koncept', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/uredni-deska\/\d+$/)
  const id = page.url().split('/').pop()
  const contents = Buffer.from('Soukromá příloha plánovaného oznámení.\n')
  await page.locator('#upload-file').setInputFiles({
    name: 'soukroma-priloha.txt', mimeType: 'text/plain', buffer: contents,
  })
  await page.getByRole('button', { name: 'Nahrát soubor', exact: true }).click()
  await expect(page.locator('.admin-files li')).toHaveCount(1)
  const attachment = await page.locator('.admin-files a').getAttribute('href')
  expect(attachment).toBeTruthy()
  await page.getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  const cancel = page.getByRole('button', { name: 'Zrušit plánované zveřejnění', exact: true })
  await expect(cancel).toBeVisible()
  await expect(page.getByRole('button', { name: 'Sejmout do archivu', exact: true })).toHaveCount(0)
  await page.getByLabel('Důvod zrušení plánovaného zveřejnění', { exact: true }).fill('Termín vyžaduje další kontrolu.')
  await cancel.click()
  await expect(page.getByRole('dialog')).toContainText('Přílohy zůstanou zachované')
  await page.getByRole('dialog').getByRole('button', { name: 'Zrušit plánované zveřejnění', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Dokument i přílohy zůstávají v soukromém konceptu.')
  await page.reload()
  await expect(page.getByRole('button', { name: 'Zveřejnit', exact: true })).toBeVisible()
  await expect(page.locator('.admin-files li')).toHaveCount(1)
  const privateFile = await page.request.get(attachment!)
  expect(privateFile.status()).toBe(200)
  expect(await privateFile.body()).toEqual(contents)
  const publicContext = await browser.newContext()
  try {
    for (const path of [`/api/v1/notices/${id}`, attachment!.replace('/admin', '')]) {
      expect((await publicContext.request.get(`http://127.0.0.1:3107${path}`)).status()).toBe(404)
    }
  } finally {
    await publicContext.close()
  }
  await page.getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zveřejnit', exact: true }).click()
  await expect(cancel).toBeVisible()
  // Simulate publication by the maintenance worker while the scheduled editor is open.
  expect(fixtureQuery("UPDATE notices SET status='published', published_on=(CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date, published_at=updated_at WHERE id=%s RETURNING id", [Number(id)])).toEqual([[Number(id)]])
  expect(fixtureQuery('SELECT status FROM notices WHERE id=%s', [Number(id)])[0][0]).toBe('published')
  await cancel.click()
  await page.getByRole('dialog').getByRole('button', { name: 'Zrušit plánované zveřejnění', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Stav dokumentu se mezitím změnil')
  expect(fixtureQuery('SELECT status FROM notices WHERE id=%s', [Number(id)])[0][0]).toBe('published')
  const preservedFile = await page.request.get(attachment!)
  expect(preservedFile.status()).toBe(200)
  expect(await preservedFile.body()).toEqual(contents)
})
