import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

function argumentValue(args, name) {
  const index = args.indexOf(name)
  if (index < 0 || !args[index + 1]) throw new Error(`Missing required argument ${name}`)
  return args[index + 1]
}

function argumentValues(args, name) {
  const values = []
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name && args[index + 1]) values.push(args[index + 1])
  }
  return values
}

function normalizePlatforms({ artifactPath, signaturePath, platforms }) {
  if (platforms) return platforms
  if (!artifactPath) throw new Error('At least one updater platform artifact is required')
  return {
    'windows-x86_64': {
      artifactPath,
      signaturePath: signaturePath ?? `${artifactPath}.sig`,
    },
  }
}

export function createUpdaterManifest({
  tag,
  artifactPath,
  signaturePath = `${artifactPath}.sig`,
  repository,
  publishedAt = new Date().toISOString(),
  notes,
  platforms,
}) {
  const version = tag.trim().replace(/^v/, '')
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid semantic release tag: ${tag}`)
  }
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`Invalid GitHub repository: ${repository}`)
  }
  const parsedDate = new Date(publishedAt)
  if (Number.isNaN(parsedDate.getTime())) throw new Error(`Invalid publication date: ${publishedAt}`)

  const encodedTag = encodeURIComponent(tag.trim())
  const platformEntries = Object.entries(normalizePlatforms({ artifactPath, signaturePath, platforms }))
  if (platformEntries.length === 0) throw new Error('At least one updater platform artifact is required')
  const manifestPlatforms = Object.fromEntries(platformEntries.map(([platform, paths]) => {
    if (!/^(windows|linux|darwin)-(x86_64|aarch64|i686|armv7)$/.test(platform)) {
      throw new Error(`Invalid updater platform: ${platform}`)
    }
    const resolvedArtifact = paths?.artifactPath
    const resolvedSignature = paths?.signaturePath ?? `${resolvedArtifact}.sig`
    if (!resolvedArtifact || !existsSync(resolvedArtifact)) {
      throw new Error(`Updater artifact not found: ${resolvedArtifact}`)
    }
    if (!existsSync(resolvedSignature)) {
      throw new Error(`Updater signature not found: ${resolvedSignature}`)
    }
    const signature = readFileSync(resolvedSignature, 'utf8').trim()
    if (!signature) throw new Error(`Updater signature is empty for ${platform}`)
    return [platform, {
      signature,
      url: `https://github.com/${repository}/releases/download/${encodedTag}/${encodeURIComponent(basename(resolvedArtifact))}`,
    }]
  }))
  return {
    version,
    notes: notes ?? `LocalAI Cowork ${tag.trim()}`,
    pub_date: parsedDate.toISOString(),
    platforms: manifestPlatforms,
  }
}

export function writeUpdaterManifest(options, outputPath) {
  const manifest = createUpdaterManifest(options)
  writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
  return manifest
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : ''
if (invokedPath === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2)
  const platformArguments = argumentValues(args, '--platform')
  const platforms = platformArguments.length > 0
    ? Object.fromEntries(platformArguments.map((value) => {
      const separator = value.indexOf('=')
      if (separator <= 0 || separator === value.length - 1) {
        throw new Error(`Invalid --platform value ${value}; expected PLATFORM=ARTIFACT`)
      }
      const platform = value.slice(0, separator)
      const artifact = resolve(value.slice(separator + 1))
      return [platform, { artifactPath: artifact, signaturePath: `${artifact}.sig` }]
    }))
    : undefined
  const artifactArgument = platformArguments.length === 0 ? argumentValue(args, '--artifact') : undefined
  const artifactPath = artifactArgument ? resolve(artifactArgument) : undefined
  const outputPath = resolve(argumentValue(args, '--output'))
  writeUpdaterManifest({
    tag: argumentValue(args, '--tag'),
    artifactPath,
    signaturePath: artifactPath ? `${artifactPath}.sig` : undefined,
    platforms,
    repository: argumentValue(args, '--repository'),
    publishedAt: process.env.SOURCE_DATE_EPOCH
      ? new Date(Number(process.env.SOURCE_DATE_EPOCH) * 1000).toISOString()
      : new Date().toISOString(),
  }, outputPath)
  console.log(`Wrote signed updater manifest: ${outputPath}`)
}
