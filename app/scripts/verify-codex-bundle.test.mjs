import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { gzipSync } from 'node:zlib'
import { CODEX_BUNDLE_VERSION, CODEX_PROTOCOL_SCHEMA, verifyCodexBundle } from './verify-codex-bundle.mjs'

function hash(value) {
  return createHash('sha256').update(value).digest('hex')
}

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'open-cowork-codex-verify-'))
  const binaryValue = Buffer.from('test-codex-binary')
  const licenseValue = Buffer.from('Apache License\nVersion 2.0\n')
  const binary = 'vendor/test/bin/codex'
  mkdirSync(join(root, 'vendor/test/bin'), { recursive: true })
  writeFileSync(join(root, binary), binaryValue)
  writeFileSync(join(root, 'LICENSE'), licenseValue)
  writeFileSync(join(root, 'runtime-bundle-manifest.json'), JSON.stringify({
    version: CODEX_BUNDLE_VERSION,
    protocolSchema: CODEX_PROTOCOL_SCHEMA,
    target: 'linux-x64',
    binary,
    sha256: hash(binaryValue),
    license: 'LICENSE',
    licenseSha256: hash(licenseValue),
  }))
  return root
}

test('verifies an intact pinned Codex bundle', () => {
  const root = fixture()
  try {
    assert.equal(verifyCodexBundle(root, { expectedTarget: 'linux-x64', verifyExecutable: false }).manifest.version, CODEX_BUNDLE_VERSION)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('rejects target drift and path traversal', () => {
  const root = fixture()
  try {
    assert.throws(() => verifyCodexBundle(root, { expectedTarget: 'windows-x64', verifyExecutable: false }), /Unexpected Codex target/)
    const manifestPath = join(root, 'runtime-bundle-manifest.json')
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
    writeFileSync(manifestPath, JSON.stringify({ ...manifest, binary: '../codex' }))
    assert.throws(() => verifyCodexBundle(root, { expectedTarget: 'linux-x64', verifyExecutable: false }), /escapes the Codex bundle root/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('verifies an archived Linux executable and rejects archive tampering', () => {
  const root = fixture()
  try {
    const manifestPath = join(root, 'runtime-bundle-manifest.json')
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
    const binaryPath = join(root, manifest.binary)
    const binaryBytes = readFileSync(binaryPath)
    const archive = `${manifest.binary}.gz`
    const archivePath = join(root, archive)
    const archiveBytes = gzipSync(binaryBytes)
    writeFileSync(archivePath, archiveBytes)
    rmSync(binaryPath)
    writeFileSync(manifestPath, JSON.stringify({
      ...manifest,
      binaryArchive: archive,
      binaryArchiveSha256: hash(archiveBytes),
      binarySize: binaryBytes.length,
    }))

    assert.equal(
      verifyCodexBundle(root, { expectedTarget: 'linux-x64', verifyExecutable: false }).manifest.version,
      CODEX_BUNDLE_VERSION,
    )
    writeFileSync(archivePath, Buffer.from('tampered'))
    assert.throws(
      () => verifyCodexBundle(root, { expectedTarget: 'linux-x64', verifyExecutable: false }),
      /archive SHA-256/,
    )
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
