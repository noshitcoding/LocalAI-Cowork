import { existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const node = process.execPath

function localBin(...segments) {
  return join(root, 'node_modules', ...segments)
}

function runStep(name, command, args, cwd = root) {
  console.log(`\n== ${name} ==`)
  const result = spawnSync(command, args, {
    cwd,
    stdio: 'inherit',
    encoding: 'utf8',
  })
  if (result.error) {
    throw new Error(`${name} could not start: ${result.error.message}`)
  }
  if (result.status !== 0) {
    throw new Error(`${name} failed with exit code ${result.status ?? 'unknown'}`)
  }
}

const localBins = {
  eslint: localBin('eslint', 'bin', 'eslint.js'),
  playwright: localBin('@playwright', 'test', 'cli.js'),
  tsc: localBin('typescript', 'bin', 'tsc'),
  vite: localBin('vite', 'bin', 'vite.js'),
  vitest: localBin('vitest', 'vitest.mjs'),
}

for (const [name, executable] of Object.entries(localBins)) {
  if (!existsSync(executable)) {
    throw new Error(`Missing local ${name} dependency: ${executable}. Run npm ci in app/.`)
  }
}

runStep('Toolchain doctor', node, [join(root, 'scripts', 'doctor.mjs'), '--ci'])
runStep('Release script tests', node, [
  '--test',
  join(root, 'scripts', 'process-utils.test.mjs'),
  join(root, 'scripts', 'supply-chain.test.mjs'),
])
runStep('Supply-chain policy', node, [join(root, 'scripts', 'supply-chain.mjs'), 'check'])
runStep('TypeScript', node, [localBins.tsc, '-b'])
runStep('ESLint', node, [localBins.eslint, '.', '--max-warnings', '0'])
runStep('i18n audit', node, [join(root, 'scripts', 'i18n-audit.mjs')])
runStep('Vitest', node, [localBins.vitest, 'run'])
runStep('Vite production build', node, [localBins.vite, 'build'])
runStep('Frontend build budgets', node, [join(root, 'scripts', 'check-budgets.mjs')])
runStep('UI accessibility and visual regressions', node, [localBins.playwright, 'test'])
runStep('Rust cargo check', 'cargo', ['check'], join(root, 'src-tauri'))
runStep('Rust tests', 'cargo', ['test'], join(root, 'src-tauri'))
runStep('Rust clippy', 'cargo', ['clippy', '--', '-D', 'warnings'], join(root, 'src-tauri'))

console.log('\nLocalAI Cowork verification completed.')
