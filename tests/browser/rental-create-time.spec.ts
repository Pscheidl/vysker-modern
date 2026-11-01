import { test, expect } from './fixtures'
import { testRates } from './rental-test-data'

test('administrator cannot preview past days or hours and an expired preview cannot be saved', async ({ page }) => {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  await page.clock.install({ time: new Date('2030-06-03T12:59:59Z') })
  await page.clock.pauseAt(new Date('2030-06-03T12:59:59Z'))
  await page.route('**/api/v1/admin/rental-assets', route => route.fulfill({ json: [{
    id: 1, name: 'Zkušební sál', description: '', rates: testRates(), require_meter: false,
    notification_email: 'office@example.test', active: true,
  }] }))
  let previews = 0
  let creates = 0
  await page.route('**/api/v1/admin/rentals/preview', route => {
    previews++
    const body = route.request().postDataJSON()
    return route.fulfill({ json: {
      occurrences: [{ start_date: body.start_date, end_date: body.end_date, start_time: body.start_time, end_time: body.end_time, conflict: false }],
      conflict_count: 0, total_price: 0, occurrence_count: 1,
    } })
  })
  await page.route('**/api/v1/admin/rentals', route => { creates++; return route.fulfill({ status: 201, json: { rentals: [{ id: 1 }] } }) })
  await page.goto('/admin/pronajmy/nova')
  await page.getByRole('combobox', { name: 'Předmět pronájmu', exact: true }).selectOption('1')
  const mode = page.getByRole('combobox', { name: 'Délka pronájmu', exact: true })
  const start = page.getByLabel('Datum', { exact: true })
  const from = page.getByRole('combobox', { name: 'Od hodin', exact: true })
  const to = page.getByRole('combobox', { name: 'Do hodin', exact: true })
  const check = page.getByRole('button', { name: 'Zkontrolovat termíny', exact: true })
  await start.fill('2030-06-03')
  await expect(start).toHaveAttribute('min', '2030-06-03')
  await expect(from.locator('option[value="14:00"]')).toHaveJSProperty('disabled', true)
  await expect(from.locator('option[value="15:00"]')).toHaveJSProperty('disabled', false)
  await from.selectOption('15:00')
  await expect(to.locator('option[value="15:00"]')).toHaveJSProperty('disabled', true)
  await to.selectOption('16:00')
  await page.getByLabel('Jméno a příjmení', { exact: true }).fill('Jan Zkušební')
  await page.getByLabel('E-mail pro vyúčtování').fill('jan@example.test')
  await page.getByLabel('Telefon', { exact: true }).fill('+420 777 123 456')
  await page.getByLabel('Adresa', { exact: true }).fill('Testovací adresa 1')
  await page.getByLabel('Sjednaná cena za jeden termín v Kč').fill('0')
  await check.click()
  await expect(page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' })).toBeVisible()
  expect(previews).toBe(1)
  await from.focus()
  await page.clock.fastForward(1_000)
  await expect(from).toBeFocused()
  await expect(from.locator('option[value="15:00"]')).toHaveJSProperty('disabled', true)
  await expect(from).toHaveValue('')
  await expect(to).toHaveValue('')
  await expect(check).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Potvrdit a vytvořit rezervaci' })).toHaveCount(0)
  await page.locator('.rental-create-form').dispatchEvent('submit')
  expect(previews).toBe(1)
  expect(creates).toBe(0)

  await mode.selectOption('daily')
  await expect(start).toHaveAttribute('min', '2030-06-04')
  await expect(page.getByLabel('Do včetně', { exact: true })).toHaveAttribute('min', '2030-06-04')
  await expect(check).toBeDisabled()
  await start.fill('2030-06-02')
  await page.locator('.rental-create-form').dispatchEvent('submit')
  expect(previews).toBe(1)
  await start.fill('2030-06-04')
  await expect(check).toBeEnabled()
})
