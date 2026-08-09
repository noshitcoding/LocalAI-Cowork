import { createHash } from 'node:crypto'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname, isAbsolute, relative, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

export const CODEX_BUNDLE_VERSION = '0.147.0'
export const CODEX_PROTOCOL_SCHEMA = `app-server-${CODEX_BUNDLE_VERSION}`

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function resolveBundleFile(bundleRoot, candidate, label) {
  if (typeof candidate !== 'string' || !candidate.trim() || isAbsolute(candidate)) {
    throw new Error(`${label} must be a relative bundle path`)
  }
  const root = resolve(bundleRoot)
  const path = resolve(root, candidate)
  const pathFromRoot = relative(root, path)
  if (!pathFromRoot || pathFromRoot === '..' || pathFromRoot.startsWith(`..\\`) || pathFromRoot.startsWith('../')) {
    throw new Error(`${label} escapes the Codex bundle root`)
  }
  if (!existsSync(path) || !statSync(path).isFile()) throw new Error(`${label} is missing: ${path}`)
  return path
}

export function verifyCodexBundle(bundleRoot, { expectedTarget, verifyExecutable = true } = {}) {
  const root = resolve(bundleRoot)
  const manifestPath = resolve(root, 'runtime-bundle-manifest.json')
  if (!existsSync(manifestPath)) throw new Error(`Codex runtime manifest is missing: ${manifestPath}`)
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  if (manifest.version !== CODEX_BUNDLE_VERSION) {
    throw new Error(`Unexpected Codex version: ${manifest.version ?? '<missing>'}`)
  }
  if (manifest.protocolSchema !== CODEX_PROTOCOL_SCHEMA) {
    throw new Error(`Unexpected Codex protocol schema: ${manifest.protocolSchema ?? '<missing>'}`)
  }
  if (expectedTarget && manifest.target !== expectedTarget) {
    throw new Error(`Unexpected Codex target: expected ${expectedTarget}, found ${manifest.target ?? '<missing>'}`)
  }

  const binary = resolveBundleFile(root, manifest.binary, 'Codex executable')
  const license = resolveBundleFile(root, manifest.license, 'Codex license')
  if (sha256(binary) !== manifest.sha256) throw new Error('Codex executable SHA-256 does not match its manifest')
  if (sha256(license) !== manifest.licenseSha256) throw new Error('Codex license SHA-256 does not match its manifest')
  if (!/^\s*Apache License\b/m.test(readFileSync(license, 'utf8'))) {
    throw new Error('Codex bundle license is not Apache-2.0')
  }
  if (existsSync(resolve(root, 'auth.json'))) throw new Error('Codex bundle must not contain auth.json')

  if (verifyExecutable) {
    const version = spawnSync(binary, ['--version'], { encoding: 'utf8', windowsHide: true })
    if (version.error || version.status !== 0 || !version.stdout.includes(CODEX_BUNDLE_VERSION)) {
      throw new Error(`Bundled Codex executable failed its version probe: ${(version.stderr || version.error?.message || '').trim()}`)
    }
  }

  return { manifestPath, binary, license, manifest }
}

function readArgument(name) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : undefined
}

const directScript = process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))
if (directScript) {
  const root = readArgument('--root')
  if (!root) throw new Error('Usage: node verify-codex-bundle.mjs --root <bundle-dir> [--target windows-x64|linux-x64]')
  const result = verifyCodexBundle(root, {
    expectedTarget: readArgument('--target'),
    verifyExecutable: !process.argv.includes('--skip-executable'),
  })
  console.log(`Verified packaged Codex ${result.manifest.version} bundle at ${dirname(result.manifestPath)}`)
}
