import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

function argumentValue(args, name) {
  const index = args.indexOf(name)
  if (index < 0 || !args[index + 1]) throw new Error(`Missing required argument ${name}`)
  return args[index + 1]
}

export function createUpdaterManifest({
  tag,
  artifactPath,
  signaturePath = `${artifactPath}.sig`,
  repository,
  publishedAt = new Date().toISOString(),
  notes,
}) {
  const version = tag.trim().replace(/^v/, '')
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid semantic release tag: ${tag}`)
  }
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`Invalid GitHub repository: ${repository}`)
  }
  if (!existsSync(artifactPath)) throw new Error(`Updater artifact not found: ${artifactPath}`)
  if (!existsSync(signaturePath)) throw new Error(`Updater signature not found: ${signaturePath}`)

  const signature = readFileSync(signaturePath, 'utf8').trim()
  if (!signature) throw new Error('Updater signature is empty')
  const parsedDate = new Date(publishedAt)
  if (Number.isNaN(parsedDate.getTime())) throw new Error(`Invalid publication date: ${publishedAt}`)

  const artifactName = basename(artifactPath)
  const encodedTag = encodeURIComponent(tag.trim())
  const encodedArtifact = encodeURIComponent(artifactName)
  return {
    version,
    notes: notes ?? `LocalAI Cowork ${tag.trim()}`,
    pub_date: parsedDate.toISOString(),
    platforms: {
      'windows-x86_64': {
        signature,
        url: `https://github.com/${repository}/releases/download/${encodedTag}/${encodedArtifact}`,
      },
    },
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
  const artifactPath = resolve(argumentValue(args, '--artifact'))
  const outputPath = resolve(argumentValue(args, '--output'))
  writeUpdaterManifest({
    tag: argumentValue(args, '--tag'),
    artifactPath,
    signaturePath: `${artifactPath}.sig`,
    repository: argumentValue(args, '--repository'),
    publishedAt: process.env.SOURCE_DATE_EPOCH
      ? new Date(Number(process.env.SOURCE_DATE_EPOCH) * 1000).toISOString()
      : new Date().toISOString(),
  }, outputPath)
  console.log(`Wrote signed updater manifest: ${outputPath}`)
}
