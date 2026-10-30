import { test, expect, type Page } from './fixtures'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

const photo = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGPoSFsFRAwQCgAsJgZhzvQyqAAAAABJRU5ErkJggg==', 'base64')

function seedReport() {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  return Number(execFileSync('python3', ['-c', [
    'import psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as conn:',
    '    assert conn.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_")',
    '    print(conn.execute("INSERT INTO issue_reports(description,address,request_ip,created_at) VALUES (%s,%s,%s,%s) RETURNING id", ("Podnět pro ověření komentářů", "Vyskeř 50", "192.0.2.5", "2026-10-30T12:00:00+01:00")).fetchone()[0])',
  ].join('\n'), database], { encoding: 'utf8' }).trim())
}

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

async function openReport(page: Page, id: number) {
  await page.goto('/admin/hlaseni-zavad')
  await page.getByRole('button', { name: `Detail #${id}`, exact: true }).click()
  return page.getByRole('region', { name: 'Komentáře obce', exact: true })
}

test('administrators create signed public replies, edit photos, and delete replies with confirmation', async ({ page, browser }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const id = seedReport()
  await login(page)
  await page.goto('/admin/ucty')
  await page.getByLabel('Výchozí jméno autora', { exact: true }).fill('')
  await page.getByRole('button', { name: 'Uložit výchozí jméno', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Výchozí jméno u komentářů bylo uloženo.')
  const thread = await openReport(page, id)
  const author = thread.getByLabel('Jméno autora', { exact: true })
  const body = thread.getByLabel('Text komentáře', { exact: true })
  const publish = thread.getByRole('button', { name: 'Zveřejnit komentář', exact: true })
  await expect(author).toHaveValue('')
  await body.fill('Opravu jsme objednali. Ozveme se s termínem.')
  await expect(publish).toBeDisabled()
  await author.fill('Jana Nováková')
  await thread.getByLabel('Fotografie komentáře (nepovinné)', { exact: true }).setInputFiles([
    { name: 'prubeh.png', mimeType: 'image/png', buffer: photo },
    { name: 'odebrat.png', mimeType: 'image/png', buffer: photo },
  ])
  await expect(thread.locator('[data-comment-photo]')).toHaveCount(2)
  await thread.getByRole('button', { name: 'Odebrat novou fotografii komentáře odebrat.png', exact: true }).click()
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: 'Zavřít detail', exact: true }).click()
  await expect(body).toHaveValue('Opravu jsme objednali. Ozveme se s termínem.')
  await page.route(`**/api/v1/admin/reports/${id}/comments`, route => {
    if (route.request().method() === 'POST') return route.fulfill({ status: 503, json: { error: 'Zkušební chyba uložení.' } })
    return route.continue()
  }, { times: 1 })
  await publish.click()
  await expect(thread.getByRole('alert')).toContainText('Zkušební chyba uložení.')
  await expect(thread.locator('[data-comment-photo]')).toHaveCount(1)
  await expect(author).toHaveValue('Jana Nováková')
  await publish.click()
  const comment = thread.getByRole('article', { name: 'Komentář od Jana Nováková', exact: true })
  await expect(comment).toContainText('Opravu jsme objednali.')
  await expect(comment.getByRole('img')).toHaveCount(1)
  await expect(author).toHaveValue('')
  await expect(body).toHaveValue('')
  const publicContext = await browser.newContext()
  const publicPage = await publicContext.newPage()
  await publicPage.goto(`http://127.0.0.1:3107/hlaseni-zavad/${id}`)
  await expect(publicPage.getByRole('region', { name: 'Komentáře obce', exact: true })).toContainText('Jana Nováková')
  await expect(publicPage.getByRole('button', { name: 'Zveřejnit komentář', exact: true })).toHaveCount(0)
  const publicPhoto = publicPage.locator('.report-comments img')
  await expect.poll(() => publicPhoto.evaluate(img => (img as HTMLImageElement).naturalWidth)).toBeGreaterThan(0)
  const originalPhotoUrl = await publicPhoto.getAttribute('src')
  await comment.getByRole('button', { name: 'Upravit komentář', exact: true }).click()
  await expect(author).toHaveValue('Jana Nováková')
  await body.fill('Oprava je hotová. Děkujeme za podnět.')
  await thread.getByRole('button', { name: /^Odebrat uloženou fotografii komentáře/ }).click()
  await thread.getByLabel('Fotografie komentáře (nepovinné)', { exact: true }).setInputFiles({ name: 'hotovo.png', mimeType: 'image/png', buffer: photo })
  await thread.getByRole('button', { name: 'Uložit změny komentáře', exact: true }).click()
  await expect(comment).toContainText('Oprava je hotová.')
  await expect(comment).toContainText('Upraveno')
  expect((await publicPage.request.get(originalPhotoUrl!)).status()).toBe(404)
  await publicPage.reload()
  await expect(publicPage.locator('.report-comments')).toContainText('Oprava je hotová.')
  const updatedPhotoUrl = await publicPage.locator('.report-comments img').getAttribute('src')
  await page.getByRole('button', { name: 'Skrýt na veřejném webu', exact: true }).click()
  await expect(thread).toContainText('Hlášení je skryté.')
  expect((await publicPage.request.get(`/api/v1/reports/${id}/comments`)).status()).toBe(404)
  expect((await publicPage.request.get(updatedPhotoUrl!)).status()).toBe(404)
  await expect.poll(() => comment.getByRole('img').evaluate(img => (img as HTMLImageElement).naturalWidth)).toBeGreaterThan(0)
  await page.getByRole('button', { name: 'Znovu zveřejnit', exact: true }).click()
  page.once('dialog', dialog => dialog.dismiss())
  await comment.getByRole('button', { name: 'Smazat komentář', exact: true }).click()
  await expect(comment).toBeVisible()
  page.once('dialog', dialog => dialog.accept())
  await comment.getByRole('button', { name: 'Smazat komentář', exact: true }).click()
  await expect(comment).toHaveCount(0)
  expect((await publicPage.request.get(updatedPhotoUrl!)).status()).toBe(404)
  await publicPage.reload()
  await expect(publicPage.locator('.report-comments')).toContainText('Obec zatím nepřidala žádný komentář.')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await publicContext.close()
  expect(errors).toEqual([])
})

