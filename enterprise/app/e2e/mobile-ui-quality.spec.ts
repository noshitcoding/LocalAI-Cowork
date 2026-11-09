import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page, type Route } from '@playwright/test'

type MobileTheme = 'light' | 'dark'

const THEMES: MobileTheme[] = ['light', 'dark']
const VIEWPORTS = [
  { id: 'narrow', width: 390, height: 844 },
  { id: 'wide', width: 412, height: 915 },
] as const

const ids = {
  user: '11111111-1111-4111-8111-111111111111',
  session: '22222222-2222-4222-8222-222222222222',
  run: '33333333-3333-4333-8333-333333333333',
  thread: '44444444-4444-4444-8444-444444444444',
  project: '55555555-5555-4555-8555-555555555555',
  executor: '66666666-6666-4666-8666-666666666666',
  desktop: '77777777-7777-4777-8777-777777777777',
  approval: '88888888-8888-4888-8888-888888888888',
  terminal: '99999999-9999-4999-8999-999999999999',
} as const

const expiresAt = '2099-01-01T00:00:00.000Z'
const createdAt = '2026-08-25T10:00:00.000Z'
const run = {
  spec: {
    schema_version: 2,
    id: ids.run,
    thread_id: ids.thread,
    project_id: ids.project,
    project: { id: ids.project, revision: 1 },
    project_privacy: 'team_managed',
    task: null,
    creator_user_id: ids.user,
    executor_target: { kind: 'server_linux', pool_id: null },
    required_capabilities: ['terminal', 'desktop'],
    input: { prompt: 'Review the release candidate' },
    model_profile_id: null,
    snapshot_id: null,
    idempotency_key: 'mobile-design-qa',
    created_at: createdAt,
  },
  state: 'waiting_approval',
  revision: 2,
  etag: 'run-etag',
  assigned_executor_id: ids.executor,
  lease_expires_at: expiresAt,
  started_at: createdAt,
  finished_at: null,
  result: null,
  error: null,
  updated_at: createdAt,
} as const

function responseHeaders(contentType = 'application/json') {
  return {
    'access-control-allow-headers': 'authorization,content-type',
    'access-control-allow-methods': 'GET,POST,PUT,DELETE,OPTIONS',
    'access-control-allow-origin': '*',
    'content-type': contentType,
  }
}

async function fulfillJson(route: Route, body: unknown) {
  await route.fulfill({ status: 200, headers: responseHeaders(), body: JSON.stringify(body) })
}

async function installRuntimeFixture(page: Page) {
  await page.route('https://cowork.example.test/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (request.method() === 'OPTIONS') {
      await route.fulfill({ status: 204, headers: responseHeaders() })
      return
    }
    if (url.pathname.endsWith('/auth/oidc/config')) {
      await fulfillJson(route, { schema_version: 2, enabled: false })
      return
    }
    if (url.pathname.endsWith('/auth/native/authorize')) {
      await fulfillJson(route, { schema_version: 2, code: 'a'.repeat(40), expires_at: expiresAt })
      return
    }
    if (url.pathname.endsWith('/auth/native/token')) {
      await fulfillJson(route, {
        schema_version: 2,
        access_token: 'b'.repeat(40),
        access_expires_at: expiresAt,
        refresh_token: 'c'.repeat(40),
        refresh_expires_at: expiresAt,
        user_id: ids.user,
        session_id: ids.session,
      })
      return
    }
    if (url.pathname === '/api/v1/runs') {
      await fulfillJson(route, { items: [run], next_cursor: null })
      return
    }
    if (url.pathname.endsWith('/events')) {
      await route.fulfill({ status: 200, headers: responseHeaders('text/event-stream'), body: '' })
      return
    }
    if (url.pathname.endsWith('/desktop-sessions')) {
      await fulfillJson(route, [{
        schema_version: 2,
        id: ids.desktop,
        run_id: ids.run,
        executor_id: ids.executor,
        state: 'agent_controlled',
        stream_protocol: 'rfb.binary.v1',
        dimensions: { width: 1280, height: 720, scale_factor: 1 },
        controller_user_id: null,
        created_at: createdAt,
        ended_at: null,
      }])
      return
    }
    if (url.pathname.endsWith('/tickets')) {
      await fulfillJson(route, { schema_version: 2, token: 'd'.repeat(40), control: false, expires_at: expiresAt })
      return
    }
    if (url.pathname.endsWith('/terminal-sessions')) {
      await fulfillJson(route, {
        schema_version: 2,
        session_id: ids.terminal,
        token: 'e'.repeat(40),
        expires_at: expiresAt,
        protocol: 'terminal.binary.v1',
      })
      return
    }
    if (url.pathname.endsWith('/approvals')) {
      await fulfillJson(route, [{
        schema_version: 2,
        id: ids.approval,
        run_id: ids.run,
        revision: 1,
        etag: 'approval-etag',
        requested_action: { summary: 'Allow the release verification command' },
        state: 'pending',
        requested_at: createdAt,
        expires_at: expiresAt,
        resolved_by_user_id: null,
        resolved_at: null,
      }])
      return
    }
    if (url.pathname.endsWith('/input-requests') || url.pathname.endsWith('/artifacts') || url.pathname.includes('/messages')) {
      await fulfillJson(route, [])
      return
    }
    await fulfillJson(route, [])
  })
}

