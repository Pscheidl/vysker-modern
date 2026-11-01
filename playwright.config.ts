import { defineConfig, devices } from '@playwright/test'

const baseURL = `http://127.0.0.1:${process.env.E2E_PORT || '3107'}`

export default defineConfig({
  testDir: './tests/browser',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'target/playwright-report' }]],
  outputDir: 'target/playwright-results',
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'python3 scripts/e2e-server.py',
    url: `${baseURL}/api/v1/ready`,
    reuseExistingServer: false,
    timeout: 60_000,
    gracefulShutdown: { signal: 'SIGTERM', timeout: 30_000 },
  },
})
