import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const appRoot = join(dirname(fileURLToPath(import.meta.url)), '..')
const repositoryRoot = join(appRoot, '..')

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

test('OpenVEX application product IDs match the release version', () => {
  const appVersion = readJson(join(appRoot, 'package.json')).version
  const androidVersion = readJson(join(repositoryRoot, 'clients', 'android', 'package.json')).version
  const cargoManifest = readFileSync(join(repositoryRoot, 'Cargo.toml'), 'utf8')
  const workspaceVersion = cargoManifest.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
  assert.ok(workspaceVersion, 'workspace package version must be present')

  const expectedProducts = new Map([
    ['app', appVersion],
    ['open-cowork-android', androidVersion],
    ['cowork-server', workspaceVersion],
  ])
  const vex = readJson(join(repositoryRoot, '.vex', 'localai-cowork.openvex.json'))
  let checked = 0
  for (const statement of vex.statements ?? []) {
    for (const product of statement.products ?? []) {
      const match = String(product['@id'] ?? '').match(/^pkg:cargo\/([^@]+)@(.+)$/)
      if (!match || !expectedProducts.has(match[1])) continue
      assert.equal(
        match[2],
        expectedProducts.get(match[1]),
        `${match[1]} OpenVEX product version must match its release manifest`,
      )
      checked += 1
    }
  }
  assert.equal(checked, 3, 'all release-versioned OpenVEX products must be checked')
})
