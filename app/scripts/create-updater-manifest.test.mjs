import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createUpdaterManifest, writeUpdaterManifest } from './create-updater-manifest.mjs'

test('creates a Tauri static updater manifest for the signed Windows installer', () => {
  const root = mkdtempSync(join(tmpdir(), 'localai-updater-manifest-'))
  try {
    const artifactPath = join(root, 'LocalAI-Cowork-Setup-x64.exe')
    const outputPath = join(root, 'latest.json')
    writeFileSync(artifactPath, 'installer')
    writeFileSync(`${artifactPath}.sig`, 'trusted-signature\n')

    const manifest = writeUpdaterManifest({
      tag: 'v1.2.3',
      artifactPath,
      repository: 'noshitcoding/LocalAI-Cowork',
      publishedAt: '2026-07-29T12:00:00Z',
    }, outputPath)

    assert.equal(manifest.version, '1.2.3')
    assert.equal(manifest.platforms['windows-x86_64'].signature, 'trusted-signature')
    assert.equal(
      manifest.platforms['windows-x86_64'].url,
      'https://github.com/noshitcoding/LocalAI-Cowork/releases/download/v1.2.3/LocalAI-Cowork-Setup-x64.exe',
    )
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('creates one signed updater manifest for Windows and Linux', () => {
  const root = mkdtempSync(join(tmpdir(), 'localai-updater-manifest-platforms-'))
  try {
    const windows = join(root, 'LocalAI-Cowork-Setup-x64.exe')
    const linux = join(root, 'LocalAI-Cowork-x86_64.AppImage')
    writeFileSync(windows, 'windows-installer')
    writeFileSync(`${windows}.sig`, 'windows-signature')
    writeFileSync(linux, 'linux-appimage')
    writeFileSync(`${linux}.sig`, 'linux-signature')

    const manifest = createUpdaterManifest({
      tag: 'v2.0.0-beta.1',
      repository: 'noshitcoding/LocalAI-Cowork',
      publishedAt: '2026-08-08T12:00:00Z',
      platforms: {
        'windows-x86_64': { artifactPath: windows },
        'linux-x86_64': { artifactPath: linux },
      },
    })

    assert.equal(manifest.version, '2.0.0-beta.1')
    assert.equal(manifest.platforms['windows-x86_64'].signature, 'windows-signature')
    assert.equal(manifest.platforms['linux-x86_64'].signature, 'linux-signature')
    assert.match(manifest.platforms['linux-x86_64'].url, /LocalAI-Cowork-x86_64\.AppImage$/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('fails closed when the updater signature or release tag is invalid', () => {
  const root = mkdtempSync(join(tmpdir(), 'localai-updater-manifest-invalid-'))
  try {
    const artifactPath = join(root, 'installer.exe')
    writeFileSync(artifactPath, 'installer')
    assert.throws(() => createUpdaterManifest({
      tag: 'latest',
      artifactPath,
      repository: 'noshitcoding/LocalAI-Cowork',
    }), /semantic release tag/)
    assert.throws(() => createUpdaterManifest({
      tag: 'v1.2.3',
      artifactPath,
      repository: 'noshitcoding/LocalAI-Cowork',
    }), /signature not found/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
