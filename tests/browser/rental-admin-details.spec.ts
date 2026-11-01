import { test, expect, type Page } from './fixtures'

// The invoice clock is authoritative Prague time even for administrators abroad.
test.use({ timezoneId: 'America/New_York' })

async function prepareInvoice(page: Page, requireMeter: boolean) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill('e2e-admin@vysker.test')
  await page.getByLabel('Heslo', { exact: true }).fill('Disposable-browser-test-password!')
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
  await page.clock.install({ time: new Date('2031-10-28T09:59:57Z') })
  await page.clock.pauseAt(new Date('2031-10-28T09:59:58Z'))
  await page.route('**/api/v1/admin/rentals/777777', route => route.fulfill({ json: {
    id: 777777, asset_id: 1, asset_name: 'Sál pro ověření času',
    start_date: '2031-10-28', end_date: '2031-10-28', start_time: '10:00', end_time: '11:00',
    invoice_available_at: '2031-10-28T11:00:00', invoice_available_unix: Date.parse('2031-10-28T10:00:00Z') / 1000,
    source: 'admin', status: 'approved', name: 'Testovací nájemník', email: 'test@example.test', phone: '+420 777 123 456',
    address: 'Testovací adresa 1', tenant_type: 'person', require_meter: requireMeter, wait_for_payment: true,
    before_uploaded: false, after_uploaded: false, rental_price: 250, event_type: 'standard',
  } }))
  await page.goto('/admin/pronajmy/777777')
  await expect(page.getByRole('heading', { name: 'Připravit vyúčtování' })).toBeVisible()
}

test('invoice becomes available at its Prague deadline without losing the prepared draft', async ({ page }) => {
  await prepareInvoice(page, false)
  const submit = page.getByRole('button', { name: 'Odeslat vyúčtování', exact: true })
  await expect(page.locator('[data-invoice-available]')).toHaveText('Vyúčtování lze odeslat od 28. 10. 2031 v 11:00.')
  await expect(submit).toBeDisabled()
  const extra = page.getByLabel('Další náklady v Kč', { exact: true })
  await extra.fill('75')
  const invoice = page.getByLabel('Faktura od účetní (nepovinné PDF)')
  await invoice.setInputFiles({ name: 'rozpracovana-faktura.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4\n%%EOF\n') })
  await page.clock.fastForward(1_000)
  await expect(submit).toBeDisabled()
  await page.clock.fastForward(1_000)
  await expect(submit).toBeEnabled()
  await expect(extra).toHaveValue('75')
  await expect(invoice).not.toHaveValue('')
  await expect(page.locator('[data-invoice-filename]')).toHaveText('rozpracovana-faktura.pdf')
  await expect(page.getByText('Podklady můžete připravit už nyní.', { exact: false })).toHaveCount(0)
})

test('missing meter photos keep invoice submission disabled after the rental ends', async ({ page }) => {
  await prepareInvoice(page, true)
  const submit = page.getByRole('button', { name: 'Odeslat vyúčtování', exact: true })
  await expect(page.locator('[data-invoice-missing]')).toContainText('před pronájmem i po něm')
  await expect(page.getByLabel('Spotřeba elektřiny v kWh')).toBeEnabled()
  await page.clock.fastForward(5_000)
  await expect(submit).toBeDisabled()
  await expect(page.getByText('Podklady můžete připravit už nyní.', { exact: false })).toHaveCount(0)
})
