import AxeBuilder from '@axe-core/playwright'
import { chromium } from '@playwright/test'
import { preview } from 'vite'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const themes = {
  light: {
    '--bg-primary': '#f4f4f1',
    '--bg-white': '#fcfbf7',
    '--text-primary': '#161615',
    '--text-secondary': '#535350',
    '--text-muted': '#6b6b66',
    '--border-color': '#cbcbc4',
    '--accent-text': '#6f5500',
    '--accent-fill': '#ffd84d',
    '--accent-soft': '#fff1b2',
    '--accent-foreground': '#171300',
    '--focus-ring': '#8a6700',
  },
  dark: {
    '--bg-primary': '#0a0a0b',
    '--bg-white': '#18181b',
    '--text-primary': '#f5f5f2',
    '--text-secondary': '#b8b8b3',
    '--text-muted': '#90908b',
    '--border-color': '#303034',
    '--accent-text': '#ffe26a',
    '--accent-fill': '#ffd84d',
    '--accent-soft': ['rgba(255, 216, 77, 0.14)', '#ffd84d24'],
    '--accent-foreground': '#171300',
    '--focus-ring': '#ffe26a',
  },
}

const server = await preview({
  root: appRoot,
  preview: { host: '127.0.0.1', port: 4181, strictPort: true },
})

let browser
try {
  browser = await chromium.launch({ headless: true })
  for (const [theme, expectedTokens] of Object.entries(themes)) {
    const context = await browser.newContext({
      viewport: { width: 1920, height: 1080 },
      colorScheme: theme,
      locale: 'en-US',
      reducedMotion: 'reduce',
      deviceScaleFactor: 1,
    })
    const page = await context.newPage()
    await page.addInitScript((selectedTheme) => {
      window.localStorage.clear()
      window.sessionStorage.clear()
      window.localStorage.setItem('open-cowork.language', 'en')
      window.localStorage.setItem('open-cowork-ui', JSON.stringify({
        state: {
          activeMode: 'work',
          leftSidebarOpen: true,
          leftSidebarWidth: 320,
          theme: selectedTheme,
        },
        version: 0,
      }))
    }, theme)
    await page.goto(`http://127.0.0.1:4181/?e2e-theme=${theme}`, { waitUntil: 'domcontentloaded' })
    await page.locator('.cowork-pane').waitFor({ state: 'visible', timeout: 15_000 })
    await page.locator('#boot-loader').waitFor({ state: 'detached', timeout: 15_000 })

    const actualTokens = await page.evaluate((names) => {
      const styles = getComputedStyle(document.body)
      return Object.fromEntries(names.map((name) => [name, styles.getPropertyValue(name).trim().toLowerCase()]))
    }, Object.keys(expectedTokens))
    const tokenMismatches = Object.entries(expectedTokens)
      .filter(([name, expected]) => (
        Array.isArray(expected) ? !expected.includes(actualTokens[name]) : actualTokens[name] !== expected
      ))
      .map(([name, expected]) => `${name}: expected ${[].concat(expected).join(' or ')}, received ${actualTokens[name] || '<empty>'}`)
    if (tokenMismatches.length > 0) {
      throw new Error(`${theme} Carbon Signal token mismatch:\n${tokenMismatches.join('\n')}`)
    }

    const accessibility = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze()
    if (accessibility.violations.length > 0) {
      const summary = accessibility.violations.flatMap((violation) => (
        violation.nodes.map((node) => `${violation.id}: ${node.target.join(' ')}`)
      ))
      throw new Error(`${theme} accessibility violations:\n${summary.join('\n')}`)
    }

    await context.close()
    console.log(`${theme} Carbon Signal tokens and WCAG checks passed.`)
  }
} finally {
  await browser?.close()
  await new Promise((resolve, reject) => {
    server.httpServer.close((error) => error ? reject(error) : resolve())
  })
}
