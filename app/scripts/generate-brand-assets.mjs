import { chromium } from '@playwright/test'
import { spawnSync } from 'node:child_process'
import { copyFile, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = path.resolve(appRoot, '..')
const masterPath = path.join(repoRoot, 'brand', 'open-workframe.svg')
const iconDir = path.join(appRoot, 'src-tauri', 'icons')
const webOnly = process.argv.includes('--web-only')

if (!webOnly) {
  const tauriCliPath = path.join(appRoot, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
  const tauri = spawnSync(process.execPath, [tauriCliPath, 'icon', masterPath], {
    cwd: appRoot,
    stdio: 'inherit',
  })

  if (tauri.status !== 0) {
    throw new Error(`Tauri icon generation failed with exit code ${tauri.status ?? 'unknown'}`)
  }
}

const masterSvg = await readFile(masterPath, 'utf8')
const faviconSvg = masterSvg
  .replace('width="1024"', 'width="64"')
  .replace('height="1024"', 'height="64"')

await writeFile(path.join(appRoot, 'public', 'favicon.svg'), faviconSvg, 'utf8')
const socialSvg = await readFile(path.join(repoRoot, 'brand', 'github-social-preview.svg'), 'utf8')
const browser = await chromium.launch({ headless: true })
try {
  const faviconPath = path.join(appRoot, 'public', 'favicon.png')
  await copyFile(path.join(iconDir, '128x128.png'), faviconPath)
  await copyFile(faviconPath, path.join(repoRoot, 'site', 'assets', 'logo.png'))

  const socialPage = await browser.newPage({ viewport: { width: 1280, height: 640 }, deviceScaleFactor: 1 })
  await socialPage.setContent(`<style>html,body{margin:0;width:1280px;height:640px;overflow:hidden}</style>${socialSvg}`)
  await socialPage.screenshot({
    path: path.join(repoRoot, 'site', 'assets', 'github-social-preview.png'),
    fullPage: false,
  })
} finally {
  await browser.close()
}

console.log(
  webOnly
    ? 'Generated Carbon Signal web and social assets from the master SVG.'
    : 'Generated Carbon Signal web, social, and Tauri icon assets from the master SVG.',
)
