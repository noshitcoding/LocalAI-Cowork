import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { createRuntimeLayout, safeRelative } from './prepare-runtime-layout.mjs'

test('runtime paths cannot escape on Windows or Unix', () => {
  for (const value of ['', '../x', 'C:\\x', '/x', '\\\\host\\x', 'a/../x', 'a\\..\\x']) assert.throws(() => safeRelative(value))
  assert.equal(safeRelative('daemon/windows-x64/manifest.json'), 'daemon/windows-x64/manifest.json')
})
test('layout binds the exact mapped payloads and fails on missing required resources', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'lacowork-layout-'))
  try {
    await mkdir(path.join(root, 'source'))
    await writeFile(path.join(root, 'source/file'), 'fixture')
    const config = { version: 'test', bundle: { resources: {
      'source/file': 'daemon/windows-x64/manifest.json',
      'source': 'codex',
    } } }
    await assert.rejects(createRuntimeLayout(root, config, 'windows-x64'), /Missing runtime artifact/)
    const destinations = ['daemon/windows-x64/manifest.json', 'codex/runtime-bundle-manifest.json', 'python/crew_runtime/runtime-bundle-manifest.json', 'python/crew_runtime/main.py']
    config.bundle.resources = {}
    for (const [index, destination] of destinations.entries()) {
      await writeFile(path.join(root, `fixture${index}`), 'fixture')
      config.bundle.resources[`fixture${index}`] = destination
    }
    const layout = await createRuntimeLayout(root, config, 'windows-x64')
    assert.equal(layout.appVersion, 'test')
    assert.equal(layout.artifacts.crewScript.path, destinations[3])
    assert.match(layout.artifacts.daemonManifest.sha256, /^[a-f0-9]{64}$/)
  } finally { await rm(root, { recursive: true, force: true }) }
})
