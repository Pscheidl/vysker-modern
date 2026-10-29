import { readFileSync } from 'node:fs'
import { test, expect, type Page } from './fixtures'

async function login(page: Page) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

test('all-day event opens one copy with browser download fallback and links to a subscription', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await login(page)
  await page.goto('/admin/kalendar/nove')
  const title = `Svoz odpadu ${Date.now()}`
  await page.getByLabel('Název akce').fill(title)
  await page.getByLabel('Místo konání', { exact: true }).fill('Vyskeř a místní části')
  await page.getByLabel('Popis akce', { exact: true }).fill('Přistavte nádoby před dům.')
  await page.getByLabel('Téma kalendáře', { exact: true }).selectOption('waste')
  await page.getByLabel('Celodenní akce', { exact: true }).check()
  await expect(page.getByLabel('Začátek')).toHaveAttribute('type', 'date')
  await expect(page.getByLabel('Konec')).toHaveAttribute('type', 'date')
  // The last included day crosses Prague's daylight-saving transition.
  await page.getByLabel('Začátek').fill('2031-03-29')
  await page.getByLabel('Konec').fill('2031-03-30')
  await page.getByLabel('Zveřejnit akci', { exact: true }).check()
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/kalendar\/\d+$/)
  await expect(page.getByText('Verze 1', { exact: true })).toBeVisible()
  const id = page.url().split('/').pop()!
  const origin = new URL(page.url()).origin
  await page.reload()
  await expect(page.getByLabel('Celodenní akce', { exact: true })).toBeChecked()
  await expect(page.getByLabel('Téma kalendáře', { exact: true })).toHaveValue('waste')
  await expect(page.getByLabel('Začátek')).toHaveValue('2031-03-29')
  await expect(page.getByLabel('Konec')).toHaveValue('2031-03-30')

  await page.goto('/kalendar')
  const overviewAction = page.getByRole('link', { name: `Přidat do kalendáře: ${title}`, exact: true })
  expect(await overviewAction.getAttribute('download')).toBeNull()
  await expect(overviewAction).toHaveAttribute('rel', 'external')
  // Chromium in CI has no calendar handler, so its native fallback is a download.
  const overviewDownloadEvent = page.waitForEvent('download')
  await overviewAction.click()
  expect(await (await overviewDownloadEvent).failure()).toBeNull()
  await expect(page).toHaveURL(/\/kalendar$/)

  await page.goto(`/kalendar/${id}`)
  await expect(page.locator('.event-times')).toContainText('celý den')
  await expect(page.locator('.event-times')).not.toContainText('00:00')
  await expect(page.locator('.event-detail .event-category')).toHaveText('Svozy odpadu')
  await expect(page.getByRole('region', { name: 'Uložit akci do kalendáře' })).toContainText('jednorázovou kopii')
  const addAction = page.getByRole('link', { name: 'Přidat do kalendáře', exact: true })
  expect(await addAction.getAttribute('download')).toBeNull()
  await expect(addAction).toHaveAttribute('rel', 'external')
  const response = await page.request.get(`/api/v1/events/${id}/calendar.ics`)
  expect(response.headers()['content-disposition']).toBe('inline; filename="akce.ics"')
  expect(response.headers()['content-type']).toBe('text/calendar; charset=utf-8')
  const downloadEvent = page.waitForEvent('download')
  await page.getByRole('link', { name: 'Přidat do kalendáře', exact: true }).click()
  const download = await downloadEvent
  const path = await download.path()
  expect(path).not.toBeNull()
  const calendar = readFileSync(path!, 'utf8').replace(/\r\n[ \t]/g, '')
  expect(calendar.match(/BEGIN:VEVENT/g)).toHaveLength(1)
  expect(calendar).toContain(`SUMMARY:${title}`)
  expect(calendar).toContain('DTSTART;VALUE=DATE:20310329')
  expect(calendar).toContain('DTEND;VALUE=DATE:20310331')
  expect(calendar).toContain(`URL:${origin}/kalendar/${id}`)
  await page.getByRole('link', { name: 'Chci odebírat kalendář i se změnami' }).click()
  await expect(page.locator('#odber-kalendare')).toHaveAttribute('open', '')
  await expect(page.getByLabel('Adresa pro odběr', { exact: true })).toHaveValue(`${origin}/api/v1/calendar.ics`)
  await page.getByLabel('Co chcete odebírat?', { exact: true }).selectOption('waste')
  await expect(page.getByLabel('Adresa pro odběr', { exact: true })).toHaveValue(`${origin}/api/v1/calendar.ics?category=waste`)
  const waste = await page.request.get('/api/v1/calendar.ics?category=waste')
  expect(waste.ok()).toBeTruthy()
  expect((await waste.text()).replace(/\r\n[ \t]/g, '')).toContain(`SUMMARY:${title}`)
  const culture = await page.request.get('/api/v1/calendar.ics?category=culture')
  expect((await culture.text()).replace(/\r\n[ \t]/g, '')).not.toContain(`SUMMARY:${title}`)
  expect(errors).toEqual([])
})

