import { test as base } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

// The suite shares one disposable database and runs with a single worker.
// Reset counters between tests, keeping throttling active within each scenario.
export const test = base.extend<{ isolatedRateLimits: void }>({
  isolatedRateLimits: [async ({}, use) => {
    const { database } = JSON.parse(readFileSync('target/e2e-fixture.json', 'utf8'))
    execFileSync('python3', ['-c', [
      'import psycopg, sys',
      'with psycopg.connect(sys.argv[1]) as connection:',
      '    name = connection.execute("SELECT current_database()").fetchone()[0]',
      '    if not name.startswith("vysker_test_"):',
      '        raise RuntimeError("Expected a disposable browser-test database")',
      '    connection.execute("DELETE FROM rate_limits")',
    ].join('\n'), database])
    await use()
  }, { auto: true }],
})

export { expect, type Page } from '@playwright/test'
