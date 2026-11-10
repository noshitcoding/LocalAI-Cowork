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
  join(root, 'scripts', 'create-updater-manifest.test.mjs'),
  join(root, 'scripts', 'installer-native-sandbox.test.mjs'),
  join(root, 'scripts', 'process-utils.test.mjs'),
  join(root, 'scripts', 'release-metadata.test.mjs'),
  join(root, 'scripts', 'supply-chain.test.mjs'),
  join(root, 'scripts', 'verify-codex-bundle.test.mjs'),
  join(root, 'scripts', 'prepare-runtime-layout.test.mjs'),
])
runStep('Bundled CrewAI runtime', 'powershell', [
  '-NoProfile',
  '-ExecutionPolicy',
  'Bypass',
  '-File',
  join(root, 'scripts', 'prepare-crew-runtime.ps1'),
])
runStep('Current CrewAI Python regression suite (offline)', 'powershell', [
  '-NoProfile',
  '-ExecutionPolicy',
  'Bypass',
  '-File',
  join(root, 'scripts', 'prepare-crew-runtime.ps1'),
  '-VerifyOnly',
])
runStep('Bundled Codex runtime', node, [join(root, 'scripts', 'prepare-codex-runtime.mjs')])
runStep('Bundled local daemon', node, [join(root, 'scripts', 'prepare-local-daemon.mjs')])
runStep('Validated runtime layout', node, [join(root, 'scripts', 'prepare-runtime-layout.mjs')])
runStep('Supply-chain policy', node, [join(root, 'scripts', 'supply-chain.mjs'), 'check'])
runStep('Distributed contract artifacts', node, [join(root, 'scripts', 'generate-contract-artifacts.mjs'), '--check'])
runStep('TypeScript', node, [localBins.tsc, '-b'])
runStep('ESLint', node, [localBins.eslint, '.', '--max-warnings', '0'])
runStep('i18n audit', node, [join(root, 'scripts', 'i18n-audit.mjs')])
runStep('Flat-design contract', node, [join(root, 'scripts', 'check-flat-design.mjs')])
runStep('Vitest', node, [localBins.vitest, 'run', '--maxWorkers=4'])
runStep('Vite production build', node, [localBins.vite, 'build'])
runStep('Frontend build budgets', node, [join(root, 'scripts', 'check-budgets.mjs')])
runStep('UI accessibility and visual regressions', node, [localBins.playwright, 'test'])
runStep('Vite isolated web build', node, [localBins.vite, 'build', '--mode', 'web'])
runStep('Web platform boundary', node, [
  localBins.playwright,
  'test',
  '-c',
  join(root, 'playwright.web.config.ts'),
  '--project=chromium',
  'e2e/web-runtime.spec.ts',
])
runStep('Rust cargo check', 'cargo', ['check'], join(root, 'src-tauri'))
runStep('Rust tests', 'cargo', ['test'], join(root, 'src-tauri'))
runStep('Audit cross-process durability', 'cargo', ['test', '--lib', '--config', 'profile.test.package.app.opt-level=1', 'audit_service::ledger::tests::eight_processes_append_1600_verified_records', '--', '--ignored', '--exact'], join(root, 'src-tauri'))
runStep('Rust clippy', 'cargo', ['clippy', '--', '-D', 'warnings'], join(root, 'src-tauri'))

console.log('\nLocalAI Cowork verification completed.')
