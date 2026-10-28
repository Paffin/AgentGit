import { existsSync } from 'node:fs'
import { defineConfig, devices } from '@playwright/test'

// The end-to-end suite runs the built UI against a REAL ag-server (see e2e/support/server.ts and
// e2e/global-setup.ts). Nothing is mocked. `npm run test:e2e` builds the UI first.
//
// Where the UI comes from: by default the freshly built `dist/` through `[ui] dir` (so a UI change needs no
// Rust rebuild). `AG_E2E_UI=embedded` runs against the UI compiled into the ag-server binary instead, the
// production path; CI does that. Binary: AG_SERVER_BIN (see e2e/support/server.ts).
//
// Browsers: the whole suite runs in Chromium, Firefox and WebKit (projects of the same name). Locally only a
// Chromium is usually installed: AG_E2E_CHROMIUM names a binary, else /opt/pw-browsers/chromium when present,
// else Playwright's own (`npx playwright install chromium`). CI uses the matching official Playwright
// container with all three browsers. Locally install them with `npx playwright install --with-deps`.
// Run one with `npx playwright test --project=firefox`
// (add `--no-deps` to skip the screenshots project, which needs Chromium).
//
// Responsive: the `tablet` (768) and `phone` (390, touch) projects run the layout and accessibility specs at
// those sizes, in Chromium.
const preinstalled = '/opt/pw-browsers/chromium'
const chromiumPath =
  process.env.AG_E2E_CHROMIUM || (existsSync(preinstalled) ? preinstalled : undefined)
const chromium = chromiumPath ? { launchOptions: { executablePath: chromiumPath } } : {}

const screenshots = /screenshots\.spec\.ts/
// Layout assertions are made at the viewports of the tablet and phone projects, not at desktop size.
const layout = /responsive\.spec\.ts/
const small =
  /(responsive|accessibility|code-workflows|code-search|task-details|review-workbench|draft-recovery|review-drafts|multi-file-edit|execution-control|run-details|task-assignment|executor-presence)\.spec\.ts/
const desktop = { viewport: { width: 1440, height: 900 } }

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
  reporter: process.env.CI
    ? [['list'], ['html', { open: 'never' }], ['json', { outputFile: 'test-results/results.json' }]]
    : [['list'], ['html', { open: 'never' }]],
  use: {
    ...desktop,
    locale: 'en-US',
    timezoneId: 'UTC',
    colorScheme: 'light',
    contextOptions: { reducedMotion: 'reduce' },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    // The documentation screenshots come first, from the pristine demo world: later tests add events.
    {
      name: 'screens',
      testMatch: screenshots,
      use: { ...devices['Desktop Chrome'], ...desktop, ...chromium },
    },
    {
      name: 'chromium',
      testIgnore: [screenshots, layout],
      dependencies: ['screens'],
      use: { ...devices['Desktop Chrome'], ...desktop, ...chromium },
    },
    {
      name: 'firefox',
      testIgnore: [screenshots, layout],
      use: { ...devices['Desktop Firefox'], ...desktop },
    },
    {
      name: 'webkit',
      testIgnore: [screenshots, layout],
      use: { ...devices['Desktop Safari'], ...desktop },
    },
    {
      name: 'tablet',
      testMatch: small,
      dependencies: ['screens'],
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 768, height: 1024 },
        hasTouch: true,
        ...chromium,
      },
    },
    {
      name: 'phone',
      testMatch: small,
      dependencies: ['screens'],
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 2,
        ...chromium,
      },
    },
  ],
})
