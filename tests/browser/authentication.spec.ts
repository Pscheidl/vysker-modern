import { test, expect } from './fixtures'

test('login throttling remains active within an isolated scenario', async ({ request }) => {
  const login = () => request.post('/api/v1/admin/login', {
    data: { email: 'e2e-admin@vysker.test', password: 'incorrect-password' },
  })
  for (let attempt = 0; attempt < 10; attempt++) {
    expect((await login()).status()).toBe(401)
  }
  expect((await login()).status()).toBe(429)
})
