import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const node = process.execPath

function runStep(name, command, args, options = {}) {
  console.log(`\n== ${name} ==`)
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    stdio: 'inherit',
    encoding: 'utf8',
  })
  if (result.status !== 0) {
    throw new Error(`${name} failed with exit code ${result.status ?? 'unknown'}`)
  }
}

function localBin(...segments) {
  return join(root, 'node_modules', ...segments)
}

const requiredBins = [
  localBin('typescript', 'bin', 'tsc'),
  localBin('eslint', 'bin', 'eslint.js'),
  localBin('vitest', 'vitest.mjs'),
  localBin('vite', 'bin', 'vite.js'),
  localBin('@tauri-apps', 'cli', 'tauri.js'),
]

for (const bin of requiredBins) {
  if (!existsSync(bin)) {
    throw new Error(`Missing local dependency: ${bin}. Run npm ci in app/.`)
  }
}

runStep('Doctor', node, [join(root, 'scripts', 'doctor.mjs')])
runStep('ESLint', node, [localBin('eslint', 'bin', 'eslint.js'), '.', '--max-warnings', '0'])
runStep('Vitest', node, [localBin('vitest', 'vitest.mjs'), 'run'])
runStep('Tauri release build', node, [localBin('@tauri-apps', 'cli', 'tauri.js'), 'build', '--no-bundle', '--ci'])
runStep('Build budgets', node, [join(root, 'scripts', 'check-budgets.mjs')])
runStep('Native Windows launch', 'powershell', [
  '-NoProfile',
  '-NonInteractive',
  '-ExecutionPolicy',
  'Bypass',
  '-File',
  join(root, 'scripts', 'native-desktop-launch-smoke.ps1'),
])

console.log('\nDesktop smoke completed.')
