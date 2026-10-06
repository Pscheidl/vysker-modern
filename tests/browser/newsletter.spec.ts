import { test, expect, type Page } from './fixtures'

const subscriptionEndpoint = /\/api\/subscribe_email[^/?]*$/
const successMessage = 'Pokud je potřeba odběr potvrdit, pošleme vám ověřovací e-mail. Zkontrolujte svou schránku.'

async function openNewsletter(page: Page) {
  await page.goto('/odber')
  const form = page.locator('.newsletter-form')
  const email = form.getByLabel('E-mailová adresa', { exact: true })
  const submit = form.getByRole('button')
  await expect(submit).toBeEnabled()
  return { form, email, submit }
}

test('newsletter validates email before sending, including direct submit events', async ({ page }) => {
  const requests: URLSearchParams[] = []
  // Leptos uses a generated server-function endpoint for this public form.
  // Intercept it so these UI regressions never create subscribers or mail.
  await page.route(subscriptionEndpoint, async route => {
    requests.push(new URLSearchParams(route.request().postData() ?? ''))
    await route.fulfill({ contentType: 'application/json', body: 'null' })
  })
  const { form, email, submit } = await openNewsletter(page)
  for (const invalid of ['', 'bez-zavinace', 'obec@', 'obec@vysker', 'obec@@vysker.cz']) {
    await email.fill(invalid)
    expect(await email.evaluate(input => (input as HTMLInputElement).checkValidity())).toBe(false)
    await submit.click()
    await form.dispatchEvent('submit')
    await expect(email).toHaveValue(invalid)
  }
  expect(requests).toHaveLength(0)
  await email.fill('valid-subscriber@vysker.test')
  await email.press('Enter')
  await expect(form.getByRole('status')).toHaveText(successMessage)
  expect(requests).toHaveLength(1)
  expect(requests[0].get('email')).toBe('valid-subscriber@vysker.test')
  expect(requests[0].get('fingerprint')).toMatch(/^[a-f0-9]{64}$/)
  expect(requests[0].get('preferences[all_notice_categories]')).toBe('true')
  expect(requests[0].get('preferences[uncategorized_notices]')).toBe('true')
  expect(requests[0].get('preferences[documents]')).toBe('true')
  expect([...requests[0].keys()].filter(key => key.startsWith('preferences[notice_category_ids]'))).toEqual([])
})

test('newsletter prevents duplicate pending requests and clears email with temporary success feedback', async ({ page }) => {
  const requests: string[] = []
  let release!: () => void
  const held = new Promise<void>(resolve => { release = resolve })
  await page.route(subscriptionEndpoint, async route => {
    requests.push(route.request().postData() ?? '')
    await held
    await route.fulfill({ contentType: 'application/json', body: 'null' })
  })
  const { form, email, submit } = await openNewsletter(page)
  await page.clock.install({ time: new Date('2026-01-01T12:00:00Z') })
  await page.clock.pauseAt(new Date('2026-01-01T12:00:01Z'))
  await form.locator('summary').click()
  await expect(form.getByLabel('Všechny kategorie úřední desky (i budoucí)', { exact: true })).toBeChecked()
  await expect(form.getByLabel('Úřední deska bez kategorie', { exact: true })).toBeChecked()
  await expect(form.getByLabel('Obecné dokumenty', { exact: true })).toBeChecked()
  await expect(form.getByLabel('Rozpočet', { exact: true })).toBeDisabled()
  await expect(form.getByLabel('Rozpočet', { exact: true })).toBeChecked()
  await email.fill('pending-subscriber@vysker.test')
  const initialBackground = await submit.evaluate(button => getComputedStyle(button).backgroundColor)
  await submit.click()
  await expect(submit).toHaveText('Odesílám…')
  await expect(submit).toBeDisabled()
  await expect(email).toBeDisabled()
  for (const checkbox of await form.getByRole('checkbox').all()) {
    await expect(checkbox).toBeDisabled()
  }
  await expect.poll(() => requests.length).toBe(1)
  await form.dispatchEvent('submit')
  await form.dispatchEvent('submit')
  release()
  await expect(submit).toHaveText('Zkontrolujte e-mail')
  await expect(submit).toHaveClass(/\bnewsletter-confirmed\b/)
  await expect(submit).toBeDisabled()
  await expect(email).toHaveValue('')
  await expect(email).toBeEnabled()
  await expect(form.getByLabel('Obecné dokumenty', { exact: true })).toBeEnabled()
  await expect(form.getByRole('status')).toHaveText(successMessage)
  await expect.poll(() => submit.evaluate(button => getComputedStyle(button).backgroundColor)).not.toBe(initialBackground)
  await form.dispatchEvent('submit')
  expect(requests).toHaveLength(1)
  await page.clock.fastForward(4_999)
  await expect(submit).toHaveText('Zkontrolujte e-mail')
  await expect(submit).toBeDisabled()
  await page.clock.fastForward(1)
  await expect(submit).toHaveText('Přihlásit k odběru')
  await expect(submit).toBeEnabled()
  await expect(submit).not.toHaveClass(/\bnewsletter-confirmed\b/)
  await expect(form.getByRole('status')).toHaveText(successMessage)
  expect(requests).toHaveLength(1)
})