test('subscription controls work by keyboard and fit a phone in both themes', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/kalendar')
  const summary = page.locator('#odber-kalendare > summary')
  await summary.focus()
  await page.keyboard.press('Enter')
  const topic = page.getByLabel('Co chcete odebírat?', { exact: true })
  await expect(topic).toBeVisible()
  const origin = new URL(page.url()).origin
  for (const category of ['', 'waste', 'culture', 'council']) {
    await topic.selectOption(category)
    const suffix = category ? `?category=${category}` : ''
    await expect(page.getByLabel('Adresa pro odběr', { exact: true })).toHaveValue(`${origin}/api/v1/calendar.ics${suffix}`)
    await expect(page.getByRole('link', { name: 'Odebírat v aplikaci', exact: true })).toHaveAttribute('href', `${origin.replace(/^https?:/, 'webcal:')}/api/v1/calendar.ics${suffix}`)
  }
  const address = page.getByLabel('Adresa pro odběr', { exact: true })
  await address.focus()
  expect(await address.evaluate(input => {
    const field = input as HTMLInputElement
    return field.selectionStart === 0 && field.selectionEnd === field.value.length
  })).toBeTruthy()
  await expect(page.getByRole('link', { name: 'Návod Apple', exact: true })).toHaveAttribute('href', 'https://support.apple.com/cs-cz/guide/iphone/iph3d1110d4/ios')
  await expect(page.getByRole('link', { name: 'Návod Google', exact: true })).toHaveAttribute('href', /support\.google\.com/)
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy()
    await expect(page.getByRole('link', { name: 'Odebírat v aplikaci', exact: true })).toBeVisible()
  }
  expect(errors).toEqual([])
})


