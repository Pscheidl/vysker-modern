import { test, expect, type Page } from './fixtures'

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('create, publish, edit and cancel an event with a public feed and permanent detail', async ({ page, browser }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await login(page)
  await page.goto('/admin/kalendar')
  await page.getByRole('link', { name: 'Nová akce', exact: true }).click()
  const title = `Sousedské setkání ${Date.now()}`
  const revisedTitle = `${title} po úpravě`
  const tomorrow = new Date(Date.now() + 86_400_000).toISOString().slice(0, 10)
  await page.getByLabel('Název akce').fill(title)
  await page.getByLabel('Místo konání', { exact: true }).fill('Náves ve Vyskři')
  await page.getByLabel('Popis akce', { exact: true }).fill('Společné odpoledne pro sousedy.')
  await page.getByLabel('Začátek').fill(`${tomorrow}T12:00`)
  await page.getByLabel('Konec').fill(`${tomorrow}T14:00`)
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/kalendar\/\d+$/)
  await expect(page.getByText('Verze 1', { exact: true })).toBeVisible()
  const id = page.url().split('/').pop()!
  const origin = new URL(page.url()).origin
  const publicContext = await browser.newContext()
  const publicPage = await publicContext.newPage()
  publicPage.on('pageerror', error => errors.push(error.message))
  try {
    // The independent context has no administrator session.
    const hidden = await publicPage.request.get(`${origin}/api/v1/events/${id}`)
    expect(hidden.status()).toBe(404)
    await publicPage.goto(`${origin}/kalendar/${id}`)
    await expect(publicPage.getByRole('heading', { name: title, exact: true })).toHaveCount(0)
    await publicPage.goto(`${origin}/`)
    await expect(publicPage.locator(`.events-strip a[href="/kalendar/${id}"]`)).toHaveCount(0)

    await page.getByLabel('Zveřejnit akci', { exact: true }).check()
    await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
    await expect(page.getByText('Verze 2', { exact: true })).toBeVisible()
    await publicPage.goto(`${origin}/kalendar`)
    await publicPage.getByRole('link', { name: title, exact: true }).click()
    await expect(publicPage).toHaveURL(new RegExp(`/kalendar/${id}$`))
    await expect(publicPage.getByRole('heading', { name: title, exact: true })).toBeVisible()
    await expect(publicPage.locator('.event-detail')).toContainText('Náves ve Vyskři')
    await expect(publicPage.locator('.event-detail')).toContainText('Společné odpoledne pro sousedy.')
    await expect(publicPage.locator('.event-times')).toContainText('12:00')

    await publicPage.goto(`${origin}/`)
    const feed = publicPage.getByRole('region', { name: 'Dění v obci' })
    await expect(feed.locator(`a[href="/kalendar/${id}"]`)).toContainText(title)
    await feed.locator(`a[href="/kalendar/${id}"]`).click()
    await expect(publicPage.getByRole('heading', { name: title, exact: true })).toBeVisible()

    await page.getByLabel('Název akce').fill(revisedTitle)
    await page.getByLabel('Místo konání', { exact: true }).fill('Kulturní sál')
    await page.getByLabel('Popis akce', { exact: true }).fill('Aktualizovaný program setkání.')
    await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
    await expect(page.getByText('Verze 3', { exact: true })).toBeVisible()
    await publicPage.reload()
    await expect(publicPage.getByRole('heading', { name: revisedTitle, exact: true })).toBeVisible()
    await expect(publicPage.locator('.event-detail')).toContainText('Kulturní sál')
    await expect(publicPage.locator('.event-detail')).toContainText('Aktualizovaný program setkání.')

    await page.getByLabel('Akce je zrušena', { exact: true }).check()
    await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
    await expect(page.getByText('Verze 4', { exact: true })).toBeVisible()
    await publicPage.reload()
    await expect(publicPage.getByRole('heading', { name: revisedTitle, exact: true })).toBeVisible()
    await expect(publicPage.getByText('Akce je zrušena.', { exact: true })).toBeVisible()
    await publicPage.goto(`${origin}/kalendar`)
    const cancelled = publicPage.locator('.event-row').filter({ has: publicPage.getByRole('link', { name: revisedTitle, exact: true }) })
    await expect(cancelled).toContainText('Zrušeno')
    await publicPage.goto(`${origin}/`)
    await expect(publicPage.locator(`.events-strip a[href="/kalendar/${id}"]`)).toContainText('Zrušeno')
    await expect(publicPage.locator(`.events-strip a[href="/kalendar/${id}"]`)).toContainText(revisedTitle)
    expect(errors).toEqual([])
  } finally {
    await publicContext.close()
  }
})

test('invalid Prague DST time keeps the form and a version conflict keeps both editors safe', async ({ page }) => {
  await login(page)
  await page.goto('/admin/kalendar/nove')
  const title = `Kontrola časů ${Date.now()}`
  await page.getByLabel('Název akce').fill(title)
  await page.getByLabel('Začátek').fill('2030-03-31T02:30')
  await page.getByLabel('Konec').fill('2030-03-31T04:00')
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('neexistuje')
  await expect(page.getByLabel('Název akce')).toHaveValue(title)
  await page.getByLabel('Začátek').fill('2030-03-31T03:00')
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/kalendar\/\d+$/)
  await expect(page.getByText('Verze 1', { exact: true })).toBeVisible()
  const other = await page.context().newPage()
  try {
    await other.goto(page.url())
    await expect(other.getByText('Verze 1', { exact: true })).toBeVisible()
    await other.getByLabel('Název akce').fill(`${title} uložená verze`)
    await other.getByRole('button', { name: 'Uložit akci', exact: true }).click()
    await expect(other.getByText('Verze 2', { exact: true })).toBeVisible()
    await page.getByLabel('Název akce').fill(`${title} rozepsané změny`)
    await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
    await expect(page.getByRole('region', { name: 'Konflikt úprav' })).toBeVisible()
    await expect(page.getByLabel('Název akce')).toHaveValue(`${title} rozepsané změny`)
    await expect(page.getByRole('button', { name: 'Uložit akci', exact: true })).toBeDisabled()
    await page.getByRole('button', { name: 'Zobrazit aktuální verzi pro porovnání', exact: true }).click()
    await expect(page.getByRole('region', { name: 'Konflikt úprav' })).toContainText(`${title} uložená verze`)
    await expect(page.getByLabel('Název akce')).toHaveValue(`${title} rozepsané změny`)
    await page.getByRole('button', { name: 'Načíst aktuální verzi', exact: true }).click()
    await page.getByRole('dialog').getByRole('button', { name: 'Načíst aktuální verzi', exact: true }).click()
    await expect(page.getByText('Verze 2', { exact: true })).toBeVisible()
    await expect(page.getByLabel('Název akce')).toHaveValue(`${title} uložená verze`)
  } finally {
    await other.close()
  }
})