async function setTheme(page: Page, theme: MobileTheme) {
  await page.addInitScript((requestedTheme) => {
    window.localStorage.clear()
    window.sessionStorage.clear()
    window.localStorage.setItem('open-cowork.language', 'en')
    window.localStorage.setItem('open-cowork-ui', JSON.stringify({
      state: {
        activeMode: 'work',
        leftSidebarOpen: true,
        leftSidebarWidth: 260,
        theme: requestedTheme,
      },
      version: 0,
    }))
  }, theme)
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' })
}

async function waitForStableLayout(page: Page) {
  await expect(page.locator('#boot-loader')).toHaveCount(0)
  await page.addStyleTag({
    content: '*,*::before,*::after{animation:none!important;caret-color:transparent!important;transition:none!important}',
  })
  await page.evaluate(async () => {
    await document.fonts.ready
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
  })
}

async function expectAccessible(page: Page) {
  const result = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze()
  expect(result.violations.map(({ id, impact, nodes }) => ({
    id,
    impact,
    targets: nodes.map((node) => node.target.join(' ')),
  }))).toEqual([])
}

async function expectMobileContract(page: Page) {
  const dimensions = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    viewportWidth: window.innerWidth,
  }))
  expect(dimensions.documentWidth).toBeLessThanOrEqual(dimensions.viewportWidth + 1)

  const undersized = await page.locator('button, input:not([type="checkbox"]):not([type="radio"]), select, textarea').evaluateAll((elements) => (
    elements
      .filter((element) => {
        const rect = element.getBoundingClientRect()
        const style = getComputedStyle(element)
        return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden'
      })
      .map((element) => {
        const rect = element.getBoundingClientRect()
        return { element: element.outerHTML.slice(0, 120), height: rect.height }
      })
      .filter(({ height }) => height < 44)
  ))
  expect(undersized).toEqual([])
}

async function createPin(page: Page) {
  await page.getByLabel('App PIN').fill('123456')
  await page.getByLabel('Confirm PIN').fill('123456')
  await page.getByRole('button', { name: 'Create PIN' }).click()
  await expect(page.getByRole('heading', { name: 'Open Cowork' })).toBeVisible()
}

for (const theme of THEMES) {
  for (const viewport of VIEWPORTS) {
    test(`mobile lock and login are stable in ${theme} at ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await setTheme(page, theme)
      await page.setViewportSize({ width: viewport.width, height: viewport.height })
      await page.goto('/', { waitUntil: 'domcontentloaded' })
      await expect(page.getByRole('heading', { name: 'Protect Open Cowork' })).toBeVisible()
      await waitForStableLayout(page)
      await expectAccessible(page)
      await expectMobileContract(page)
      await expect(page).toHaveScreenshot(`mobile-lock-${theme}-${viewport.id}.png`)

      await createPin(page)
      await waitForStableLayout(page)
      await expectAccessible(page)
      await expectMobileContract(page)
      await expect(page).toHaveScreenshot(`mobile-login-${theme}-${viewport.id}.png`)
    })
  }
}

test('mobile run, intervention, remote, terminal, offline, and outbox surfaces stay available', async ({ context, page }) => {
  await setTheme(page, 'light')
  await installRuntimeFixture(page)
  await page.setViewportSize({ width: 412, height: 915 })
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await createPin(page)

  await page.getByLabel('Server URL').fill('https://cowork.example.test')
  await page.getByLabel('Email').fill('qa@example.test')
  await page.getByLabel('Password').fill('correct horse battery staple')
  await page.getByRole('button', { name: 'Connect' }).click()
  await expect(page.getByText('Review the release candidate')).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Action required' })).toBeVisible()
  const remoteDesktop = page.getByRole('region', { name: 'Remote desktop', exact: true })
  await expect(remoteDesktop).toBeVisible()
  await waitForStableLayout(page)
  await expectAccessible(page)
  await expect(page).toHaveScreenshot('mobile-run-online-light-wide.png')
  await expect(remoteDesktop).toHaveScreenshot('mobile-remote-light-wide.png')

  await page.getByRole('button', { name: 'Terminal' }).click()
  const terminal = page.getByLabel('Server terminal')
  await expect(terminal).toBeVisible()
  await terminal.scrollIntoViewIfNeeded()
  await waitForStableLayout(page)
  await expect(terminal).toHaveScreenshot('mobile-terminal-light-wide.png')

  await page.getByRole('button', { name: 'Back to runs' }).click()
  await expect(page.getByRole('heading', { name: 'Runs' })).toBeVisible()
  await waitForStableLayout(page)
  await expect(page).toHaveScreenshot('mobile-run-list-light-wide.png')

  await context.setOffline(true)
  await expect(page.getByText('Offline cache')).toBeVisible()
  await page.getByRole('button', { name: /Review the release candidate/ }).click()
  await expect(page.getByText(/Offline cached run/)).toBeVisible()
  await page.getByLabel('Offline reply').fill('Continue after connectivity returns')
  await page.getByRole('button', { name: 'Add to outbox' }).click()
  await page.getByRole('button', { name: 'Back to runs' }).click()
  await expect(page.getByText('Encrypted outbox')).toBeVisible()
  await waitForStableLayout(page)
  await expectAccessible(page)
  await expectMobileContract(page)
  await expect(page).toHaveScreenshot('mobile-offline-outbox-light-wide.png')
})