test('default signatures persist, earlier signatures remain, and stale edits preserve the draft', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const id = seedReport()
  await login(page)
  await page.goto('/admin/ucty')
  await page.getByLabel('Výchozí jméno autora', { exact: true }).fill('Petr Svoboda')
  await page.getByRole('button', { name: 'Uložit výchozí jméno', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Výchozí jméno u komentářů bylo uloženo.')
  const thread = await openReport(page, id)
  await expect(thread.getByLabel('Jméno autora', { exact: true })).toHaveValue('Petr Svoboda')
  await thread.getByLabel('Text komentáře', { exact: true }).fill('Prověříme možnosti opravy.')
  await thread.getByRole('button', { name: 'Zveřejnit komentář', exact: true }).click()
  const comment = thread.getByRole('article', { name: 'Komentář od Petr Svoboda', exact: true })
  await expect(comment).toBeVisible()
  await expect(thread.getByLabel('Jméno autora', { exact: true })).toHaveValue('Petr Svoboda')
  await page.goto('/admin/ucty')
  await page.getByLabel('Výchozí jméno autora', { exact: true }).fill('Petr Svoboda, obec Vyskeř')
  await page.getByRole('button', { name: 'Uložit výchozí jméno', exact: true }).click()
  await expect(page.locator('.admin-flash')).toContainText('Výchozí jméno u komentářů bylo uloženo.')
  await openReport(page, id)
  await expect(thread.getByLabel('Jméno autora', { exact: true })).toHaveValue('Petr Svoboda, obec Vyskeř')
  await expect(comment).toBeVisible()
  await comment.getByRole('button', { name: 'Upravit komentář', exact: true }).click()
  await expect(thread.getByLabel('Jméno autora', { exact: true })).toHaveValue('Petr Svoboda')
  await thread.getByLabel('Text komentáře', { exact: true }).fill('Můj rozepsaný text')
  const session = await (await page.request.get('/api/v1/admin/session')).json()
  const [stored] = await (await page.request.get(`/api/v1/admin/reports/${id}/comments`)).json()
  const update = await page.request.put(`/api/v1/admin/reports/${id}/comments/${stored.id}`, {
    headers: { 'x-csrf-token': session.csrf_token, Origin: 'http://127.0.0.1:3107' },
    multipart: { author_name: stored.author_name, body: 'Souběžně uložená nová informace.', expected_updated_at: stored.updated_at, remove_photo_ids: '[]' },
  })
  expect(update.status()).toBe(200)
  await thread.getByRole('button', { name: 'Uložit změny komentáře', exact: true }).click()
  await expect(thread.getByRole('alert')).toContainText('Komentář mezitím upravil jiný administrátor.')
  await expect(thread.getByLabel('Text komentáře', { exact: true })).toHaveValue('Můj rozepsaný text')
  await thread.getByRole('button', { name: 'Obnovit komentáře', exact: true }).click()
  await expect(comment).toContainText('Souběžně uložená nová informace.')
  await expect(thread.getByLabel('Text komentáře', { exact: true })).toHaveValue('Můj rozepsaný text')
  page.once('dialog', dialog => dialog.accept())
  await thread.getByRole('button', { name: 'Zrušit úpravy komentáře', exact: true }).click()
  await expect(thread.getByLabel('Jméno autora', { exact: true })).toHaveValue('Petr Svoboda, obec Vyskeř')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(errors).toEqual([])
})
