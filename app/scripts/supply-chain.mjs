import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptPath = fileURLToPath(import.meta.url)
const defaultAppRoot = resolve(dirname(scriptPath), '..')

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}

function fileHash(path) {
  const bytes = readFileSync(path)
  return { bytes: bytes.length, sha256: sha256(bytes) }
}

function command(commandName, args, cwd) {
  return execFileSync(commandName, args, {
    cwd,
    encoding: 'utf8',
    maxBuffer: 128 * 1024 * 1024,
    windowsHide: true,
  }).trim()
}

function optionalCommand(commandName, args, cwd, fallback = '') {
  try {
    return command(commandName, args, cwd)
  } catch {
    return fallback
  }
}

function cargoVersion(cargoToml) {
  const packageStart = cargoToml.search(/^\[package\]\s*$/m)
  if (packageStart < 0) throw new Error('src-tauri/Cargo.toml has no package section')
  const packageRemainder = cargoToml.slice(packageStart).replace(/^\[package\]\s*(?:\r?\n)?/, '')
  const nextSection = packageRemainder.search(/^\[/m)
  const packageBlock = nextSection >= 0 ? packageRemainder.slice(0, nextSection) : packageRemainder
  const version = packageBlock.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1]
  if (!version) throw new Error('src-tauri/Cargo.toml has no package version')
  return version
}

function cargoRustVersion(cargoToml) {
  const value = cargoToml.match(/^rust-version\s*=\s*"([^"]+)"\s*$/m)?.[1]
  if (!value) throw new Error('src-tauri/Cargo.toml has no rust-version')
  return value
}

function numericVersion(value) {
  const parts = String(value).split('.').map((part) => Number.parseInt(part, 10))
  if (parts.some(Number.isNaN)) throw new Error(`Invalid numeric version: ${value}`)
  return parts
}

function compareNumericVersions(left, right) {
  const a = numericVersion(left)
  const b = numericVersion(right)
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0)
    if (difference !== 0) return difference
  }
  return 0
}

export function assertTauriVersionCompatibility({ rustTauri, npmApi, npmCli }) {
  const releaseLine = (value) => String(value).split('.').slice(0, 2).join('.')
  const expected = releaseLine(rustTauri)
  const mismatches = [
    ['@tauri-apps/api', npmApi],
    ['@tauri-apps/cli', npmCli],
  ].filter(([, value]) => releaseLine(value) !== expected)
  if (mismatches.length > 0) {
    throw new Error(`Tauri release-line mismatch: tauri=${rustTauri}, ${mismatches.map(([name, value]) => `${name}=${value}`).join(', ')}`)
  }
}

export function assertVersionConsistency({ packageVersion, packageLockVersion, cargoPackageVersion, tauriVersion, releaseTag }) {
  const versions = new Map([
    ['package.json', packageVersion],
    ['package-lock.json', packageLockVersion],
    ['Cargo.toml', cargoPackageVersion],
    ['tauri.conf.json', tauriVersion],
  ])
  const expected = packageVersion
  const mismatches = [...versions].filter(([, version]) => version !== expected)
  if (mismatches.length > 0) {
    throw new Error(`Release version mismatch: ${[...versions].map(([name, version]) => `${name}=${version}`).join(', ')}`)
  }
  if (releaseTag && releaseTag !== `v${expected}`) {
    throw new Error(`Release tag ${releaseTag} does not match product version v${expected}`)
  }
  return expected
}

function releaseVersion(appRoot, releaseTag = '') {
  const packageJson = readJson(join(appRoot, 'package.json'))
  const packageLock = readJson(join(appRoot, 'package-lock.json'))
  const tauri = readJson(join(appRoot, 'src-tauri', 'tauri.conf.json'))
  const cargoToml = readFileSync(join(appRoot, 'src-tauri', 'Cargo.toml'), 'utf8')
  const version = assertVersionConsistency({
    packageVersion: packageJson.version,
    packageLockVersion: packageLock.packages?.['']?.version ?? packageLock.version,
    cargoPackageVersion: cargoVersion(cargoToml),
    tauriVersion: tauri.version,
    releaseTag,
  })
  const rustVersion = cargoRustVersion(cargoToml)
  const toolchainPath = resolve(appRoot, '..', 'rust-toolchain.toml')
  if (existsSync(toolchainPath)) {
    const channel = readFileSync(toolchainPath, 'utf8').match(/^channel\s*=\s*"([^"]+)"\s*$/m)?.[1]
    if (!channel || compareNumericVersions(channel, rustVersion) !== 0) {
      throw new Error(`Rust toolchain ${channel || '<missing>'} does not match Cargo rust-version ${rustVersion}`)
    }
  }
  return version
}

