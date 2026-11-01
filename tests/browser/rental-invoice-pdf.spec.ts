import { test, expect, type Page } from './fixtures'
import { testRates } from './rental-test-data'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createServer, type Socket } from 'node:net'

const adminPassword = 'Disposable-browser-test-password!'
const limitedPassword = 'Dlouhe-heslo-noveho-spravce-2026!'

function pdfFixture() {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Count 1 /Kids [3 0 R] >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>',
  ]
  // A PDF binary marker makes this fixture representative of normal invoices
  // and ensures SMTP uses base64 without altering its bytes.
  let body = '%PDF-1.4\n%\xE2\xE3\xCF\xD3\n'
  const offsets = [0]
  for (const [index, object] of objects.entries()) {
    offsets.push(Buffer.byteLength(body, 'latin1'))
    body += `${index + 1} 0 obj\n${object}\nendobj\n`
  }
  const xref = Buffer.byteLength(body, 'latin1')
  body += `xref\n0 ${offsets.length}\n0000000000 65535 f \n`
  for (const offset of offsets.slice(1)) body += `${String(offset).padStart(10, '0')} 00000 n \n`
  body += `trailer\n<< /Size ${offsets.length} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
  return Buffer.from(body, 'latin1')
}

function setRentalInPast(id: number) {
  const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
  execFileSync('python3', ['-c', [
    'import psycopg, sys',
    'with psycopg.connect(sys.argv[1]) as connection:',
    '    if not connection.execute("SELECT current_database()").fetchone()[0].startswith("vysker_test_"):',
    '        raise RuntimeError("Expected a disposable browser-test database")',
    '    connection.execute("UPDATE rentals SET start_date = CURRENT_DATE - 1, end_date = CURRENT_DATE - 1 WHERE id = %s", (int(sys.argv[2]),))',
  ].join('\n'), database, String(id)])
}

async function login(page: Page, email: string, password: string) {
  await page.goto('/admin')
  await page.getByLabel('E-mail', { exact: true }).fill(email)
  await page.getByLabel('Heslo', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Přihlásit se', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Administrace' })).toBeVisible()
}

async function smtpCapture(port: number) {
  const messages: string[] = []
  const sockets = new Set<Socket>()
  const server = createServer(socket => {
    sockets.add(socket)
    socket.on('close', () => sockets.delete(socket))
    socket.on('error', () => {})
    socket.setEncoding('utf8')
    socket.write('220 localhost SMTP\r\n')
    let pending = ''
    let data = false
    let message = ''
    socket.on('data', chunk => {
      pending += chunk
      let end = pending.indexOf('\r\n')
      while (end !== -1) {
        const line = pending.slice(0, end)
        pending = pending.slice(end + 2)
        if (data) {
          if (line === '.') {
            messages.push(message)
            message = ''
            data = false
            socket.write('250 message accepted\r\n')
          } else {
            message += `${line.startsWith('..') ? line.slice(1) : line}\r\n`
          }
        } else if (line.toUpperCase() === 'DATA') {
          data = true
          socket.write('354 send message\r\n')
        } else if (line.toUpperCase() === 'QUIT') {
          socket.end('221 goodbye\r\n')
        } else {
          socket.write('250 localhost\r\n')
        }
        end = pending.indexOf('\r\n')
      }
    })
  })
  await new Promise<void>((resolve, reject) => server.once('error', reject).listen(port, '127.0.0.1', resolve))
  return { messages, close: async () => {
    for (const socket of sockets) socket.destroy()
    await new Promise<void>(resolve => server.close(() => resolve()))
  } }
}

test('optional accountant PDF is validated, access controlled and delivered with the settlement', async ({ page, browser }) => {
  test.setTimeout(120_000)
  let phase = 'startup'
  const pageErrors: string[] = []
  const browserErrors: string[] = []
  page.on('pageerror', error => pageErrors.push(`${phase}: ${error.stack || error.message}`))
  page.on('console', message => { if (message.type() === 'error') browserErrors.push(`${phase}: ${message.text()}`) })
  const smtpPort = Number(process.env.E2E_SMTP_PORT || '9')
  test.skip(smtpPort === 9, 'Set E2E_SMTP_PORT to an unused local port to test SMTP capture.')
  const smtp = await smtpCapture(smtpPort)
  const accountantPdf = pdfFixture()
  try {
    await login(page, 'e2e-admin@vysker.test', adminPassword)
    const session = await (await page.request.get('/api/v1/admin/session')).json()
    const headers = { 'X-CSRF-Token': session.csrf_token }
    const suffix = Date.now()
    const email = `faktura-${suffix}@vysker.test`
    const limited = await page.request.post('/api/v1/admin/accounts', { headers, data: {
      email: `bez-prava-${suffix}@vysker.test`, password: limitedPassword, current_password: adminPassword,
      can_view_identity: false, can_manage_permissions: false, can_create_accounts: false, can_manage_mail_settings: false,
    } })
    expect(limited.status()).toBe(201)
    const assetResponse = await page.request.post('/api/v1/admin/rental-assets', { headers, data: {
      name: `Sál pro fakturu ${suffix}`, description: '', rates: testRates(),
      notification_email: 'office@vysker.test', require_meter: false, active: true,
    } })
    expect(assetResponse.status()).toBe(200)
    const asset = await assetResponse.json()
    const monday = new Date(Date.now() + 14 * 86_400_000)
    monday.setUTCDate(monday.getUTCDate() + (1 - monday.getUTCDay() + 7) % 7)
    const day = monday.toISOString().slice(0, 10)
    const application = await page.request.post('/api/v1/rentals', { multipart: {
      asset_id: String(asset.id), start_date: day, end_date: day, start_time: '10:00', end_time: '11:00', event_type: 'standard', name: 'Nájemce s fakturou',
      email, phone: '+420 777 123 456', address: 'Vyskeř 1', consent: 'true',
      identity: { name: 'doklad.png', mimeType: 'image/png', buffer: readFileSync('public/images/vysker-znak-app-192.png') },
    } })
    expect(application.status()).toBe(201)
    const rental = await application.json()
    expect((await page.request.post(`/api/v1/admin/rentals/${rental.id}/approve`, { headers })).status()).toBe(200)
    setRentalInPast(rental.id)
    const invoice = JSON.stringify({ energy_kwh: '0', energy_cost: 0, extra_cost: 0, account_number: '19-2000145399', bank_code: '0800', account_iban: '', due_days: 7 })

    const limitedContext = await browser.newContext({ baseURL: new URL(page.url()).origin })
    try {
      const limitedPage = await limitedContext.newPage()
      await login(limitedPage, `bez-prava-${suffix}@vysker.test`, limitedPassword)
      await limitedPage.goto(`/admin/pronajmy/${rental.id}`)
      await expect(limitedPage.getByLabel('Faktura od účetní (nepovinné PDF)')).toHaveCount(0)
      const limitedSession = await (await limitedPage.request.get('/api/v1/admin/session')).json()
      const denied = await limitedPage.request.post(`/api/v1/admin/rentals/${rental.id}/invoice-with-attachment`, {
        headers: { 'X-CSRF-Token': limitedSession.csrf_token },
        multipart: { invoice, attachment: { name: 'faktura.pdf', mimeType: 'application/pdf', buffer: accountantPdf } },
      })
      expect(denied.status()).toBe(403)
      expect((await (await limitedPage.request.get(`/api/v1/admin/rentals/${rental.id}`)).json()).status).toBe('approved')
    } finally {
      await limitedContext.close()
    }

    await page.goto(`/admin/pronajmy/${rental.id}`)
    await expect(page.getByRole('heading', { name: 'Připravit vyúčtování' })).toBeVisible()
    await page.getByLabel('Číslo účtu včetně případného předčíslí').fill('19-2000145399')
    await page.getByLabel('Kód banky', { exact: true }).fill('0800')
    const file = page.getByLabel('Faktura od účetní (nepovinné PDF)')
    await expect(file).toHaveAttribute('accept', 'application/pdf')
    phase = 'invalid file'
    await file.setInputFiles({ name: 'faktura.pdf', mimeType: 'application/pdf', buffer: Buffer.from('Not a PDF') })
    const bad = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${rental.id}/invoice-with-attachment`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Odeslat vyúčtování' }).click()
    expect((await bad).status()).toBe(400)
    await expect(page.getByRole('alert')).toContainText(/PDF/i)
    expect({ pageErrors, browserErrors: pageErrors.length ? browserErrors : [] }).toEqual({ pageErrors: [], browserErrors: [] })
    expect((await (await page.request.get(`/api/v1/admin/rentals/${rental.id}`)).json()).status).toBe('approved')

    phase = 'oversized file'
    await file.setInputFiles({ name: 'prilis-velka.pdf', mimeType: 'application/pdf', buffer: Buffer.alloc(10 * 1024 * 1024 + 1) })
    await page.getByRole('button', { name: 'Odeslat vyúčtování' }).click()
    await expect(page.getByRole('alert')).toContainText(/10 MiB/)
    expect({ pageErrors, browserErrors: pageErrors.length ? browserErrors : [] }).toEqual({ pageErrors: [], browserErrors: [] })
    expect((await (await page.request.get(`/api/v1/admin/rentals/${rental.id}`)).json()).status).toBe('approved')

    phase = 'valid file and invoice submission'
    await file.setInputFiles({ name: 'faktura.pdf', mimeType: 'application/pdf', buffer: accountantPdf })
    const sent = page.waitForResponse(response => response.url().endsWith(`/api/v1/admin/rentals/${rental.id}/invoice-with-attachment`) && response.request().method() === 'POST')
    await page.getByRole('button', { name: 'Odeslat vyúčtování' }).click()
    const response = await sent
    expect(response.status()).toBe(200)
    expect(response.request().headers()['content-type']).toContain('multipart/form-data')
    await expect(page.getByRole('heading', { name: 'Vyúčtování', exact: true })).toBeVisible()
    expect({ pageErrors, browserErrors: pageErrors.length ? browserErrors : [] }).toEqual({ pageErrors: [], browserErrors: [] })
    expect((await (await page.request.get(`/api/v1/admin/rentals/${rental.id}`)).json()).status).toBe('invoiced')

    await expect.poll(() => smtp.messages.some(message => message.includes(`To: ${email}`) && message.includes('filename="faktura.pdf"')), { timeout: 70_000 }).toBe(true)
    const emailMessage = smtp.messages.find(message => message.includes(`To: ${email}`) && message.includes('filename="faktura.pdf"'))!
    const attachmentHeaders = emailMessage.indexOf('filename="faktura.pdf"')
    const attachmentStart = emailMessage.indexOf('\r\n\r\n', attachmentHeaders) + 4
    const attachmentEnd = emailMessage.indexOf('\r\n--', attachmentStart)
    expect(attachmentStart).toBeGreaterThan(3)
    expect(attachmentEnd).toBeGreaterThan(attachmentStart)
    expect(Buffer.from(emailMessage.slice(attachmentStart, attachmentEnd).replace(/\s/g, ''), 'base64')).toEqual(accountantPdf)
    await page.getByRole('button', { name: 'Obnovit stav odeslání' }).click()
    await expect(page.getByText('Předáno odesílacímu serveru', { exact: false })).toBeVisible()
    expect({ pageErrors, browserErrors: pageErrors.length ? browserErrors : [] }).toEqual({ pageErrors: [], browserErrors: [] })
  } finally {
    await smtp.close()
  }
})
