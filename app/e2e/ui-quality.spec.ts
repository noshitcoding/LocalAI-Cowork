import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

type ProductSurface = {
  id: string
  path: string
  ready: string
}

type ProductTheme = 'light' | 'dark'

const PRODUCT_SURFACES: ProductSurface[] = [
  { id: 'cowork', path: '/', ready: '.cowork-pane' },
  { id: 'tasks', path: '/tasks', ready: '[data-doc-id="view:/tasks"]' },
  { id: 'crew', path: '/crew', ready: '.crew-shell-top' },
  { id: 'projects', path: '/projects', ready: '.project-view' },
  { id: 'features', path: '/features', ready: '.feature-workbench' },
  { id: 'settings', path: '/settings', ready: '.settings-layout' },
]

const VIEWPORTS = [
  { id: 'compact', width: 900, height: 650 },
  { id: 'wide', width: 1920, height: 1080 },
] as const

const THEMES: ProductTheme[] = ['light', 'dark']

function withTheme(path: string, theme: ProductTheme) {
  const url = new URL(path, 'http://127.0.0.1:4173')
  url.searchParams.set('e2e-theme', theme)
  return `${url.pathname}${url.search}`
}

async function openStableSurface(page: Page, surface: ProductSurface) {
  const runtimeErrors: string[] = []
  page.on('pageerror', (error) => runtimeErrors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error') runtimeErrors.push(message.text())
  })
  page.on('requestfailed', (request) => runtimeErrors.push(`${request.url()}: ${request.failure()?.errorText ?? 'request failed'}`))
  await page.goto(surface.path, { waitUntil: 'domcontentloaded' })
  try {
    await page.locator(surface.ready).waitFor({ state: 'visible', timeout: 10_000 })
  } catch (error) {
    throw new Error(`Surface ${surface.id} did not become ready. Runtime errors: ${runtimeErrors.join(' | ') || 'none'}.`, { cause: error })
  }
  await expect(page.locator('#boot-loader')).toHaveCount(0)
  await page.addStyleTag({
    content: `
      *, *::before, *::after {
        animation-delay: 0s !important;
        animation-duration: 0s !important;
        caret-color: transparent !important;
        transition-delay: 0s !important;
        transition-duration: 0s !important;
      }
    `,
  })
  await page.evaluate(async () => {
    await document.fonts.ready
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
  })
}

function formatViolations(violations: Awaited<ReturnType<AxeBuilder['analyze']>>['violations']) {
  return violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    help: violation.help,
    nodes: violation.nodes.map((node) => ({
      target: node.target.join(' '),
      summary: node.failureSummary,
    })),
  }))
}

test.beforeEach(async ({ context, page }) => {
  await context.addInitScript(() => {
    if (!window.location.search.includes('preserve-e2e-state')) {
      window.localStorage.clear()
      window.sessionStorage.clear()
    }
    window.localStorage.setItem('open-cowork.language', 'en')
    const requestedTheme = new URLSearchParams(window.location.search).get('e2e-theme')
    const theme = requestedTheme === 'dark' ? 'dark' : 'light'
    window.localStorage.setItem('open-cowork-ui', JSON.stringify({
      state: {
        activeMode: 'work',
        leftSidebarOpen: true,
        leftSidebarWidth: 320,
        theme,
      },
      version: 0,
    }))
  })
  await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' })
})

for (const theme of THEMES) {
  for (const viewport of VIEWPORTS) {
    for (const surface of PRODUCT_SURFACES) {
      test(`${surface.id} is accessible and visually stable in ${theme} mode at ${viewport.width}x${viewport.height}`, async ({ page }) => {
        await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' })
        await page.setViewportSize({ width: viewport.width, height: viewport.height })
        await openStableSurface(page, { ...surface, path: withTheme(surface.path, theme) })

        const dimensions = await page.evaluate(() => ({
          documentWidth: document.documentElement.scrollWidth,
          viewportWidth: window.innerWidth,
        }))
        expect(dimensions.documentWidth, 'The app shell must not create horizontal page overflow.').toBeLessThanOrEqual(dimensions.viewportWidth + 1)

        const accessibility = await new AxeBuilder({ page })
          .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
          .analyze()
        expect(formatViolations(accessibility.violations)).toEqual([])

        await expect(page).toHaveScreenshot(`${surface.id}-${theme}-${viewport.id}.png`, {
          fullPage: false,
        })
      })
    }
  }
}

