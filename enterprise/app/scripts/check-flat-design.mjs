import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { dirname, extname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const sourceRoot = join(appRoot, 'src')
const fix = process.argv.includes('--fix')
const sourceExtensions = new Set(['.css', '.ts', '.tsx'])
const violations = []

function walk(directory) {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

function normalizeRadiusValue(value) {
  const normalized = value
    .replaceAll('var(--radius-sm, 6px)', 'var(--radius-control)')
    .replaceAll('var(--radius-sm)', 'var(--radius-control)')
    .replaceAll('var(--radius-md)', 'var(--radius-surface)')
    .replaceAll('var(--radius-lg)', 'var(--radius-surface)')

  if (/^(?:0|0px|inherit|initial|unset)$/i.test(normalized.trim())) return normalized.trim()
  if (/^var\(--radius-(?:surface|control|label|round)\)$/i.test(normalized.trim())) return normalized.trim()
  if (/calc\(/i.test(normalized)) return 'var(--radius-surface)'

  return normalized
    .replace(/(?<![-\w.])(?:999|[1-9]\d*(?:\.\d+)?)px\b/gi, 'var(--radius-surface)')
    .replace(/(?<![-\w.])(?:100|[1-9]\d?(?:\.\d+)?)%\b/gi, 'var(--radius-surface)')
    .replace(/\s+/g, ' ')
    .trim()
}

function normalizeCss(source) {
  return source
    .replaceAll('var(--radius-sm, 6px)', 'var(--radius-control)')
    .replaceAll('var(--radius-sm)', 'var(--radius-control)')
    .replaceAll('var(--radius-md)', 'var(--radius-surface)')
    .replaceAll('var(--radius-lg)', 'var(--radius-surface)')
    .replace(/box-shadow\s*:\s*[^;]+;/gis, 'box-shadow: none;')
    .replace(/((?:-webkit-)?backdrop-filter)\s*:\s*[^;]+;/gis, '$1: none;')
    .replace(/(background(?:-image)?\s*:)\s*[^;]*(?:linear|radial|conic)-gradient\([^;]*;/gis, '$1 var(--bg-primary);')
    .replace(/(?<!-)transition\s*:\s*([^;]+);/gis, (match, value) => /^none(?:\s*!important)?$/i.test(value.trim())
      ? match
      : 'transition: background-color var(--transition), border-color var(--transition), color var(--transition), opacity var(--transition);')
    .replace(/(?<!-)animation\s*:\s*([^;]+);/gis, (match, value) => /^none(?:\s*!important)?$/i.test(value.trim()) ? match : 'animation: none;')
    .replace(/transform\s*:\s*(?:translateY|scale)\([^;]+;/gis, 'transform: none;')
    .replace(/transform\s*:\s*rotate\([^;]+;/gis, 'transform: none;')
    .replace(/transform\s*:\s*translateX\((?:2|16)px\)\s*;/gis, 'transform: none;')
    .replace(/border-radius\s*:\s*([^;]+);/gis, (_match, value) => `border-radius: ${normalizeRadiusValue(value)};`)
}

function normalizeSource(source) {
  return source
    .replaceAll('var(--radius-sm)', 'var(--radius-control)')
    .replaceAll('var(--radius-md)', 'var(--radius-surface)')
    .replaceAll('var(--radius-lg)', 'var(--radius-surface)')
}

function report(file, message) {
  violations.push(`${file.replaceAll('\\', '/')}: ${message}`)
}

function validateCss(file, source) {
  if (/(?:linear|radial|conic)-gradient\(/i.test(source)) report(file, 'gradient backgrounds are forbidden')
  for (const match of source.matchAll(/box-shadow\s*:\s*([^;]+);/gi)) {
    if (!/^none(?:\s*!important)?$/i.test(match[1].trim())) report(file, 'decorative box-shadow is forbidden')
  }
  for (const match of source.matchAll(/(?:-webkit-)?backdrop-filter\s*:\s*([^;]+);/gi)) {
    if (!/^none(?:\s*!important)?$/i.test(match[1].trim())) report(file, 'backdrop blur/filter is forbidden')
  }
  for (const match of source.matchAll(/(?<!-)transition\s*:\s*([^;]+);/gi)) {
    const value = match[1].trim()
    const canonical = 'background-color var(--transition), border-color var(--transition), color var(--transition), opacity var(--transition)'
    if (!/^none(?:\s*!important)?$/i.test(value) && value !== canonical) report(file, `motion transition is outside the contract (${value})`)
  }
  for (const match of source.matchAll(/(?<!-)animation\s*:\s*([^;]+);/gi)) {
    if (!/^none(?:\s*!important)?$/i.test(match[1].trim())) report(file, 'animation is outside the flat motion contract')
  }
  if (/transform\s*:\s*(?:translateY|scale)\(/i.test(source)) report(file, 'translateY/scale motion is forbidden')
  if (/var\(--radius-(?:sm|md|lg)\b/i.test(source)) report(file, 'legacy radius token is forbidden')

  for (const match of source.matchAll(/border-radius\s*:\s*([^;]+);/gi)) {
    const value = match[1].trim()
    if (/\d+(?:\.\d+)?(?:px|%|rem|em)\b/i.test(value)) {
      report(file, `hard-coded border radius is forbidden (${value})`)
    }
  }

  const roundAllowlist = /spinner|state-dot|typing-dot|live-tool-call-dot|crew-card-dot|needs-attention::after|sandbox-status-badge::before|toggle-(?:switch|track|knob)|input\[type=['"]radio['"]\]|progress-ring/i
  for (const block of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/border-radius\s*:\s*var\(--radius-round\)/i.test(block[2])) continue
    if (!roundAllowlist.test(block[1])) report(file, `round radius is not allowlisted for selector ${block[1].trim()}`)
  }
}

function validateSource(file, source) {
  if (/borderRadius\s*:\s*['"]?(?:\d+|[^'"\n]*var\(--radius-(?:sm|md|lg))/i.test(source)) {
    report(file, 'inline borderRadius must use a semantic flat-design token')
  }
  if (/boxShadow\s*:\s*(?!['"]none['"])/i.test(source)) report(file, 'inline boxShadow is forbidden')
}

if (!existsSync(sourceRoot)) {
  console.error(`Source root missing: ${sourceRoot}`)
  process.exit(1)
}

const files = walk(sourceRoot).filter((file) => sourceExtensions.has(extname(file)))
for (const file of files) {
  const source = readFileSync(file, 'utf8')
  const next = extname(file) === '.css' ? normalizeCss(source) : normalizeSource(source)
  if (fix && next !== source) writeFileSync(file, next, 'utf8')
  const checked = fix ? next : source
  if (extname(file) === '.css') validateCss(file, checked)
  else validateSource(file, checked)
}

if (violations.length > 0) {
  console.error(violations.join('\n'))
  process.exit(1)
}

console.log(`Flat design contract passed for ${files.length} source files${fix ? ' after normalization' : ''}.`)
