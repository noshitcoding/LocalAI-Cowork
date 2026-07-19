import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import {
  assertVersionConsistency,
  assertTauriVersionCompatibility,
  collectInventory,
  createSbom,
  evaluateLicenseExpression,
  releaseWorkflowSigningErrors,
  workflowHardeningErrors,
  writeReleaseProvenance,
  writeSbomArtifacts,
} from './supply-chain.mjs'

const policy = {
  allowedLicenses: ['Apache-2.0', 'MIT'],
  allowedExceptions: ['LLVM-exception'],
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, 'utf8')
}

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'localai-cowork-supply-'))
  const tauri = join(root, 'src-tauri')
  mkdirSync(tauri, { recursive: true })
  writeJson(join(root, 'package.json'), {
    name: 'localai-cowork',
    version: '1.2.3',
    license: 'Apache-2.0',
    dependencies: { example: '1.0.0' },
  })
  writeJson(join(root, 'package-lock.json'), {
    name: 'localai-cowork',
    version: '1.2.3',
    lockfileVersion: 3,
    packages: {
      '': { name: 'localai-cowork', version: '1.2.3', license: 'Apache-2.0', dependencies: { example: '1.0.0' } },
      'node_modules/example': { version: '1.0.0', license: 'MIT', integrity: 'sha512-ZXhhbXBsZQ==' },
      'node_modules/build-only': { version: '2.0.0', license: 'Apache-2.0', dev: true },
    },
  })
  writeJson(join(root, 'supply-chain-policy.json'), { schemaVersion: 1, ...policy })
  writeJson(join(tauri, 'tauri.conf.json'), { version: '1.2.3' })
  writeFileSync(join(tauri, 'Cargo.toml'), '[package]\nname = "app"\nversion = "1.2.3"\nrust-version = "1.89"\nlicense = "Apache-2.0"\n', 'utf8')
  writeFileSync(join(tauri, 'Cargo.lock'), `version = 4\n\n[[package]]\nname = "app"\nversion = "1.2.3"\ndependencies = ["example-crate"]\n\n[[package]]\nname = "example-crate"\nversion = "4.5.6"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "${'a'.repeat(64)}"\n`, 'utf8')
  const rootId = 'path+file:///fixture#app@1.2.3'
  const dependencyId = 'registry+https://github.com/rust-lang/crates.io-index#example-crate@4.5.6'
  const cargoMetadata = {
    packages: [
      { id: rootId, name: 'app', version: '1.2.3', source: null, license: 'Apache-2.0', rust_version: '1.89' },
      {
        id: dependencyId,
        name: 'example-crate',
        version: '4.5.6',
        source: 'registry+https://github.com/rust-lang/crates.io-index',
        license: 'MIT OR Apache-2.0',
        rust_version: '1.70',
        repository: 'https://example.test/example-crate',
      },
    ],
    workspace_members: [rootId],
    resolve: {
      root: rootId,
      nodes: [
        { id: rootId, deps: [{ name: 'example_crate', pkg: dependencyId }] },
        { id: dependencyId, deps: [] },
      ],
    },
  }
  return { root, cargoMetadata }
}

test('license policy supports SPDX choice, conjunction, and exceptions', () => {
  assert.equal(evaluateLicenseExpression('MIT OR LGPL-2.1-or-later', policy), true)
  assert.equal(evaluateLicenseExpression('MIT AND LGPL-2.1-or-later', policy), false)
  assert.equal(evaluateLicenseExpression('Apache-2.0 WITH LLVM-exception', policy), true)
  assert.equal(evaluateLicenseExpression('Apache-2.0 WITH Classpath-exception-2.0', policy), false)
  assert.equal(evaluateLicenseExpression('SEE LICENSE IN LICENSE.txt', policy), false)
})

test('version gate rejects metadata drift and mismatched tags', () => {
  assert.equal(assertVersionConsistency({
    packageVersion: '1.2.3',
    packageLockVersion: '1.2.3',
    cargoPackageVersion: '1.2.3',
    tauriVersion: '1.2.3',
    releaseTag: 'v1.2.3',
  }), '1.2.3')
  assert.throws(() => assertVersionConsistency({
    packageVersion: '1.2.3',
    packageLockVersion: '1.2.3',
    cargoPackageVersion: '1.2.2',
    tauriVersion: '1.2.3',
  }), /version mismatch/i)
  assert.throws(() => assertVersionConsistency({
    packageVersion: '1.2.3',
    packageLockVersion: '1.2.3',
    cargoPackageVersion: '1.2.3',
    tauriVersion: '1.2.3',
    releaseTag: 'v9.9.9',
  }), /does not match/)
})

test('Tauri Rust, API, and CLI packages must share a major/minor release line', () => {
  assert.doesNotThrow(() => assertTauriVersionCompatibility({
    rustTauri: '2.11.5',
    npmApi: '2.11.1',
    npmCli: '2.11.4',
  }))
  assert.throws(() => assertTauriVersionCompatibility({
    rustTauri: '2.11.5',
    npmApi: '2.10.1',
    npmCli: '2.11.4',
  }), /Tauri release-line mismatch/)
})

test('workflow policy rejects mutable action tags and ignored audits', () => {
  assert.deepEqual(workflowHardeningErrors('uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5\n'), [])
  const errors = workflowHardeningErrors('uses: actions/checkout@v4\nrun: npm audit --audit-level=high || true\n')
  assert.ok(errors.some((error) => error.includes('not pinned')))
  assert.ok(errors.some((error) => error.includes('non-blocking')))
})

