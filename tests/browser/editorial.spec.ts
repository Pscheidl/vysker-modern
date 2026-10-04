import { test, expect, type Page } from './fixtures'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

function fixture(sql: string, params: unknown[] = []) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return JSON.parse(execFileSync('python3', ['-c',
    'import json,psycopg,sys\nwith psycopg.connect(sys.argv[1]) as c:\n r=c.execute(sys.argv[2],json.loads(sys.argv[3]))\n print(json.dumps(r.fetchall() if r.description else []))',
    database, sql, JSON.stringify(params)], { encoding: 'utf8' }))
}
async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('imported photographs belong in the page image library and preview privately', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const png = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg=='
  const doc = fixture("INSERT INTO documents(title,status,created_at) VALUES ('Imported image fixture','draft','now') RETURNING id")[0][0]
  const image = fixture("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,'puvodni-kaple.png','image/png',octet_length(decode(%s,'base64')),decode(%s,'base64')) RETURNING id", [doc, png, png])[0][0]
  const url = `/api/v1/legacy-media/${image}`
  fixture("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES ('browser-image','https://vysker.cz/assets/Image.ashx','hash','now','now','{}',%s,%s,%s)", [doc, image, `/api/v1/attachments/${image}`])
  const id = fixture("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('browser-imported-images','Převzaté fotografie',%s,FALSE,'now') RETURNING id", [`![Původní popis](${url})`])[0][0]
  fixture("INSERT INTO page_revisions(page_id,version,title,content,slug,published,saved_at) SELECT id,version,title,content,slug,published,updated_at FROM pages WHERE id=%s", [id])
  await login(page)
  await page.goto(`/admin/stranky/${id}`)
  await page.getByText('Dříve nahrané obrázky', { exact: true }).click()
  const library = page.locator('.page-image-list')
  await expect(library).toContainText('puvodni-kaple.png')
  await expect(library).toContainText('Převzato z původního webu')
  await expect(library.locator('img')).toHaveAttribute('src', `/api/v1/admin/legacy-media/${image}`)
  await expect.poll(() => library.locator('img').evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(2)
  expect((await request.get(`/api/v1/admin/legacy-media/${image}`)).status()).toBe(401)
  expect((await request.get(url)).status()).toBe(404)
  await page.getByRole('button', { name: 'Náhled textu', exact: true }).click()
  const preview = page.getByRole('region', { name: 'Náhled textu stránky' })
  await expect.poll(() => preview.getByRole('img', { name: 'Původní popis' }).evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(2)
  await page.getByLabel('Popis obrázku', { exact: false }).fill('Nový popis')
  const content = page.getByLabel('Obsah stránky', { exact: false })
  await content.evaluate((el: HTMLTextAreaElement) => el.setSelectionRange(el.value.length, el.value.length))
  await library.getByRole('button', { name: 'Vložit do textu', exact: true }).click()
  await expect(content).toHaveValue(new RegExp(`!\\[Nový popis\\]\\(${url}\\)`))
  expect((await content.inputValue()).match(/\/api\/v1\/legacy-media\//g)).toHaveLength(2)
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 2', { exact: false })).toBeVisible()
  await content.fill('Text bez fotografie')
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 3', { exact: false })).toBeVisible()
  await page.getByText('Dříve nahrané obrázky', { exact: true }).click()
  await expect(library).toContainText('puvodni-kaple.png')
  expect(fixture('SELECT count(*) FROM page_images WHERE page_id=%s', [id])[0][0]).toBe(0)
  expect((await request.get(url)).status()).toBe(404)
  expect(errors).toEqual([])
})

test('page images preserve edits, preview drafts and follow publication on mobile', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await login(page)
  await page.goto('/admin/stranky/nove')
  await page.getByLabel('Název', { exact: false }).fill('Obrázky v editoru')
  await page.getByLabel('Adresa stránky', { exact: false }).fill('browser-obrazky')
  const content = page.getByLabel('Obsah stránky', { exact: false })
  await content.fill('🌳 Příliš žluťoučký kůň')
  await content.evaluate((el: HTMLTextAreaElement) => { el.focus(); el.setSelectionRange(3, el.value.length) })
  await page.getByRole('button', { name: 'Tučně', exact: true }).click()
  await expect(content).toHaveValue('🌳 **Příliš žluťoučký kůň**')
  await expect(page.getByText('Obrázky přidáte po prvním uložení stránky.', { exact: false })).toBeVisible()
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/stranky\/\d+$/)
  const editorUrl = page.url()
  await expect(page.getByText('Verze 1', { exact: false })).toBeVisible()
  const unsaved = '🌳 **Příliš žluťoučký kůň**\n\nRozpracovaný text\n\n| Den | Čas |\n| --- | --- |\n| Pondělí | 8–12 |\n\n~~Staré~~'
  await content.fill(unsaved)
  const description = 'Kaple [náves] "u nás"'
  await page.getByLabel('Popis obrázku', { exact: false }).fill(description)
  const file = page.getByLabel('Vybrat obrázek', { exact: false })
  await file.setInputFiles({ name: 'fake.png', mimeType: 'image/png', buffer: Buffer.from('<svg onload="alert(1)"></svg>') })
  await page.getByRole('button', { name: 'Nahrát a vložit obrázek', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Podporované obrázky')
  await expect(content).toHaveValue(unsaved)
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg==', 'base64')
  await file.setInputFiles({ name: 'kaple.png', mimeType: 'image/png', buffer: png })
  await content.evaluate((el: HTMLTextAreaElement) => el.setSelectionRange(el.value.length, el.value.length))
  let release!: () => void
  const held = new Promise<void>(resolve => { release = resolve })
  let started!: () => void
  const uploadStarted = new Promise<void>(resolve => { started = resolve })
  await page.route('**/api/v1/admin/pages/*/images', async route => {
    if (route.request().method() === 'POST') { started(); await held }
    await route.continue()
  })
  await page.getByRole('button', { name: 'Nahrát a vložit obrázek', exact: true }).click()
  await uploadStarted
  await expect(page.getByRole('button', { name: 'Uložit změny', exact: true })).toBeDisabled()
  release()
  await expect(content).toHaveValue(/!\[Kaple/)
  const inserted = await content.inputValue()
  expect(inserted.startsWith(unsaved)).toBe(true)
  const imageUrl = inserted.match(/\/api\/v1\/page-images\/\d+/)![0]
  expect((await request.get(imageUrl)).status()).toBe(404)
  await page.getByRole('button', { name: 'Náhled textu', exact: true }).click()
  const preview = page.getByRole('region', { name: 'Náhled textu stránky' })
  await expect(preview.getByRole('img', { name: description, exact: true })).toBeVisible()
  expect(await preview.getByRole('img').evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(2)
  await expect(preview.locator('table td').first()).toHaveText('Pondělí')
  await expect(preview.locator('del')).toHaveText('Staré')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  await page.screenshot({ path: test.info().outputPath('page-image-editor-mobile.png'), fullPage: true })
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 2', { exact: false })).toBeVisible()
  expect((await request.get(imageUrl)).status()).toBe(404)
  await page.getByLabel('Zveřejnit na webu', { exact: false }).check()
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 3', { exact: false })).toBeVisible()
  const publicImage = await request.get(imageUrl)
  expect(publicImage.status()).toBe(200)
  expect(publicImage.headers()['content-type']).toBe('image/png')
  expect(await publicImage.body()).toEqual(png)
  await page.goto('/stranky/browser-obrazky')
  await expect(page.getByRole('img', { name: description, exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  await page.screenshot({ path: test.info().outputPath('page-image-public-mobile.png'), fullPage: true })
  await page.goto(editorUrl)
  await page.getByLabel('Zveřejnit na webu', { exact: false }).uncheck()
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 4', { exact: false })).toBeVisible()
  expect((await request.get(imageUrl)).status()).toBe(404)
  await page.getByText('Dříve nahrané obrázky', { exact: true }).click()
  await page.getByLabel('Popis obrázku', { exact: false }).fill('Další pohled')
  await expect(page.locator('.page-image-list')).toContainText('kaple.png')
  await page.getByRole('button', { name: 'Vložit do textu', exact: true }).click()
  await expect(content).toHaveValue(/!\[Další pohled\]/)
  expect((await content.inputValue()).match(/\/api\/v1\/page-images\//g)).toHaveLength(2)
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 5', { exact: false })).toBeVisible()
  expect(errors).toEqual([])
})

test('recovery link changes the password once and disappears from the address bar', async ({ page }) => {
  fixture("INSERT INTO administrators(email,password_hash) SELECT 'recovery-browser@vysker.test',password_hash FROM administrators WHERE email='e2e-admin@vysker.test'")
  await page.goto('/admin')
  await page.getByRole('link', { name: 'Zapomenuté heslo' }).click()
  await page.getByLabel('E-mail', { exact: true }).fill('recovery-browser@vysker.test')
  await page.getByRole('button', { name: 'Poslat odkaz' }).click()
  await expect(page.getByRole('status')).toContainText('Pokud pro tuto adresu')
  const body = fixture("SELECT body FROM recovery_mail m JOIN administrators a ON a.id=m.administrator_id WHERE a.email='recovery-browser@vysker.test' ORDER BY m.id DESC LIMIT 1")[0][0]
  const link = body.match(/http:\/\/127\.0\.0\.1:3107\/admin\/obnova#token=[a-f0-9]+/)[0]
  await page.goto(link)
  const password = page.getByLabel('Nové heslo (alespoň 24 znaků)', { exact: true })
  await expect(password).toHaveAttribute('minlength', '24')
  await expect(page).toHaveURL('http://127.0.0.1:3107/admin/obnova')
  await password.fill('Recovered-browser-password-12345!')
  await page.getByLabel('Nové heslo znovu').fill('Recovered-browser-password-12345!')
  await page.getByRole('button', { name: 'Změnit heslo', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Heslo bylo změněno')
  await page.getByRole('link', { name: 'Zpět k přihlášení' }).click()
  await page.getByLabel('E-mail', { exact: true }).fill('recovery-browser@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Recovered-browser-password-12345!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  expect(fixture("SELECT count(*) FROM password_resets r JOIN administrators a ON a.id=r.administrator_id WHERE a.email='recovery-browser@vysker.test'")[0][0]).toBe(0)
})

test('Markdown preview, edit conflicts, revisions and editable navigation', async ({ page, context }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await login(page)
  await page.goto('/admin/stranky/nove')
  await page.getByLabel('Název', { exact: false }).fill('Spolky v browser testu')
  await page.getByLabel('Adresa stránky', { exact: false }).fill('browser-spolky')
  await page.getByLabel('Obsah stránky', { exact: false }).fill('## Činnost\n\n**Důležité**\n\n<script>throw Error("XSS")</script>\n\n[Kontakt](/kontakt)')
  await page.getByRole('button', { name: 'Náhled textu', exact: true }).click()
  await expect(page.locator('.admin-page-preview strong')).toHaveText('Důležité')
  await expect(page.locator('.admin-page-preview script')).toHaveCount(0)
  await page.getByLabel('Zveřejnit na webu', { exact: false }).check()
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/stranky\/\d+$/)
  const editorUrl = page.url()
  const second = await context.newPage()
  await second.goto(editorUrl)
  await expect(second.getByLabel('Obsah stránky', { exact: false })).toHaveValue(/Činnost/)
  let releaseSave!: () => void
  const heldSave = new Promise<void>(resolve => { releaseSave = resolve })
  let markStarted!: () => void
  const saveStarted = new Promise<void>(resolve => { markStarted = resolve })
  await page.route('**/api/v1/admin/pages/*', async route => {
    if (route.request().method() === 'PUT') { markStarted(); await heldSave }
    await route.continue()
  })
  await page.getByLabel('Obsah stránky', { exact: false }).fill('Aktuální obsah')
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await saveStarted
  await expect(page.locator('.revision-list').getByRole('button', { name: 'Načíst do editoru' }).first()).toBeDisabled()
  releaseSave()
  await expect(page.getByText('Verze 2', { exact: false })).toBeVisible()
  await second.getByLabel('Obsah stránky', { exact: false }).fill('Rozpracováno v druhé záložce')
  await second.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(second.getByRole('alert')).toContainText('mezitím')
  await expect(second.getByLabel('Obsah stránky', { exact: false })).toHaveValue('Rozpracováno v druhé záložce')
  await second.close()
  await page.locator('.revision-list li').filter({ hasText: 'Verze 1' }).getByRole('button', { name: 'Načíst do editoru' }).click()
  await expect(page.getByLabel('Obsah stránky', { exact: false })).toHaveValue(/Činnost/)
  await page.getByRole('button', { name: 'Uložit změny', exact: true }).click()
  await expect(page.getByText('Verze 3', { exact: false })).toBeVisible()
  await page.getByRole('link', { name: 'Navigace', exact: true }).click()
  await page.getByRole('button', { name: 'Přidat položku' }).click()
  await page.getByLabel('Název', { exact: false }).fill('Spolky browser')
  await page.getByLabel('Adresa odkazu', { exact: false }).fill('/stranky/browser-spolky')
  await page.getByRole('button', { name: 'Uložit položku', exact: true }).click()
  await expect(page.locator('.admin-table')).toContainText('Spolky browser')
  await page.goto('/stranky/browser-spolky')
  await expect(page.getByRole('heading', { name: 'Činnost', exact: true })).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Hlavní navigace', exact: true }).getByRole('link', { name: 'Spolky browser' })).toBeVisible()
  expect(errors).toEqual([])
})

test('an older pending email request cannot hide a newly received recovery link', async ({ page }) => {
  let release!: () => void
  const held = new Promise<void>(resolve => { release = resolve })
  let started!: () => void
  const requestStarted = new Promise<void>(resolve => { started = resolve })
  await page.route('**/api/v1/admin/password-recovery', async route => {
    started()
    await held
    await route.fulfill({ status: 202, contentType: 'application/json', body: '{"accepted":true}' })
  })
  await page.goto('/admin/obnova')
  await page.getByLabel('E-mail', { exact: true }).fill('pending-recovery@vysker.test')
  await page.getByRole('button', { name: 'Poslat odkaz' }).click()
  await requestStarted
  await page.goto(`/admin/obnova#token=${'a'.repeat(64)}`)
  const password = page.getByLabel('Nové heslo (alespoň 24 znaků)', { exact: true })
  await expect(password).toBeVisible()
  release()
  await expect(password).toBeEnabled()
  await expect(page.getByText('Heslo bylo změněno.', { exact: false })).toHaveCount(0)
})
