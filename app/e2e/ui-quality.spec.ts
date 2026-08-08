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
  { id: 'crew', path: '/crew', ready: '.crew-shell' },
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
        leftSidebarWidth: 260,
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

test('chat dropdowns open upward and stay inside fullscreen dimensions', async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1080 })

  for (const theme of THEMES) {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' })
    await openStableSurface(page, { ...PRODUCT_SURFACES[0], path: withTheme('/', theme) })

    const controls = page.locator('.chat-input-toolbar-compact .chat-dropdown-toggle')
    await expect(controls).toHaveCount(3)

    const readability = await controls.evaluateAll((toggles) => {
      const rgb = (value: string) => (value.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number)
      const luminance = (value: string) => {
        const channels = rgb(value).map((channel) => {
          const normalized = channel / 255
          return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4
        })
        return (0.2126 * channels[0]) + (0.7152 * channels[1]) + (0.0722 * channels[2])
      }
      const contrast = (foreground: string, background: string) => {
        const lighter = Math.max(luminance(foreground), luminance(background))
        const darker = Math.min(luminance(foreground), luminance(background))
        return (lighter + 0.05) / (darker + 0.05)
      }

      return toggles.map((toggle) => {
        const style = getComputedStyle(toggle)
        return {
          height: toggle.getBoundingClientRect().height,
          fontSize: Number.parseFloat(style.fontSize),
          contrast: contrast(style.color, style.backgroundColor),
        }
      })
    })

    for (const control of readability) {
      expect(control.height).toBeGreaterThanOrEqual(36)
      expect(control.fontSize).toBeGreaterThanOrEqual(12)
      expect(control.contrast).toBeGreaterThanOrEqual(4.5)
    }

    const modelToggle = page.getByRole('combobox', { name: 'Model' })
    await modelToggle.click()
    const modelList = page.getByRole('listbox', { name: 'Model' })
    await expect(modelList).toBeVisible()
    await expect(modelList.getByRole('option').first()).toBeVisible()

    const dropdownAccessibility = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze()
    expect(formatViolations(dropdownAccessibility.violations)).toEqual([])

    const geometry = await modelList.evaluate((listbox) => {
      const toggle = listbox.parentElement!.querySelector<HTMLElement>('.chat-dropdown-toggle')!
      const listboxBox = listbox.getBoundingClientRect()
      const toggleBox = toggle.getBoundingClientRect()
      return {
        listboxTop: listboxBox.top,
        listboxBottom: listboxBox.bottom,
        toggleTop: toggleBox.top,
      }
    })

    expect(geometry.listboxTop).toBeGreaterThanOrEqual(0)
    expect(geometry.listboxBottom).toBeLessThan(geometry.toggleTop)
  }
})

test('minimal shell keeps onboarding and context inside the two drawers', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 650 })
  await openStableSurface(page, PRODUCT_SURFACES[0])

  await expect(page.getByRole('heading', { name: 'Set up LocalAI Cowork' })).toHaveCount(0)
  await expect(page.getByText(/Getting started is available in the main menu/)).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Message input' })).toBeVisible()
  await expect(page.getByRole('complementary', { name: 'Run context' })).toHaveCount(0)

  await page.getByRole('button', { name: 'Open main menu' }).click()
  const menu = page.getByRole('dialog', { name: 'Main menu' })
  await expect(menu).toBeVisible()
  await expect(menu.getByRole('searchbox', { name: 'Search areas and commands' })).toBeFocused()
  await menu.getByText('Getting started', { exact: true }).click()
  await expect(menu.getByText('Choose a model in the chat controls.')).toBeVisible()

  await menu.getByRole('button', { name: /Context & status/ }).click()
  await expect(menu).toHaveCount(0)
  await expect(page.getByRole('complementary', { name: 'Run context' })).toBeVisible()
  await page.getByRole('button', { name: 'Close run context' }).click()
  await expect(page.getByRole('complementary', { name: 'Run context' })).toHaveCount(0)
})

test('burger navigation has deterministic focus and closes on route changes', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 650 })
  await openStableSurface(page, PRODUCT_SURFACES[0])

  await page.keyboard.press('Control+K')
  const menu = page.getByRole('dialog', { name: 'Main menu' })
  const search = menu.getByRole('searchbox', { name: 'Search areas and commands' })
  await expect(search).toBeFocused()
  await page.keyboard.press('Shift+Tab')
  await expect(menu.getByRole('button', { name: 'Close menu' })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(search).toBeFocused()

  const menuAccessibility = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze()
  expect(formatViolations(menuAccessibility.violations)).toEqual([])
  await expect(page).toHaveScreenshot('burger-menu-light-compact.png', { fullPage: false })

  await page.keyboard.press('Escape')
  await expect(menu).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Open main menu' })).toBeFocused()

  await page.getByRole('button', { name: 'Open main menu' }).click()
  await menu.getByRole('button', { name: /^Settings/ }).click()
  await expect(page).toHaveURL(/\/settings$/)
  await expect(page.getByRole('heading', { name: 'AI & model' })).toBeVisible()
  await expect(menu).toHaveCount(0)

  const settingsContent = page.locator('.settings-content')
  const settingsScrollRange = await settingsContent.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(settingsScrollRange.scrollHeight).toBeGreaterThan(settingsScrollRange.clientHeight)
  await settingsContent.hover()
  await page.mouse.wheel(0, 600)
  await expect.poll(() => settingsContent.evaluate((element) => element.scrollTop)).toBeGreaterThan(0)

  await page.getByRole('button', { name: 'Open main menu' }).click()
  const settingsSections = menu.getByLabel('Settings Sections')
  await expect(settingsSections.getByRole('button')).toHaveCount(9)
  await settingsSections.getByRole('button', { name: 'Interface' }).click()
  await expect(page).toHaveURL(/\/settings\?section=ui$/)
  await expect(page.getByRole('heading', { name: 'Interface' })).toBeVisible()
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
  await page.getByRole('button', { name: 'Open main menu' }).click()
  await page.getByRole('dialog', { name: 'Main menu' }).getByRole('button', { name: /Context & status/ }).click()
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
