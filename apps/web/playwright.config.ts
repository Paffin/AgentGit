import { existsSync } from 'node:fs'
import { defineConfig } from '@playwright/test'

// The end-to-end suite runs the built UI against a REAL ag-server (see e2e/support/server.ts and
// e2e/global-setup.ts). Nothing is mocked. `npm run test:e2e` builds the UI first.
//
// Browser: set AG_E2E_CHROMIUM to a Chromium binary; otherwise a pre-installed one at
// /opt/pw-browsers/chromium is used when present; otherwise Playwright's own (`npx playwright install chromium`).
const preinstalled = '/opt/pw-browsers/chromium'
const executablePath =
  process.env.AG_E2E_CHROMIUM || (existsSync(preinstalled) ? preinstalled : undefined)

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  globalSetup: './e2e/global-setup.ts',
  outputDir: 'test-results',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : 3,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    viewport: { width: 1440, height: 900 },
    locale: 'en-US',
    timezoneId: 'UTC',
    colorScheme: 'light',
    reducedMotion: 'reduce',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: executablePath ? { executablePath } : {},
  },
  projects: [
    // The documentation screenshots come first, from the pristine demo world: later tests add events.
    { name: 'screens', testMatch: /screenshots\.spec\.ts/ },
    { name: 'main', testIgnore: /screenshots\.spec\.ts/, dependencies: ['screens'] },
  ],
})
