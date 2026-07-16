import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gzipSync } from 'node:zlib'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const distDir = join(root, 'dist')
const assetsDir = join(distDir, 'assets')
const indexHtmlPath = join(distDir, 'index.html')

// Current-app baseline after route lazy-loading, the optional terminal bundle,
// and the settings provider dashboard. Keep the CSS allowance deliberately tight.
// Keep headroom tight so the check still catches accidental bundle growth.
const budgets = {
  initialGzipBytes: 300 * 1024,
  cssGzipBytes: 29 * 1024,
  totalJsGzipBytes: 420 * 1024,
  largestJsChunkGzipBytes: 120 * 1024,
}

function formatBytes(bytes) {
  return `${(bytes / 1024).toFixed(1)} KiB`
}

function gzipSize(filePath) {
  return gzipSync(readFileSync(filePath)).byteLength
}

function fail(message) {
  console.error(message)
  process.exitCode = 1
}

if (!existsSync(indexHtmlPath) || !existsSync(assetsDir)) {
  fail('Build output missing. Run the Vite build before checking budgets.')
  process.exit()
}

const assets = readdirSync(assetsDir)
  .map((name) => {
    const filePath = join(assetsDir, name)
    return {
      name,
      filePath,
      rawBytes: statSync(filePath).size,
      gzipBytes: gzipSize(filePath),
    }
  })
  .filter((asset) => asset.name.endsWith('.js') || asset.name.endsWith('.css'))

const indexHtml = readFileSync(indexHtmlPath, 'utf8')
const initialAssetNames = Array.from(indexHtml.matchAll(/\/assets\/([^"']+\.(?:js|css))/g), (match) => match[1])
const initialAssets = assets.filter((asset) => initialAssetNames.includes(asset.name))
const cssAssets = assets.filter((asset) => asset.name.endsWith('.css'))
const jsAssets = assets.filter((asset) => asset.name.endsWith('.js'))

const initialGzipBytes = initialAssets.reduce((sum, asset) => sum + asset.gzipBytes, 0)
const cssGzipBytes = cssAssets.reduce((sum, asset) => sum + asset.gzipBytes, 0)
const totalJsGzipBytes = jsAssets.reduce((sum, asset) => sum + asset.gzipBytes, 0)
const largestJsChunk = jsAssets.reduce((largest, asset) => (
  asset.gzipBytes > largest.gzipBytes ? asset : largest
), { name: 'none', gzipBytes: 0 })

const results = [
  ['Initial gzip', initialGzipBytes, budgets.initialGzipBytes],
  ['CSS gzip', cssGzipBytes, budgets.cssGzipBytes],
  ['Total JS gzip', totalJsGzipBytes, budgets.totalJsGzipBytes],
  [`Largest JS chunk (${largestJsChunk.name})`, largestJsChunk.gzipBytes, budgets.largestJsChunkGzipBytes],
]

for (const [name, actual, limit] of results) {
  const ok = actual <= limit
  console.log(`${ok ? '[OK]' : '[FAIL]'} ${name}: ${formatBytes(actual)} / ${formatBytes(limit)}`)
  if (!ok) {
    fail(`${name} exceeds budget.`)
  }
}

if (process.exitCode) {
  process.exit(process.exitCode)
}

console.log('Build budgets completed.')
