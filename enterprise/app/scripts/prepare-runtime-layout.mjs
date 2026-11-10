import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { lstat, readdir, readFile, rename, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export function safeRelative(value) {
  if (typeof value !== 'string' || !value || value.includes(':') || value.split(/[\\/]/).some((part) => !part || part === '.' || part === '..')) {
    throw new Error(`Unsafe runtime resource path: ${value}`)
  }
  return value.replaceAll('\\', '/')
}

async function hashFile(filename) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(filename)) hash.update(chunk)
  return hash.digest('hex')
}

export async function createRuntimeLayout(tauriRoot, config, target) {
  const artifacts = {}
  const add = async (source, destination) => {
    safeRelative(source)
    destination = safeRelative(destination)
    if (destination === 'runtime-layout.json') return
    const sourcePath = path.join(tauriRoot, source)
    const metadata = await lstat(sourcePath)
    if (metadata.isSymbolicLink()) throw new Error(`Linked runtime payload is forbidden: ${source}`)
    if (metadata.isDirectory()) {
      for (const child of (await readdir(sourcePath)).sort()) await add(`${source}/${child}`, `${destination}/${child}`)
      return
    }
    let name = destination
    if (destination === `daemon/${target}/manifest.json`) name = 'daemonManifest'
    if (destination === 'codex/runtime-bundle-manifest.json') name = 'codexManifest'
    if (destination === 'python/crew_runtime/runtime-bundle-manifest.json') name = 'crewManifest'
    if (destination === 'python/crew_runtime/main.py') name = 'crewScript'
    artifacts[name] = { path: destination, sha256: await hashFile(sourcePath) }
  }
  for (const [source, destination] of Object.entries(config.bundle.resources)) if (destination !== null) await add(source, destination)
  const required = ['daemonManifest', 'codexManifest', 'crewScript', ...(target === 'windows-x64' ? ['crewManifest'] : [])]
  for (const name of required) {
    if (!artifacts[name]) throw new Error(`Missing runtime artifact: ${name}`)
  }
  return { schemaVersion: 1, appVersion: config.version, target, artifacts }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src-tauri')
  const config = JSON.parse(await readFile(path.join(root, 'tauri.conf.json'), 'utf8'))
  const target = (process.env.TAURI_BUILD_TARGET || process.platform).includes('linux') ? 'linux-x64' : 'windows-x64'
  if (target === 'linux-x64') {
    const linux = JSON.parse(await readFile(path.join(root, 'tauri.linux.conf.json'), 'utf8'))
    config.bundle.resources = { ...config.bundle.resources, ...linux.bundle.resources }
  }
  const layout = await createRuntimeLayout(root, config, target)
  const filename = path.join(root, 'runtime-layout.json')
  await writeFile(`${filename}.tmp`, `${JSON.stringify(layout, null, 2)}\n`)
  await rename(`${filename}.tmp`, filename)
  console.log(`Validated runtime layout: ${layout.target}, ${Object.keys(layout.artifacts).length} artifacts`)
}
