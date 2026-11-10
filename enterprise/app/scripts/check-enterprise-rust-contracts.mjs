import { execFileSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

const app = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = resolve(app, '..')
const fixtures = JSON.parse(execFileSync('cargo', ['run', '--quiet', '-p', 'cowork-contracts', '--example', 'enterprise-contracts'], { cwd: root, encoding: 'utf8' }))
const vite = await createServer({ root: app, logLevel: 'silent', appType: 'custom', server: { middlewareMode: true } })
try {
  const { enterpriseContractRegistry, environmentSchema } = await vite.ssrLoadModule('/src/runtime/enterpriseContracts.ts')
  const { authTokensSchema } = await vite.ssrLoadModule('/src/runtime/authContracts.ts')
  const registry = { ...enterpriseContractRegistry, AuthTokens: authTokensSchema }
  for (const [name, value] of Object.entries(fixtures)) {
    if (name === 'EnvironmentBackends') { for (const backend of value) environmentSchema.shape.backend.parse(backend) }
    else registry[name].parse(value)
  }
  console.log('enterprise_rust_typescript_wire_compatibility=ok')
} finally { await vite.close() }