function normalizeLicenseExpression(expression) {
  return expression.trim().replace(/\s*\/\s*/g, ' OR ').replace(/\s+/g, ' ')
}

function tokenizeLicense(expression) {
  const normalized = normalizeLicenseExpression(expression)
  const tokens = normalized.match(/\(|\)|\bAND\b|\bOR\b|\bWITH\b|[A-Za-z0-9.+-]+/g) ?? []
  if (tokens.join('').replace(/[()]/g, '') !== normalized.replace(/\s+/g, '').replace(/[()]/g, '')) {
    throw new Error(`Unsupported license expression syntax: ${expression}`)
  }
  return { normalized, tokens }
}

export function evaluateLicenseExpression(expression, policy) {
  if (!expression || typeof expression !== 'string') return false
  const allowed = new Set(policy.allowedLicenses)
  const allowedExceptions = new Set(policy.allowedExceptions ?? [])
  let parsed
  try {
    parsed = tokenizeLicense(expression)
  } catch {
    return false
  }
  const { tokens } = parsed
  let index = 0

  function parsePrimary() {
    if (tokens[index] === '(') {
      index += 1
      const value = parseOr()
      if (tokens[index] !== ')') throw new Error('missing closing parenthesis')
      index += 1
      return value
    }
    const identifier = tokens[index]
    if (!identifier || ['AND', 'OR', 'WITH', ')'].includes(identifier)) throw new Error('expected license identifier')
    index += 1
    let value = allowed.has(identifier)
    if (tokens[index] === 'WITH') {
      index += 1
      const exception = tokens[index]
      if (!exception || ['AND', 'OR', 'WITH', '(', ')'].includes(exception)) throw new Error('expected exception identifier')
      index += 1
      value = value && allowedExceptions.has(exception)
    }
    return value
  }

  function parseAnd() {
    let value = parsePrimary()
    while (tokens[index] === 'AND') {
      index += 1
      const right = parsePrimary()
      value = value && right
    }
    return value
  }

  function parseOr() {
    let value = parseAnd()
    while (tokens[index] === 'OR') {
      index += 1
      const right = parseAnd()
      value = value || right
    }
    return value
  }

  try {
    const approved = parseOr()
    return approved && index === tokens.length
  } catch {
    return false
  }
}

function npmName(packagePath, metadata) {
  if (metadata.name) return metadata.name
  const marker = 'node_modules/'
  const tail = packagePath.slice(packagePath.lastIndexOf(marker) + marker.length)
  const segments = tail.split('/')
  return segments[0]?.startsWith('@') ? segments.slice(0, 2).join('/') : segments[0]
}

function npmPurl(name, version) {
  if (name.startsWith('@')) {
    const [scope, packageName] = name.split('/')
    return `pkg:npm/${encodeURIComponent(scope)}/${encodeURIComponent(packageName)}@${encodeURIComponent(version)}`
  }
  return `pkg:npm/${encodeURIComponent(name)}@${encodeURIComponent(version)}`
}

function cargoPurl(name, version) {
  return `pkg:cargo/${encodeURIComponent(name)}@${encodeURIComponent(version)}`
}

function externalReferences(metadata) {
  const references = []
  if (metadata.repository) references.push({ type: 'vcs', url: metadata.repository })
  if (metadata.homepage) references.push({ type: 'website', url: metadata.homepage })
  return references.length > 0 ? references : undefined
}

function integrityHash(integrity) {
  const match = typeof integrity === 'string' ? integrity.match(/^sha512-(.+)$/) : null
  return match ? [{ alg: 'SHA-512', content: Buffer.from(match[1], 'base64').toString('hex') }] : undefined
}

function parseCargoChecksums(lockText) {
  const checksums = new Map()
  for (const block of lockText.split('[[package]]').slice(1)) {
    const name = block.match(/^\s*name\s*=\s*"([^"]+)"/m)?.[1]
    const version = block.match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1]
    const source = block.match(/^\s*source\s*=\s*"([^"]+)"/m)?.[1] ?? ''
    const checksum = block.match(/^\s*checksum\s*=\s*"([a-f0-9]{64})"/m)?.[1]
    if (name && version && checksum) checksums.set(`${name}\0${version}\0${source}`, checksum)
  }
  return checksums
}