test('newsletter sends the selected categories and allows documents only', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  const requests: URLSearchParams[] = []
  await page.route(subscriptionEndpoint, async route => {
    requests.push(new URLSearchParams(route.request().postData() ?? ''))
    await route.fulfill({ contentType: 'application/json', body: 'null' })
  })
  const { form, email, submit } = await openNewsletter(page)
  await form.locator('summary').click()
  await expect(form.getByRole('group', { name: 'Vyberte témata odběru', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await form.getByLabel('Všechny kategorie úřední desky (i budoucí)', { exact: true }).uncheck()
  await form.getByLabel('Úřední deska bez kategorie', { exact: true }).uncheck()
  await form.getByLabel('Obecné dokumenty', { exact: true }).uncheck()
  const budget = form.getByLabel('Rozpočet', { exact: true })
  await expect(budget).toBeEnabled()
  await budget.focus()
  await expect(budget).toBeFocused()
  await page.keyboard.press('Space')
  await expect(budget).toBeChecked()
  const categoryId = await budget.inputValue()
  await email.fill('category-subscriber@vysker.test')
  await submit.click()
  await expect(form.getByRole('status')).toHaveText(successMessage)
  expect(requests).toHaveLength(1)
  expect(requests[0].get('preferences[all_notice_categories]')).toBe('false')
  expect(requests[0].get('preferences[notice_category_ids][0]')).toBe(categoryId)
  expect(requests[0].get('preferences[uncategorized_notices]')).toBe('false')
  expect(requests[0].get('preferences[documents]')).toBe('false')

  await email.fill('documents-subscriber@vysker.test')
  await budget.uncheck()
  await form.getByLabel('Obecné dokumenty', { exact: true }).check()
  await submit.click()
  await expect(form.getByRole('status')).toHaveText(successMessage)
  expect(requests).toHaveLength(2)
  expect(requests[1].get('preferences[all_notice_categories]')).toBe('false')
  expect(requests[1].get('preferences[uncategorized_notices]')).toBe('false')
  expect(requests[1].get('preferences[documents]')).toBe('true')
  expect([...requests[1].keys()].filter(key => key.startsWith('preferences[notice_category_ids]'))).toEqual([])
})

test('newsletter rejects an empty selection and preserves the email for correction', async ({ page }) => {
  const requests: string[] = []
  await page.route(subscriptionEndpoint, async route => {
    requests.push(route.request().postData() ?? '')
    await route.fulfill({ contentType: 'application/json', body: 'null' })
  })
  const { form, email, submit } = await openNewsletter(page)
  await form.locator('summary').click()
  await form.getByLabel('Všechny kategorie úřední desky (i budoucí)', { exact: true }).uncheck()
  await form.getByLabel('Úřední deska bez kategorie', { exact: true }).uncheck()
  await form.getByLabel('Obecné dokumenty', { exact: true }).uncheck()
  await form.locator('summary').click()
  await email.fill('empty-selection@vysker.test')
  await submit.click()
  await expect(form.getByRole('alert')).toHaveText('Vyberte alespoň jednu kategorii úřední desky, úřední desku bez kategorie nebo obecné dokumenty.')
  await expect(form.locator('details')).toHaveAttribute('open', '')
  await expect(email).toHaveValue('empty-selection@vysker.test')
  await form.dispatchEvent('submit')
  expect(requests).toHaveLength(0)
  await form.getByLabel('Úřední deska bez kategorie', { exact: true }).check()
  await expect(form.getByRole('alert')).toHaveCount(0)
  await submit.click()
  await expect(form.getByRole('status')).toHaveText(successMessage)
  expect(requests).toHaveLength(1)
})

test('newsletter keeps email after a failed request and permits retry and another address', async ({ page }) => {
  const requests: URLSearchParams[] = []
  await page.route(subscriptionEndpoint, async route => {
    requests.push(new URLSearchParams(route.request().postData() ?? ''))
    if (requests.length === 1) {
      await route.abort('failed')
    } else {
      await route.fulfill({ contentType: 'application/json', body: 'null' })
    }
  })
  const { form, email, submit } = await openNewsletter(page)
  await email.fill('retry-subscriber@vysker.test')
  await submit.click()
  await expect(form.getByRole('alert')).toBeVisible()
  await expect(form.getByRole('alert')).not.toHaveText('')
  await expect(email).toHaveValue('retry-subscriber@vysker.test')
  await expect(email).toBeEnabled()
  await expect(submit).toHaveText('Přihlásit k odběru')
  await expect(submit).toBeEnabled()
  await expect(submit).not.toHaveClass(/\bnewsletter-confirmed\b/)
  await submit.click()
  await expect(form.getByRole('status')).toHaveText(successMessage)
  await expect(form.getByRole('alert')).toHaveCount(0)
  await expect(email).toHaveValue('')
  await expect(submit).toHaveText('Zkontrolujte e-mail')
  expect(requests.map(request => request.get('email'))).toEqual([
    'retry-subscriber@vysker.test',
    'retry-subscriber@vysker.test',
  ])
  await email.fill('another-subscriber@vysker.test')
  await expect(submit).toHaveText('Přihlásit k odběru')
  await expect(submit).toBeEnabled()
  await expect(form.getByRole('status')).toHaveCount(0)
  await email.press('Enter')
  await expect(form.getByRole('status')).toHaveText(successMessage)
  await expect(email).toHaveValue('')
  expect(requests).toHaveLength(3)
  expect(requests[2].get('email')).toBe('another-subscriber@vysker.test')
})
