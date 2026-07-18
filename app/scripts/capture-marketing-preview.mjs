import { chromium } from '@playwright/test'
import { preview } from 'vite'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const outputPath = path.resolve(appRoot, '..', 'site', 'assets', 'app-preview.png')

const server = await preview({
  root: appRoot,
  preview: {
    host: '127.0.0.1',
    port: 4173,
    strictPort: true,
  },
})

let browser
try {
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({
    viewport: { width: 1920, height: 1080 },
    colorScheme: 'dark',
    locale: 'en-US',
    reducedMotion: 'reduce',
    deviceScaleFactor: 1,
  })
  await page.addInitScript(() => {
    window.localStorage.clear()
    window.sessionStorage.clear()
    window.localStorage.setItem('open-cowork.language', 'en')
    window.localStorage.setItem('open-cowork-ui', JSON.stringify({
      state: {
        activeMode: 'work',
        leftSidebarOpen: true,
        leftSidebarWidth: 320,
        theme: 'dark',
      },
      version: 0,
    }))
  })
  await page.goto('http://127.0.0.1:4173/', { waitUntil: 'domcontentloaded' })
  await page.locator('.cowork-pane').waitFor({ state: 'visible', timeout: 10_000 })
  await page.locator('#boot-loader').waitFor({ state: 'detached', timeout: 10_000 })
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
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
  })
  // Give Chromium one complete compositor cycle after lazy views and fonts settle.
  await page.waitForTimeout(1_000)
  await page.screenshot({ path: outputPath, fullPage: false })
  console.log(`Marketing preview written to ${outputPath}`)
} finally {
  await browser?.close()
  await new Promise((resolve, reject) => {
    server.httpServer.close((error) => error ? reject(error) : resolve())
  })
}
