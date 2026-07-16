import { existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { resolveNpmInvocation } from './process-utils.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const ciMode = process.argv.includes('--ci')

const checks = []

function run(command, args = []) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
  })
  return {
    ok: result.status === 0,
    output: `${result.stdout ?? ''}${result.stderr ?? ''}${result.error?.message ?? ''}`.trim(),
    status: result.status,
  }
}

function addCheck(name, ok, details, required = true) {
  checks.push({ name, ok, details, required })
}

function commandVersion(name, command, args = ['--version'], required = true) {
  const result = run(command, args)
  addCheck(name, result.ok, result.output || `Command "${command}" was not available.`, required)
}

addCheck('Node.js', true, `${process.version} (${process.execPath})`)
const npmInvocation = resolveNpmInvocation()
commandVersion('npm', npmInvocation.command, [...npmInvocation.args, '--version'], ciMode)
commandVersion('Cargo', 'cargo', ['--version'], ciMode)
commandVersion('Rust', 'rustc', ['--version'], ciMode)

addCheck('package.json', existsSync(join(root, 'package.json')), 'app/package.json exists')
addCheck('package-lock.json', existsSync(join(root, 'package-lock.json')), 'app/package-lock.json exists')
addCheck('node_modules', existsSync(join(root, 'node_modules')), 'app/node_modules exists', false)
addCheck('Tauri config', existsSync(join(root, 'src-tauri', 'tauri.conf.json')), 'src-tauri/tauri.conf.json exists')
addCheck('Rust manifest', existsSync(join(root, 'src-tauri', 'Cargo.toml')), 'src-tauri/Cargo.toml exists')

const missingRequired = checks.filter((check) => check.required && !check.ok)
const missingOptional = checks.filter((check) => !check.required && !check.ok)

for (const check of checks) {
  const marker = check.ok ? 'OK' : check.required ? 'FAIL' : 'WARN'
  console.log(`[${marker}] ${check.name}: ${check.details}`)
}

if (missingOptional.length > 0) {
  console.log(`\nOptional warnings: ${missingOptional.length}`)
}

if (missingRequired.length > 0) {
  console.error(`\nRequired checks failed: ${missingRequired.map((check) => check.name).join(', ')}`)
  process.exit(1)
}

console.log('\nOpen_Cowork doctor completed.')
