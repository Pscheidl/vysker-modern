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
  await email.fill('pending-subscriber@vysker.test')
  const initialBackground = await submit.evaluate(button => getComputedStyle(button).backgroundColor)
  await submit.click()
  await expect(submit).toHaveText('Odesílám…')
  await expect(submit).toBeDisabled()
  await expect(email).toBeDisabled()
  await expect.poll(() => requests.length).toBe(1)
  await form.dispatchEvent('submit')
  await form.dispatchEvent('submit')
  release()
  await expect(submit).toHaveText('Zkontrolujte e-mail')
  await expect(submit).toHaveClass(/\bnewsletter-confirmed\b/)
  await expect(submit).toBeDisabled()
  await expect(email).toHaveValue('')
  await expect(email).toBeEnabled()
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