function reachableCargoIds(metadata) {
  const nodes = new Map((metadata.resolve?.nodes ?? []).map((node) => [node.id, node]))
  const queue = [metadata.resolve?.root, ...(metadata.workspace_members ?? [])].filter(Boolean)
  const reachable = new Set()
  while (queue.length > 0) {
    const id = queue.pop()
    if (reachable.has(id)) continue
    reachable.add(id)
    const node = nodes.get(id)
    for (const dependency of node?.deps ?? []) queue.push(dependency.pkg)
  }
  return reachable
}

function validateLicenses(records, policy) {
  const errors = records
    .filter((record) => !evaluateLicenseExpression(record.license, policy))
    .map((record) => `${record.ecosystem}:${record.name}@${record.version}: ${record.license || '<missing>'}`)
  if (errors.length > 0) {
    throw new Error(`Dependency license policy rejected ${errors.length} package(s):\n${errors.slice(0, 30).join('\n')}`)
  }
}

export function workflowHardeningErrors(text) {
  const errors = []
  for (const match of text.matchAll(/^\s*uses:\s*([^\s#]+)(?:\s*#.*)?$/gm)) {
    const reference = match[1]
    if (reference.startsWith('./')) continue
    const separator = reference.lastIndexOf('@')
    const revision = separator >= 0 ? reference.slice(separator + 1) : ''
    if (!/^[a-f0-9]{40}$/.test(revision)) errors.push(`Action is not pinned to a commit: ${reference}`)
  }
  if (/npm audit[^\r\n]*(?:\|\|\s*true|continue-on-error)/i.test(text)) {
    errors.push('npm audit is configured as non-blocking')
  }
  return errors
}

export function releaseWorkflowSigningErrors(text) {
  const errors = []
  const signingIndex = text.indexOf('sign-windows-installer.ps1')
  const sbomIndex = text.indexOf('supply-chain:sbom')
  const provenanceIndex = text.indexOf('supply-chain:release')
  const attestationIndex = text.indexOf('attest-build-provenance')
  const publicationIndex = text.indexOf('action-gh-release')
  if (signingIndex < 0) errors.push('release workflow is missing Authenticode signing')
  for (const [name, index] of [
    ['SBOM generation', sbomIndex],
    ['release provenance', provenanceIndex],
    ['build attestation', attestationIndex],
    ['release publication', publicationIndex],
  ]) {
    if (index >= 0 && signingIndex >= index) errors.push(`Authenticode signing must run before ${name}`)
  }
  for (const variable of [
    'OPEN_COWORK_CODESIGN_PFX_BASE64',
    'OPEN_COWORK_CODESIGN_PASSWORD',
    'OPEN_COWORK_CODESIGN_THUMBPRINT',
  ]) {
    if (!text.includes(`${variable}: \${{ secrets.${variable} }}`)) {
      errors.push(`release workflow is missing secret-backed ${variable}`)
    }
  }
  if (/sign-windows-installer\.ps1[^\r\n]*(?:Password|Pfx)/i.test(text)) {
    errors.push('code-signing secret material must not be passed on the command line')
  }
  if (/OPEN_COWORK_AUTHENTICODE_TEST_MODE|TestAllowUntrustedCertificate|TestSkipTimestamp/i.test(text)) {
    errors.push('Authenticode test bypasses are forbidden in the release workflow')
  }
  return errors
}

function validateWorkflowHardening(appRoot, policy) {
  const workflowRoot = resolve(appRoot, '..', '.github', 'workflows')
  const workflowFiles = readdirSync(workflowRoot)
    .filter((name) => /\.ya?ml$/i.test(name))
    .sort()
  const errors = []
  let combined = ''
  for (const name of workflowFiles) {
    const text = readFileSync(join(workflowRoot, name), 'utf8')
    combined += `\n${text}`
    errors.push(...workflowHardeningErrors(text).map((error) => `${name}: ${error}`))
    if (name === 'windows-installer.yml' || name === 'windows-installer.yaml') {
      errors.push(...releaseWorkflowSigningErrors(text).map((error) => `${name}: ${error}`))
    }
  }
  if (!combined.includes(`cargo install cargo-audit --locked --version ${policy.cargoAuditVersion}`)) {
    errors.push(`cargo-audit is not installed at policy version ${policy.cargoAuditVersion}`)
  }
  if (!combined.includes('npm audit --audit-level=high')) errors.push('blocking high-severity npm audit is missing')
  for (const required of ['supply-chain:sbom', 'supply-chain:release', 'attest-build-provenance', 'attest-sbom']) {
    if (!combined.includes(required)) errors.push(`release workflow is missing ${required}`)
  }
  if (errors.length > 0) throw new Error(`Workflow supply-chain policy failed:\n${errors.join('\n')}`)
}

function deterministicUuid(seed) {
  const value = sha256(seed).slice(0, 32).split('')
  value[12] = '5'
  value[16] = ((Number.parseInt(value[16], 16) & 0x3) | 0x8).toString(16)
  const hex = value.join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

function commitContext(appRoot) {
  const repoRoot = resolve(appRoot, '..')
  const commit = process.env.GITHUB_SHA || optionalCommand('git', ['rev-parse', 'HEAD'], repoRoot, 'unknown')
  const timestamp = process.env.SOURCE_DATE_EPOCH
    ? new Date(Number(process.env.SOURCE_DATE_EPOCH) * 1000).toISOString()
    : optionalCommand('git', ['show', '-s', '--format=%cI', commit], repoRoot, '1970-01-01T00:00:00.000Z')
  return { commit, timestamp }
}

export function collectInventory(appRoot = defaultAppRoot, options = {}) {
  const policy = options.policy ?? readJson(join(appRoot, 'supply-chain-policy.json'))
  const packageJson = readJson(join(appRoot, 'package.json'))
  const packageLockPath = join(appRoot, 'package-lock.json')
  const cargoLockPath = join(appRoot, 'src-tauri', 'Cargo.lock')
  const packageLock = readJson(packageLockPath)
  const target = options.target ?? process.env.TAURI_BUILD_TARGET ?? 'x86_64-pc-windows-msvc'
  const metadata = options.cargoMetadata ?? JSON.parse(command('cargo', [
    'metadata', '--locked', '--format-version', '1', '--filter-platform', target,
  ], join(appRoot, 'src-tauri')))
  const cargoChecksums = parseCargoChecksums(readFileSync(cargoLockPath, 'utf8'))
  const reachable = reachableCargoIds(metadata)
  const rustTauri = metadata.packages.find((pkg) => reachable.has(pkg.id) && pkg.name === 'tauri')?.version
  const npmApi = packageLock.packages?.['node_modules/@tauri-apps/api']?.version
  const npmCli = packageLock.packages?.['node_modules/@tauri-apps/cli']?.version
  if (rustTauri || npmApi || npmCli) {
    if (!rustTauri || !npmApi || !npmCli) throw new Error('Tauri Rust, API, and CLI versions must all be present')
    assertTauriVersionCompatibility({ rustTauri, npmApi, npmCli })
  }
  const declaredRustVersion = cargoRustVersion(readFileSync(join(appRoot, 'src-tauri', 'Cargo.toml'), 'utf8'))
  const incompatibleRustPackages = metadata.packages
    .filter((pkg) => reachable.has(pkg.id) && pkg.rust_version && compareNumericVersions(pkg.rust_version, declaredRustVersion) > 0)
    .map((pkg) => `${pkg.name}@${pkg.version} requires Rust ${pkg.rust_version}`)
  if (incompatibleRustPackages.length > 0) {
    throw new Error(`Cargo rust-version ${declaredRustVersion} is below dependency requirements:\n${incompatibleRustPackages.join('\n')}`)
  }
  const npmRecords = []
  const npmComponents = new Map()

  for (const [packagePath, entry] of Object.entries(packageLock.packages ?? {})) {
    if (!packagePath || !packagePath.includes('node_modules/') || !entry.version) continue
    const name = npmName(packagePath.replaceAll('\\', '/'), entry)
    const license = entry.license ?? ''
    npmRecords.push({ ecosystem: 'npm', name, version: entry.version, license })
    if (entry.dev) continue
    const ref = npmPurl(name, entry.version)
    if (!npmComponents.has(ref)) {
      npmComponents.set(ref, {
        type: 'library',
        'bom-ref': ref,
        group: name.startsWith('@') ? name.split('/')[0] : undefined,
        name: name.startsWith('@') ? name.split('/')[1] : name,
        version: entry.version,
        purl: ref,
        hashes: integrityHash(entry.integrity),
        licenses: [{ expression: normalizeLicenseExpression(license) }],
      })
    }
  }

  const packageById = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]))
  const cargoRecords = []
  const cargoComponents = new Map()
  for (const id of reachable) {
    const pkg = packageById.get(id)
    if (!pkg || !pkg.source) continue
    const license = pkg.license ?? ''
    cargoRecords.push({ ecosystem: 'cargo', name: pkg.name, version: pkg.version, license })
    const ref = cargoPurl(pkg.name, pkg.version)
    const checksum = cargoChecksums.get(`${pkg.name}\0${pkg.version}\0${pkg.source}`)
    cargoComponents.set(id, {
      type: 'library',
      'bom-ref': ref,
      name: pkg.name,
      version: pkg.version,
      purl: ref,
      hashes: checksum ? [{ alg: 'SHA-256', content: checksum }] : undefined,
      licenses: [{ expression: normalizeLicenseExpression(license) }],
      externalReferences: externalReferences(pkg),
    })
  }

  validateLicenses([...npmRecords, ...cargoRecords], policy)
  const components = [...npmComponents.values(), ...cargoComponents.values()]
    .map((component) => Object.fromEntries(Object.entries(component).filter(([, value]) => value !== undefined)))
    .sort((left, right) => left['bom-ref'].localeCompare(right['bom-ref']))
  const cargoRefById = new Map([...cargoComponents].map(([id, component]) => [id, component['bom-ref']]))
  const dependencyMap = new Map()
  for (const node of metadata.resolve?.nodes ?? []) {
    const ref = cargoRefById.get(node.id)
    if (!ref) continue
    dependencyMap.set(ref, (node.deps ?? []).map((dependency) => cargoRefById.get(dependency.pkg)).filter(Boolean).sort())
  }

  const rootDependencies = []
  for (const name of Object.keys(packageJson.dependencies ?? {})) {
    const topLevel = packageLock.packages?.[`node_modules/${name}`]
    if (topLevel?.version) rootDependencies.push(npmPurl(name, topLevel.version))
  }
  const rootNode = (metadata.resolve?.nodes ?? []).find((node) => node.id === metadata.resolve?.root)
  for (const dependency of rootNode?.deps ?? []) {
    const ref = cargoRefById.get(dependency.pkg)
    if (ref) rootDependencies.push(ref)
  }

  return {
    packageJson,
    policy,
    target,
    components,
    records: [...npmRecords, ...cargoRecords],
    dependencies: [
      { ref: `pkg:generic/open-cowork@${packageJson.version}`, dependsOn: [...new Set(rootDependencies)].sort() },
      ...[...dependencyMap].map(([ref, dependsOn]) => ({ ref, dependsOn })).sort((a, b) => a.ref.localeCompare(b.ref)),
    ],
    materialSeed: Buffer.concat([readFileSync(packageLockPath), readFileSync(cargoLockPath)]),
  }
}

export function createSbom(appRoot = defaultAppRoot, options = {}) {
  const version = releaseVersion(appRoot, options.releaseTag ?? '')
  const inventory = options.inventory ?? collectInventory(appRoot, options)
  const context = options.context ?? commitContext(appRoot)
  return {
    bomFormat: 'CycloneDX',
    specVersion: '1.6',
    serialNumber: `urn:uuid:${deterministicUuid(inventory.materialSeed)}`,
    version: 1,
    metadata: {
      timestamp: context.timestamp,
      tools: { components: [{ type: 'application', name: 'open-cowork-supply-chain', version: '1' }] },
      component: {
        type: 'application',
        'bom-ref': `pkg:generic/open-cowork@${version}`,
        name: 'open-cowork',
        version,
        purl: `pkg:generic/open-cowork@${version}`,
      },
      properties: [
        { name: 'open-cowork:git-commit', value: context.commit },
        { name: 'open-cowork:target', value: inventory.target },
      ],
    },
    components: inventory.components,
    dependencies: inventory.dependencies,
  }
}

function notices(inventory, version, context) {
  const packages = inventory.records
    .map((record) => ({
      ecosystem: record.ecosystem,
      name: record.name,
      version: record.version,
      license: normalizeLicenseExpression(record.license),
    }))
    .sort((left, right) => `${left.ecosystem}:${left.name}@${left.version}`.localeCompare(`${right.ecosystem}:${right.name}@${right.version}`))
  return { schemaVersion: 1, product: 'open-cowork', version, commit: context.commit, generatedAt: context.timestamp, packages }
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, 'utf8')
}

