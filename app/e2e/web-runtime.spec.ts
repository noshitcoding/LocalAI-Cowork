import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

function collectRuntimeErrors(page: Page): string[] {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })
  return errors
}

async function mockPublicControlPlane(page: Page): Promise<void> {
  await page.route('**/api/v1/auth/oidc/config', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ schema_version: 2, enabled: false }),
    })
  })
  await page.route('**/api/v1/auth/browser/refresh', async (route) => {
    await route.fulfill({
      status: 401,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'unauthorized', message: 'no browser session', details: {} }),
    })
  })
}

async function expectWcagAa(page: Page): Promise<void> {
  const accessibility = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze()
  expect(accessibility.violations.map(({ id, impact, nodes }) => ({
    id,
    impact,
    targets: nodes.map((node) => node.target.join(' ')),
  }))).toEqual([])
}

test('web build loads only the remote control plane without Tauri initialization', async ({ page }) => {
  await mockPublicControlPlane(page)
  const errors = collectRuntimeErrors(page)
  await page.goto('/', { waitUntil: 'networkidle' })

  await expect(page).toHaveURL(/\/server$/)
  await expect(page.getByRole('heading', { name: 'Connect to Open Cowork Server' })).toBeVisible()
  await expect(page.locator('#boot-loader')).toHaveCount(0)
  await expectWcagAa(page)
  const loadedResources = await page.evaluate(() => performance.getEntriesByType('resource').map((entry) => entry.name))
  expect(loadedResources.filter((url) => /\/tauri-[^/]+\.js(?:\?|$)/.test(url))).toEqual([])
  expect(errors).toEqual([])
})

test('web OIDC callback route stays in the browser runtime', async ({ page }) => {
  await mockPublicControlPlane(page)
  const errors = collectRuntimeErrors(page)
  await page.goto('/auth/callback', { waitUntil: 'networkidle' })

  await expect(page.getByRole('heading', { name: /Completing single sign-on/ })).toBeVisible()
  await expect(page.locator('#boot-loader')).toHaveCount(0)
  await expectWcagAa(page)
  expect(errors).toEqual([])
})
