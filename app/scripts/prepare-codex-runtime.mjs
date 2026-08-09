import { createHash } from 'node:crypto'
import { spawn, spawnSync } from 'node:child_process'
import { chmodSync, cpSync, createWriteStream, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

const VERSION = '0.147.0'
const PROTOCOL_SCHEMA = `app-server-${VERSION}`
const LICENSE_URL = `https://raw.githubusercontent.com/openai/codex/rust-v${VERSION}/LICENSE`
const targets = {
  'windows-x64': {
    npmVersion: `${VERSION}-win32-x64`,
    archiveSha256: '299d8603750caaffc24f218789d989f77cf157070bd42451d352f5578a800766',
    binary: 'vendor/x86_64-pc-windows-msvc/bin/codex.exe',
  },
  'linux-x64': {
    npmVersion: `${VERSION}-linux-x64`,
    archiveSha256: 'c969740cf8297e4c31905cd551efeb2c99af5080c12c236bdf825598b250139a',
    binary: 'vendor/x86_64-unknown-linux-musl/bin/codex',
  },
}

function targetFromArgs() {
  const explicitIndex = process.argv.indexOf('--target')
  if (explicitIndex >= 0) return process.argv[explicitIndex + 1]
  if (process.arch !== 'x64') throw new Error(`Codex ${VERSION} is bundled only for x64 releases`)
  if (process.platform === 'win32') return 'windows-x64'
  if (process.platform === 'linux') return 'linux-x64'
  throw new Error(`Unsupported Codex bundle host: ${process.platform}-${process.arch}`)
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', stdio: 'pipe', ...options })
  if (result.error) {
    throw new Error(`${command} ${args.join(' ')} failed to start: ${result.error.message}`)
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed: ${(result.stderr || result.stdout || '').trim()}`)
  }
  return result.stdout.trim()
}

function cleanEnvironment(codexHome) {
  const environment = { ...process.env, CODEX_HOME: codexHome }
  for (const name of ['OPENAI_API_KEY', 'CODEX_API_KEY', 'AZURE_OPENAI_API_KEY', 'ANTHROPIC_API_KEY']) {
    delete environment[name]
  }
  return environment
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: 'follow' })
  if (!response.ok || !response.body) throw new Error(`Download failed (${response.status}): ${url}`)
  await pipeline(response.body, createWriteStream(destination))
}

async function verifyHandshake(binary, codexHome) {
  await new Promise((resolvePromise, reject) => {
    const child = spawn(binary, ['app-server', '--listen', 'stdio://'], {
      env: cleanEnvironment(codexHome),
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    })
    let stdout = ''
    let stderr = ''
    let settled = false
    const finish = (error) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      if (error) {
        child.kill()
        reject(error)
        return
      }
      child.once('close', () => resolvePromise())
      child.stdin.end()
      child.kill()
    }
    const timer = setTimeout(() => finish(new Error('Codex App Server handshake timed out')), 20_000)
    child.stderr.on('data', (chunk) => { stderr += chunk.toString() })
    child.on('error', (error) => finish(error))
    child.on('exit', (code) => {
      if (!settled) finish(new Error(`Codex App Server exited during handshake (${code}): ${stderr.slice(0, 500)}`))
    })
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString()
      const lines = stdout.split(/\r?\n/)
      stdout = lines.pop() ?? ''
      for (const line of lines) {
        if (!line.trim()) continue
        let message
        try { message = JSON.parse(line) } catch { continue }
        if (message.id === 1) {
          if (message.error || typeof message.result?.userAgent !== 'string') {
            finish(new Error(`Codex App Server returned an incompatible handshake: ${line}`))
            return
          }
          child.stdin.write(`${JSON.stringify({ method: 'initialized', params: {} })}\n`, () => finish())
        }
      }
    })
    child.stdin.write(`${JSON.stringify({
      method: 'initialize',
      id: 1,
      params: { clientInfo: { name: 'open_cowork_build', title: 'OpenCowork build verification', version: '0.3.0' } },
    })}\n`)
  })
}

const targetName = targetFromArgs()
const target = targets[targetName]
if (!target) throw new Error(`Unsupported --target value: ${targetName}`)

const appRoot = resolve(fileURLToPath(new URL('.', import.meta.url)), '..')
const destination = join(appRoot, 'src-tauri', 'resources', 'codex')
const manifestPath = join(destination, 'runtime-bundle-manifest.json')
const destinationBinary = join(destination, ...target.binary.split('/'))

if (existsSync(manifestPath) && existsSync(destinationBinary)) {
  const cached = JSON.parse(readFileSync(manifestPath, 'utf8'))
  if (
    cached.version === VERSION
    && cached.protocolSchema === PROTOCOL_SCHEMA
    && cached.target === targetName
    && cached.binary === target.binary
    && cached.sha256 === sha256(destinationBinary)
    && existsSync(join(destination, cached.license ?? 'LICENSE'))
  ) {
    console.log(`Verified cached Codex ${VERSION} bundle for ${targetName}`)
    process.exit(0)
  }
}

const temporaryRoot = join(tmpdir(), `open-cowork-codex-${process.pid}-${Date.now()}`)
const packageRoot = join(temporaryRoot, 'package')
const codexHome = join(temporaryRoot, 'codex-home')
mkdirSync(temporaryRoot, { recursive: true })
mkdirSync(codexHome, { recursive: true })
writeFileSync(join(codexHome, 'config.toml'), 'cli_auth_credentials_store = "keyring"\ncheck_for_update_on_startup = false\n')

try {
  const npmCli = process.env.npm_execpath
  const npmCommand = npmCli ? process.execPath : (process.platform === 'win32' ? 'npm.cmd' : 'npm')
  const npmPrefix = npmCli ? [npmCli] : []
  const packOutput = JSON.parse(run(npmCommand, [...npmPrefix,
    'pack', `@openai/codex@${target.npmVersion}`, '--pack-destination', temporaryRoot, '--json',
  ]))
  const archive = join(temporaryRoot, packOutput[0].filename)
  const archiveHash = sha256(archive)
  if (archiveHash !== target.archiveSha256) {
    throw new Error(`Codex npm archive SHA-256 mismatch: expected ${target.archiveSha256}, found ${archiveHash}`)
  }

  run('tar', ['-xf', archive, '-C', temporaryRoot])
  const packageJson = JSON.parse(readFileSync(join(packageRoot, 'package.json'), 'utf8'))
  if (packageJson.version !== target.npmVersion || packageJson.license !== 'Apache-2.0') {
    throw new Error(`Unexpected Codex package metadata for ${targetName}`)
  }

  rmSync(destination, { recursive: true, force: true })
  mkdirSync(destination, { recursive: true })
  cpSync(join(packageRoot, 'vendor'), join(destination, 'vendor'), { recursive: true })
  cpSync(join(packageRoot, 'README.md'), join(destination, 'README.md'))
  const licensePath = join(destination, 'LICENSE')
  await download(LICENSE_URL, licensePath)
  if (!/^\s*Apache License\b/m.test(readFileSync(licensePath, 'utf8'))) {
    throw new Error('Downloaded Codex license did not match Apache-2.0 text')
  }

  if (process.platform !== 'win32') chmodSync(destinationBinary, 0o755)
  const versionOutput = run(destinationBinary, ['--version'], { env: cleanEnvironment(codexHome), windowsHide: true })
  if (!versionOutput.includes(VERSION)) throw new Error(`Codex version check failed: ${versionOutput}`)
  await verifyHandshake(destinationBinary, codexHome)

  const manifest = {
    version: VERSION,
    protocolSchema: PROTOCOL_SCHEMA,
    target: targetName,
    package: `@openai/codex@${target.npmVersion}`,
    archiveSha256: archiveHash,
    binary: target.binary,
    sha256: sha256(destinationBinary),
    license: 'LICENSE',
    licenseSha256: sha256(licensePath),
    verifiedAt: new Date().toISOString(),
  }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
  console.log(`Prepared and verified Codex ${VERSION} bundle for ${targetName}`)
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 })
}