test('release policy requires secret-backed Authenticode before release evidence', () => {
  const valid = `
env:
  LOCALAI_COWORK_CODESIGN_PFX_BASE64: \${{ secrets.LOCALAI_COWORK_CODESIGN_PFX_BASE64 }}
  LOCALAI_COWORK_CODESIGN_PASSWORD: \${{ secrets.LOCALAI_COWORK_CODESIGN_PASSWORD }}
  LOCALAI_COWORK_CODESIGN_THUMBPRINT: \${{ secrets.LOCALAI_COWORK_CODESIGN_THUMBPRINT }}
run: .\\app\\scripts\\sign-windows-installer.ps1 -InstallerPath release-assets\\LocalAI-Cowork-Setup-x64.exe
next: supply-chain:sbom
then: supply-chain:release
uses: actions/attest-build-provenance@commit
publish: action-gh-release
`
  assert.deepEqual(releaseWorkflowSigningErrors(valid), [])
  const invalid = valid
    .replace('run: .\\app\\scripts\\sign-windows-installer.ps1 -InstallerPath release-assets\\LocalAI-Cowork-Setup-x64.exe\n', '')
    .replace('LOCALAI_COWORK_CODESIGN_PASSWORD: \${{ secrets.LOCALAI_COWORK_CODESIGN_PASSWORD }}', 'LOCALAI_COWORK_CODESIGN_PASSWORD: plaintext')
  const errors = releaseWorkflowSigningErrors(invalid)
  assert.ok(errors.some((error) => error.includes('missing Authenticode')))
  assert.ok(errors.some((error) => error.includes('secret-backed LOCALAI_COWORK_CODESIGN_PASSWORD')))
  assert.ok(releaseWorkflowSigningErrors(`${valid}\nrun: -TestSkipTimestamp`).some((error) => error.includes('test bypasses')))
})

test('installer tool discovery preserves a single vswhere path as an array element', () => {
  const installerScript = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'build-installer.ps1'), 'utf8')
  assert.match(installerScript, /\$vswhere\s*=\s*@\(\$vswhereCandidates\)\[0\]/)
  assert.doesNotMatch(installerScript, /&\s+\$vswhereCandidates\[0\]/)
})

test('inventory checks all licenses but excludes dev-only npm packages from release components', () => {
  const { root, cargoMetadata } = fixture()
  try {
    const inventory = collectInventory(root, { policy, cargoMetadata })
    assert.equal(inventory.records.length, 3)
    assert.ok(inventory.records.some((entry) => entry.name === 'build-only'))
    assert.ok(inventory.components.some((entry) => entry.purl === 'pkg:npm/example@1.0.0'))
    assert.ok(!inventory.components.some((entry) => entry.name === 'build-only'))
    assert.ok(inventory.components.some((entry) => entry.purl === 'pkg:cargo/example-crate@4.5.6'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('inventory fails closed for a denied or missing dependency license', () => {
  const { root, cargoMetadata } = fixture()
  try {
    const lockPath = join(root, 'package-lock.json')
    const lock = JSON.parse(readFileSync(lockPath, 'utf8'))
    lock.packages['node_modules/example'].license = 'GPL-3.0-only'
    writeJson(lockPath, lock)
    assert.throws(() => collectInventory(root, { policy, cargoMetadata }), /license policy rejected/i)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('inventory rejects dependencies above the declared Rust toolchain floor', () => {
  const { root, cargoMetadata } = fixture()
  try {
    cargoMetadata.packages[1].rust_version = '1.90'
    assert.throws(() => collectInventory(root, { policy, cargoMetadata }), /below dependency requirements/i)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('CycloneDX output is deterministic for identical lockfiles and context', () => {
  const { root, cargoMetadata } = fixture()
  const context = { commit: 'a'.repeat(40), timestamp: '2026-07-11T00:00:00Z' }
  try {
    const first = createSbom(root, { policy, cargoMetadata, context })
    const second = createSbom(root, { policy, cargoMetadata, context })
    assert.deepEqual(first, second)
    assert.equal(first.bomFormat, 'CycloneDX')
    assert.equal(first.specVersion, '1.6')
    assert.match(first.serialNumber, /^urn:uuid:/)
    assert.ok(first.components.some((entry) => entry.hashes?.[0]?.alg === 'SHA-256'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('release metadata hashes SBOM, notices, installer, provenance, and materials', () => {
  const { root, cargoMetadata } = fixture()
  const output = join(root, 'release-assets')
  const context = { commit: 'b'.repeat(40), timestamp: '2026-07-11T00:00:00Z' }
  mkdirSync(output)
  writeFileSync(join(output, 'LocalAI-Cowork-Setup-x64.exe'), 'installer-fixture', 'utf8')
  try {
    writeSbomArtifacts(root, output, { policy, cargoMetadata, context, releaseTag: 'v1.2.3' })
    const provenance = writeReleaseProvenance(root, output, { context, releaseTag: 'v1.2.3' })
    const sums = readFileSync(join(output, 'SHA256SUMS'), 'utf8')
    assert.equal(provenance.build.version, '1.2.3')
    assert.ok(provenance.subject.some((entry) => entry.name === 'localai-cowork.cdx.json'))
    assert.ok(provenance.materials.some((entry) => entry.path === 'src-tauri/Cargo.lock'))
    assert.match(sums, /LocalAI-Cowork-Setup-x64\.exe/)
    assert.match(sums, /localai-cowork\.cdx\.json/)
    assert.match(sums, /THIRD_PARTY_NOTICES\.json/)
    assert.match(sums, /release-provenance\.json/)
    assert.ok(!sums.includes('SHA256SUMS'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
