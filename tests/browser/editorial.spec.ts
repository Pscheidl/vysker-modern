import { test, expect, type Page } from '@playwright/test'
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