function outputDirectory(value, appRoot) {
  const target = resolve(appRoot, value)
  mkdirSync(target, { recursive: true })
  return target
}

function assertRegularAsset(path, root) {
  const stats = lstatSync(path)
  if (!stats.isFile() || stats.isSymbolicLink()) throw new Error(`Release asset is not a regular file: ${path}`)
  const canonical = realpathSync(path)
  const canonicalRoot = `${realpathSync(root)}${sep}`
  if (!canonical.startsWith(canonicalRoot)) throw new Error(`Release asset escaped output directory: ${path}`)
}

export function writeSbomArtifacts(appRoot, targetDir, options = {}) {
  const version = releaseVersion(appRoot, options.releaseTag ?? '')
  const inventory = collectInventory(appRoot, options)
  const context = options.context ?? commitContext(appRoot)
  const sbom = createSbom(appRoot, { ...options, context, inventory })
  writeJson(join(targetDir, 'open-cowork.cdx.json'), sbom)
  writeJson(join(targetDir, 'THIRD_PARTY_NOTICES.json'), notices(inventory, version, context))
  return { inventory, context, version }
}

export function writeReleaseProvenance(appRoot, assetsDir, options = {}) {
  const releaseTag = options.releaseTag ?? process.env.RELEASE_TAG ?? ''
  const version = releaseVersion(appRoot, releaseTag)
  const context = options.context ?? commitContext(appRoot)
  const excluded = new Set(['release-provenance.json', 'SHA256SUMS'])
  const assetFiles = readdirSync(assetsDir).filter((name) => !excluded.has(name)).sort()
  if (assetFiles.length === 0) throw new Error('Release asset directory is empty')
  const assets = assetFiles.map((name) => {
    const path = join(assetsDir, name)
    assertRegularAsset(path, assetsDir)
    return { name, ...fileHash(path) }
  })
  const materialNames = ['package.json', 'package-lock.json', 'supply-chain-policy.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json']
  const materials = materialNames.map((name) => ({ path: name.replaceAll('\\', '/'), ...fileHash(join(appRoot, name)) }))
  const provenance = {
    schemaVersion: 1,
    predicateType: 'https://open-cowork.dev/provenance/release/v1',
    subject: assets,
    build: {
      product: 'open-cowork',
      version,
      tag: releaseTag || null,
      commit: context.commit,
      createdAt: context.timestamp,
      target: process.env.TAURI_BUILD_TARGET ?? 'x86_64-pc-windows-msvc',
      runner: process.env.GITHUB_ACTIONS === 'true' ? 'github-actions' : 'local',
      node: process.version,
      cargo: optionalCommand('cargo', ['--version'], appRoot, 'unavailable'),
      rustc: optionalCommand('rustc', ['--version'], appRoot, 'unavailable'),
    },
    materials,
  }
  writeJson(join(assetsDir, 'release-provenance.json'), provenance)

  const checksumFiles = readdirSync(assetsDir).filter((name) => name !== 'SHA256SUMS').sort()
  const checksumLines = checksumFiles.map((name) => {
    const path = join(assetsDir, name)
    assertRegularAsset(path, assetsDir)
    return `${fileHash(path).sha256}  ${name}`
  })
  writeFileSync(join(assetsDir, 'SHA256SUMS'), `${checksumLines.join('\n')}\n`, 'utf8')
  return provenance
}