test('fortnightly collection keeps its first date and shows its next occurrence until the series ends', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const today = new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Europe/Prague', year: 'numeric', month: '2-digit', day: '2-digit',
  }).format(new Date())
  const day = (offset: number) => {
    const date = new Date(`${today}T12:00:00Z`)
    date.setUTCDate(date.getUTCDate() + offset)
    return date.toISOString().slice(0, 10)
  }
  const first = day(-27)
  const next = day(1)
  const until = day(29)
  const title = `Pravidelný svoz odpadu ${Date.now()}`

  await login(page)
  await page.goto('/admin/kalendar/nove')
  await page.getByLabel('Název akce').fill(title)
  await page.getByLabel('Téma kalendáře', { exact: true }).selectOption('waste')
  await page.getByLabel('Celodenní akce', { exact: true }).check()
  await page.getByLabel('Začátek').fill(first)
  await page.getByLabel('Konec').fill(first)
  await page.getByLabel('Opakovat každých 14 dní', { exact: true }).check()
  await expect(page.getByText('Začátek a konec výše patří prvnímu termínu.', { exact: false })).toContainText('celou řadu')
  await page.getByLabel('Opakovat do', { exact: true }).fill(until)
  await page.getByLabel('Zveřejnit akci', { exact: true }).check()
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page).toHaveURL(/\/admin\/kalendar\/\d+$/)
  await expect(page.getByText('Verze 1', { exact: true })).toBeVisible()
  const id = page.url().split('/').pop()!
  await page.reload()
  await expect(page.getByLabel('Začátek')).toHaveValue(first)
  await expect(page.getByLabel('Konec')).toHaveValue(first)
  await expect(page.getByLabel('Opakovat každých 14 dní', { exact: true })).toBeChecked()
  await expect(page.getByLabel('Opakovat do', { exact: true })).toHaveValue(until)

  await page.goto('/kalendar')
  const row = page.locator('.event-row').filter({ has: page.getByRole('link', { name: title, exact: true }) })
  await expect(row).toHaveCount(1)
  await expect(row.locator('.event-times time').first()).toHaveAttribute('datetime', next)
  await expect(row.locator('.event-recurrence')).toContainText('každých 14 dní')
  await expect(row.getByRole('link', { name: `Přidat do kalendáře: ${title}`, exact: true })).toHaveAttribute('title', /celé řady/)
  await page.goto('/')
  const homeEvent = page.locator(`.events-strip a[href="/kalendar/${id}"]`)
  await expect(homeEvent.locator('time')).toHaveAttribute('datetime', next)
  await expect(homeEvent).toContainText('každých 14 dní')
  await page.goto(`/kalendar/${id}`)
  await expect(page.locator('.event-times time').first()).toHaveAttribute('datetime', next)
  await expect(page.getByRole('region', { name: 'Uložit akci do kalendáře' })).toContainText('celá řada opakování')
  const response = await page.request.get(`/api/v1/events/${id}/calendar.ics`)
  expect(response.ok()).toBeTruthy()
  const calendar = (await response.text()).replace(/\r\n[ \t]/g, '')
  expect(calendar.match(/BEGIN:VEVENT/g)).toHaveLength(1)
  expect(calendar).toContain(`DTSTART;VALUE=DATE:${first.replaceAll('-', '')}`)
  expect(calendar).toContain(`RRULE:FREQ=WEEKLY;INTERVAL=2;UNTIL=${until.replaceAll('-', '')}`)

  // Editing a series keeps its original anchor and can move its final occurrence into the archive.
  await page.goto(`/admin/kalendar/${id}`)
  await expect(page.getByLabel('Začátek')).toHaveValue(first)
  await page.getByLabel('Opakovat do', { exact: true }).fill(day(-13))
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page.getByText('Verze 2', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Začátek')).toHaveValue(first)
  await expect(page.getByLabel('Opakovat do', { exact: true })).toHaveValue(day(-13))
  await page.goto('/kalendar')
  await expect(page.getByRole('link', { name: title, exact: true })).toHaveCount(0)
  await page.getByRole('link', { name: 'Archiv akcí', exact: true }).click()
  const archived = page.locator('.event-row').filter({ has: page.getByRole('link', { name: title, exact: true }) })
  await expect(archived).toHaveCount(1)
  await expect(archived.locator('.event-times time').first()).toHaveAttribute('datetime', day(-13))
  await page.goto(`/kalendar/${id}`)
  await expect(page.locator('.event-times time').first()).toHaveAttribute('datetime', day(-13))

  // Disabling repetition removes its limit from the saved data and the exported calendar.
  await page.goto(`/admin/kalendar/${id}`)
  await page.getByLabel('Opakovat každých 14 dní', { exact: true }).uncheck()
  await expect(page.getByLabel('Opakovat do', { exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Uložit akci', exact: true }).click()
  await expect(page.getByText('Verze 3', { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Opakovat každých 14 dní', { exact: true })).not.toBeChecked()
  await page.getByLabel('Opakovat každých 14 dní', { exact: true }).check()
  await expect(page.getByLabel('Opakovat do', { exact: true })).toHaveValue('')
  const single = await page.request.get(`/api/v1/events/${id}/calendar.ics`)
  expect(await single.text()).not.toContain('RRULE:')
  expect(errors).toEqual([])
})
