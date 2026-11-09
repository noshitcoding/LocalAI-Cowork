import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './e2e',
  testMatch: ['mobile-ui-quality.spec.ts'],
  outputDir: './test-results/mobile-ui',
  fullyParallel: false,
  workers: 1,
  timeout: 45_000,
  expect: {
    timeout: 10_000,
    toHaveScreenshot: {
      animations: 'disabled',
      caret: 'hide',
      maxDiffPixelRatio: 0.002,
      threshold: 0.2,
    },
  },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-mobile' }]],
  use: {
    baseURL: 'http://127.0.0.1:4188',
    browserName: 'chromium',
    colorScheme: 'light',
    locale: 'en-US',
    reducedMotion: 'reduce',
    deviceScaleFactor: 1,
    hasTouch: true,
    isMobile: true,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'npm run build:mobile:preview && npm run preview:mobile:e2e',
    url: 'http://127.0.0.1:4188',
    reuseExistingServer: false,
    timeout: 90_000,
  },
})