function argumentValue(args, name, fallback) {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : fallback
}

function main() {
  const [operation = 'check', ...args] = process.argv.slice(2)
  const releaseTag = argumentValue(args, '--tag', process.env.RELEASE_TAG ?? '')
  if (operation === 'check') {
    const version = releaseVersion(defaultAppRoot, releaseTag)
    const inventory = collectInventory(defaultAppRoot)
    validateWorkflowHardening(defaultAppRoot, inventory.policy)
    const npmCount = inventory.records.filter((entry) => entry.ecosystem === 'npm').length
    const cargoCount = inventory.records.filter((entry) => entry.ecosystem === 'cargo').length
    console.log(`Supply-chain policy passed for Open Cowork ${version}: ${npmCount} npm and ${cargoCount} Cargo package records.`)
    return
  }

  const output = outputDirectory(argumentValue(args, '--output', '../release-assets'), defaultAppRoot)
  if (operation === 'sbom') {
    const result = writeSbomArtifacts(defaultAppRoot, output, { releaseTag })
    console.log(`Wrote CycloneDX SBOM and third-party notices for ${result.inventory.components.length} release components.`)
    return
  }
  if (operation === 'release') {
    writeReleaseProvenance(defaultAppRoot, output, { releaseTag })
    console.log('Wrote release provenance and SHA256SUMS.')
    return
  }
  throw new Error(`Unknown supply-chain operation: ${operation}`)
}

if (resolve(process.argv[1] ?? '') === scriptPath) {
  try {
    main()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