test('guided onboarding stays discoverable and prepares a safe first task', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 650 })
  await openStableSurface(page, PRODUCT_SURFACES[0])

  await expect(page.getByRole('heading', { name: 'Set up LocalAI Cowork' })).toBeVisible()
  await page.getByRole('button', { name: 'Run context' }).click()
  await expect(page.getByRole('complementary', { name: 'Run context' })).toBeVisible()
  await page.getByRole('button', { name: 'Close run context' }).click()
  await expect(page.getByRole('complementary', { name: 'Run context' })).toBeHidden()
  await page.getByRole('button', { name: 'Dismiss onboarding' }).click()
  await page.getByRole('button', { name: 'Open getting started' }).click()
  await page.evaluate(() => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {
        invoke: async (command: string) => (
          command === 'plugin:dialog|open' ? ['C:\\workspace'] : null
        ),
      },
    })
  })
  await page.getByRole('button', { name: 'Context', exact: true }).click()
  await page.getByRole('button', { name: 'Choose a working folder' }).click()
  await expect(page.getByRole('button', { name: 'Choose another folder' })).toBeVisible()
  await page.getByRole('button', { name: 'Control' }).click()
  await page.getByRole('button', { name: 'Use starter task' }).click()

  await expect(page.getByRole('textbox', { name: 'Message input' })).toHaveValue(/create a concise project brief/i)
  await expect(page.getByRole('button', { name: 'Open getting started' })).toBeVisible()
})

test('run context renders persisted events and artifacts', async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.evaluate(() => {
    window.localStorage.setItem('engine-store', JSON.stringify({
      state: {
        activeProvider: 'ollama',
        currentRunId: 'run-visual-evidence',
        currentSessionId: 'session-visual-evidence',
      },
      version: 0,
    }))
  })
  await page.addInitScript(() => {
    if (!window.location.search.includes('preserve-e2e-state')) return
    let callbackId = 0
    const callbacks = new Map<number, (payload: unknown) => void>()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {
        metadata: {
          currentWindow: { label: 'main' },
          currentWebview: { windowLabel: 'main', label: 'main' },
        },
        transformCallback: (callback: (payload: unknown) => void) => {
          callbackId += 1
          callbacks.set(callbackId, callback)
          return callbackId
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        runCallback: (id: number, payload: unknown) => callbacks.get(id)?.(payload),
        invoke: async (command: string, args?: Record<string, unknown>) => {
          if (command === 'plugin:window|outer_size' || command === 'plugin:window|inner_size') return { width: 1920, height: 1080 }
          if (command === 'plugin:window|outer_position' || command === 'plugin:window|inner_position') return { x: 0, y: 0 }
          if (command.startsWith('plugin:window|is_')) return false
          if (command === 'plugin:event|listen') return args?.handler ?? 1
          if (command.startsWith('plugin:event|') || command.startsWith('plugin:window|')) return null
          if (command === 'credential_get') return { value: null }
          if (command === 'engine_run_event_list') {
            return [
              { id: 'event-2', run_id: 'run-visual-evidence', sequence: 2, event_type: 'artifact_written', summary: 'Wrote release report', created_at: '2026-07-12T20:01:00Z' },
              { id: 'event-1', run_id: 'run-visual-evidence', sequence: 1, event_type: 'tool_completed', summary: 'Workspace inspection completed', created_at: '2026-07-12T20:00:00Z' },
            ]
          }
          if (command === 'engine_run_artifact_list') {
            return [{ id: 'artifact-1', run_id: 'run-visual-evidence', kind: 'pdf', path: 'C:/workspace/release-report.pdf', title: 'Release report', summary: 'Architecture, risks, and prioritized next steps', created_at: '2026-07-12T20:01:00Z' }]
          }
          if (command === 'office_open_document') {
            const request = args?.request as { path?: string } | undefined
            const runtimeWindow = window as typeof window & { __openedArtifactPath?: string }
            runtimeWindow.__openedArtifactPath = request?.path
            return { launched: true }
          }
          if (command.includes('list')) return []
          return null
        },
      },
    })
  })

  await openStableSurface(page, { ...PRODUCT_SURFACES[0], path: '/?preserve-e2e-state=1' })
  await expect(page.getByText('Wrote release report')).toBeVisible()
  await expect(page.getByText('Release report', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Open output: Release report' }).click()
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __openedArtifactPath?: string }
  ).__openedArtifactPath)).toBe('C:/workspace/release-report.pdf')

  const accessibility = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze()
  expect(formatViolations(accessibility.violations)).toEqual([])
  await expect(page).toHaveScreenshot('cowork-run-context-populated.png', { fullPage: false })
})
