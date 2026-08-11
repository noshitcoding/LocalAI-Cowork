import { readFile, mkdir, writeFile } from 'node:fs/promises'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { z } from 'zod'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = resolve(appRoot, '..')
const generatedRoot = join(repoRoot, 'contracts', 'generated')
const baselinePath = join(generatedRoot, 'v1', 'contracts.schema.json')
const check = process.argv.includes('--check')

function stable(value) {
  if (Array.isArray(value)) return value.map(stable)
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, stable(value[key])]))
  }
  return value
}

function json(value) { return `${JSON.stringify(stable(value), null, 2)}\n` }

function routeBlocks(source) {
  const blocks = []
  let cursor = 0
  while ((cursor = source.indexOf('.route(', cursor)) !== -1) {
    const start = cursor
    let index = cursor + '.route('.length
    let depth = 1
    let quote = null
    let escaped = false
    for (; index < source.length && depth > 0; index++) {
      const character = source[index]
      if (quote) {
        if (escaped) escaped = false
        else if (character === '\\') escaped = true
        else if (character === quote) quote = null
      } else if (character === '"' || character === "'") quote = character
      else if (character === '(') depth++
      else if (character === ')') depth--
    }
    if (depth !== 0) throw new Error(`Unbalanced Axum route at offset ${start}`)
    blocks.push({ start, text: source.slice(start, index) })
    cursor = index
  }
  return blocks
}

function extractRoutes(source) {
  const agentStart = source.indexOf('let agent = Router::new()')
  const publicStart = source.indexOf('let public_auth = Router::new()')
  const appStart = source.indexOf('let app = Router::new()')
  if ([agentStart, publicStart, appStart].some((offset) => offset < 0)) throw new Error('Router group markers are missing')
  return routeBlocks(source).flatMap(({ start, text }) => {
    const path = text.match(/"(\/[^"\n]+)"/)?.[1]
    if (!path) return []
    const methods = ['get', 'post', 'put', 'delete'].filter((method) => new RegExp(`\\b${method}\\s*\\(`).test(text))
    if (methods.length === 0) throw new Error(`No HTTP method found for ${path}`)
    const group = start >= appStart ? 'root' : start >= publicStart ? 'public' : start >= agentStart ? 'agent' : 'protected'
    const fullPath = group === 'root' ? path : `/api/v1${path}`
    return methods.map((method) => ({ path: fullPath, method, group }))
  })
}

function operationId(method, path) {
  const words = path.replace(/^\/api\/v1\/?/, '').replace(/[{}]/g, '').split(/[^A-Za-z0-9]+/).filter(Boolean)
  return `${method}${words.map((word) => word[0].toUpperCase() + word.slice(1)).join('') || 'Root'}`
}

const responseSchemas = new Map([
  ['get /api/v1/version', 'VersionResponse'], ['get /api/v1/capabilities', 'CapabilityCatalog'],
  ['get /api/v1/runs', 'ListRunsResponse'], ['post /api/v1/runs', 'RunRecord'],
  ['get /api/v1/runs/{run_id}', 'RunRecord'], ['post /api/v1/runs/{run_id}/cancel', 'RunRecord'],
  ['get /api/v1/threads/{thread_id}/messages', 'MessageRecord'],
  ['post /api/v1/threads/{thread_id}/messages', 'ThreadMessageRun'],
  ['get /api/v1/sync/changes', 'PullSyncChangesResponse'],
  ['post /api/v1/sync/changes', 'PushSyncChangesResponse'],
  ['get /api/v1/sync/entities/{entity_type}', 'SyncedEntityPage'],
  ['get /api/v1/agent/executors/{executor_id}/sync/changes', 'PullSyncChangesResponse'],
  ['post /api/v1/agent/executors/{executor_id}/sync/changes', 'PushSyncChangesResponse'],
  ['get /api/v1/agent/executors/{executor_id}/sync/entities/{entity_type}', 'SyncedEntityPage'],
  ['get /api/v1/projects', 'ProjectRecord'], ['post /api/v1/projects', 'ProjectRecord'],
  ['get /api/v1/teams', 'TeamRecord'], ['post /api/v1/teams', 'TeamRecord'],
  ['get /api/v1/teams/{team_id}/members', 'TeamMemberRecord'],
  ['get /api/v1/provider-profiles', 'ProviderProfile'], ['post /api/v1/provider-profiles', 'ProviderProfile'],
  ['put /api/v1/provider-profiles/{profile_id}', 'ProviderProfile'],
  ['put /api/v1/provider-profiles/{profile_id}/secret', 'ProviderProfile'],
  ['get /api/v1/projects/{project_id}', 'ProjectRecord'], ['put /api/v1/projects/{project_id}', 'ProjectRecord'],
  ['put /api/v1/threads/{thread_id}', 'ThreadRecord'],
  ['get /api/v1/projects/{project_id}/threads', 'ThreadRecord'], ['get /api/v1/tasks', 'TaskDefinition'],
  ['post /api/v1/tasks', 'TaskDefinition'], ['get /api/v1/tasks/{task_id}', 'TaskDefinition'],
  ['get /api/v1/schedules', 'ScheduleRecord'], ['post /api/v1/schedules', 'ScheduleRecord'],
  ['put /api/v1/schedules/{schedule_id}', 'ScheduleRecord'],
  ['get /api/v1/operations/metrics', 'OperationsSnapshot'],
  ['get /api/v1/auth/sessions', 'AuthSessionRecord'],
  ['get /api/v1/support-grants', 'SupportGrantRecord'], ['post /api/v1/support-grants', 'SupportGrantRecord'],
  ['get /api/v1/runs/{run_id}/artifacts', 'RunArtifact'],
  ['get /api/v1/runs/{run_id}/approvals', 'ApprovalRequest'],
  ['get /api/v1/runs/{run_id}/input-requests', 'RunInputRequest'],
  ['get /api/v1/runs/{run_id}/checkpoints', 'RunCheckpoint'],
  ['get /api/v1/runs/{run_id}/desktop-sessions', 'DesktopSession'],
  ['post /api/v1/runs/{run_id}/desktop-sessions', 'DesktopSession'],
  ['post /api/v1/runs/{run_id}/terminal-sessions', 'TerminalSessionTicket'],
  ['get /api/v1/snapshots/{manifest_id}', 'SnapshotManifest'],
  ['post /api/v1/snapshots/{manifest_id}/commit', 'SnapshotManifest'],
  ['get /api/v1/snapshots/{manifest_id}/upload', 'SnapshotUploadSession'],
  ['get /api/v1/projects/{project_id}/versions', 'ProjectVersion'],
  ['post /api/v1/projects/{project_id}/versions', 'ProjectVersion'],
])
const arrayResponses = new Set([
  'get /api/v1/projects', 'get /api/v1/teams', 'get /api/v1/teams/{team_id}/members',
  'get /api/v1/provider-profiles', 'get /api/v1/tasks', 'get /api/v1/schedules',
  'get /api/v1/projects/{project_id}/threads',
  'get /api/v1/threads/{thread_id}/messages',
  'get /api/v1/auth/sessions',
  'get /api/v1/support-grants', 'get /api/v1/runs/{run_id}/artifacts',
  'get /api/v1/runs/{run_id}/approvals', 'get /api/v1/runs/{run_id}/input-requests',
  'get /api/v1/runs/{run_id}/checkpoints', 'get /api/v1/runs/{run_id}/desktop-sessions',
  'get /api/v1/projects/{project_id}/versions',
])

function schemaRef(name, array = false) {
  const ref = { $ref: `#/components/schemas/${name}` }
  return array ? { type: 'array', items: ref } : ref
}

function buildOpenApi(routes, schemas) {
  const paths = {}
  for (const route of routes) {
    paths[route.path] ??= {}
    const key = `${route.method} ${route.path}`
    const schemaName = responseSchemas.get(key)
    const response = schemaName
      ? { description: 'Successful response', content: { 'application/json': { schema: schemaRef(schemaName, arrayResponses.has(key)) } } }
      : { description: 'Successful response' }
    const parameters = [...route.path.matchAll(/{([^}]+)}/g)].map((match) => ({
      name: match[1], in: 'path', required: true,
      schema: match[1] === 'entity_type'
        ? { type: 'string' }
        : { type: 'string', format: 'uuid' },
    }))
    paths[route.path][route.method] = {
      operationId: operationId(route.method, route.path),
      tags: [route.group],
      ...(parameters.length ? { parameters } : {}),
      ...(route.group === 'protected' ? { security: [{ bearerAuth: [] }] }
        : route.group === 'agent' ? { security: [{ executorBearerAuth: [] }] } : {}),
      responses: { '200': response, '400': { $ref: '#/components/responses/Error' }, '401': { $ref: '#/components/responses/Error' } },
    }
  }
  return {
    openapi: '3.1.0',
    info: { title: 'Open Cowork Control Plane', version: '0.3.0', description: 'Generated from the Axum route graph and versioned runtime contracts.' },
    servers: [{ url: '/' }],
    paths,
    components: {
      securitySchemes: {
        bearerAuth: { type: 'http', scheme: 'bearer', bearerFormat: 'opaque' },
        executorBearerAuth: { type: 'http', scheme: 'bearer', bearerFormat: 'opaque executor credential' },
      },
      responses: { Error: { description: 'Structured API error', content: { 'application/json': { schema: { $ref: '#/components/schemas/ErrorResponse' } } } } },
      schemas: {
        ...schemas,
        ErrorResponse: { type: 'object', required: ['error', 'message', 'details'], properties: { error: { type: 'string' }, message: { type: 'string' }, details: { type: 'object' } }, additionalProperties: false },
      },
    },
  }
}

function assertCompatible(previous, current, location = '$') {
  if (!previous || typeof previous !== 'object') return
  if (!current || typeof current !== 'object') throw new Error(`${location} was removed`)
  if (previous.type && current.type && JSON.stringify(previous.type) !== JSON.stringify(current.type)) throw new Error(`${location} changed type`)
  if (Array.isArray(previous.enum)) {
    const currentValues = new Set(current.enum ?? [])
    for (const value of previous.enum) if (!currentValues.has(value)) throw new Error(`${location} removed enum value ${JSON.stringify(value)}`)
  }
  for (const required of previous.required ?? []) {
    if (!(current.required ?? []).includes(required)) throw new Error(`${location}.${required} is no longer required`)
  }
  const addedRequired = (current.required ?? []).filter((name) => !(previous.required ?? []).includes(name))
  if (addedRequired.length) throw new Error(`${location} added required properties: ${addedRequired.join(', ')}`)
  for (const [name, schema] of Object.entries(previous.properties ?? {})) {
    if (!(name in (current.properties ?? {}))) throw new Error(`${location}.${name} was removed`)
    assertCompatible(schema, current.properties[name], `${location}.${name}`)
  }
  if (previous.items) assertCompatible(previous.items, current.items, `${location}[]`)
  for (const keyword of ['anyOf', 'oneOf', 'allOf']) {
    if (previous[keyword]) {
      if (!Array.isArray(current[keyword]) || current[keyword].length < previous[keyword].length) throw new Error(`${location}.${keyword} was narrowed`)
      previous[keyword].forEach((entry, index) => assertCompatible(entry, current[keyword][index], `${location}.${keyword}[${index}]`))
    }
  }
}

function validateOpenApi(document) {
  const operationIds = new Set()
  const visit = (value, location = '#', inheritedResource = document) => {
    if (Array.isArray(value)) return value.forEach((entry, index) => visit(entry, `${location}/${index}`, inheritedResource))
    if (!value || typeof value !== 'object') return
    const resource = typeof value.$id === 'string' ? value : inheritedResource
    if (typeof value.$ref === 'string' && value.$ref.startsWith('#/')) {
      const resolved = value.$ref.slice(2).split('/').reduce((current, part) => current?.[part.replaceAll('~1', '/').replaceAll('~0', '~')], resource)
      if (resolved === undefined) throw new Error(`${location} has unresolved reference ${value.$ref}`)
    }
    if (typeof value.operationId === 'string') {
      if (operationIds.has(value.operationId)) throw new Error(`Duplicate operationId ${value.operationId}`)
      operationIds.add(value.operationId)
    }
    for (const [key, entry] of Object.entries(value)) visit(entry, `${location}/${key}`, resource)
  }
  visit(document)
  if (!document.paths['/api/v1/openapi.json'] || !document.paths['/api/v1/schemas/contracts.json']) {
    throw new Error('Contract artifact routes are absent from generated OpenAPI')
  }
}

function validateCompatibilityGuard() {
  const previous = { type: 'object', required: ['id'], properties: { id: { type: 'string' }, state: { enum: ['a', 'b'] } } }
  assertCompatible(previous, { ...previous, properties: { ...previous.properties, optional: { type: 'string' } } }, '$selftest')
  for (const breaking of [
    { type: 'object', required: ['id'], properties: { id: { type: 'number' }, state: { enum: ['a', 'b'] } } },
    { type: 'object', required: ['id', 'new'], properties: { ...previous.properties, new: { type: 'string' } } },
    { type: 'object', required: ['id'], properties: { id: { type: 'string' }, state: { enum: ['a'] } } },
  ]) {
    let rejected = false
    try { assertCompatible(previous, breaking, '$selftest') } catch { rejected = true }
    if (!rejected) throw new Error('N-1 compatibility guard accepted a synthetic breaking change')
  }
}

async function main() {
  const vite = await createServer({ root: appRoot, logLevel: 'silent', appType: 'custom', server: { middlewareMode: true } })
  let registry
  let protocolVersion
  let minimumCompatibleVersion
  try {
    ({
      contractSchemaRegistry: registry,
      protocolSchemaVersion: protocolVersion,
      minimumCompatibleSchemaVersion: minimumCompatibleVersion,
    } = await vite.ssrLoadModule('/src/runtime/contractArtifactRegistry.ts'))
  }
  finally { await vite.close() }
  if (!Number.isInteger(protocolVersion) || protocolVersion < 1 || minimumCompatibleVersion !== protocolVersion - 1) {
    throw new Error(`Protocol must advertise an exact N-1 window; got ${minimumCompatibleVersion}..=${protocolVersion}`)
  }
  const outputRoot = join(generatedRoot, `v${protocolVersion}`)
  const schemas = {}
  for (const name of Object.keys(registry).sort()) {
    const generated = z.toJSONSchema(registry[name], { target: 'draft-2020-12', unrepresentable: 'any', reused: 'ref', cycles: 'ref' })
    delete generated.$schema
    generated.$id = `https://schemas.open-cowork.invalid/v${protocolVersion}/${name}.schema.json`
    generated.title = name
    schemas[name] = generated
  }
  const bundle = { $schema: 'https://json-schema.org/draft/2020-12/schema', $id: `https://schemas.open-cowork.invalid/v${protocolVersion}/contracts.schema.json`, title: `Open Cowork protocol v${protocolVersion} contracts`, $defs: schemas }
  const rustSource = await readFile(join(repoRoot, 'server', 'cowork-server', 'src', 'main.rs'), 'utf8')
  const routes = extractRoutes(rustSource)
  const unique = new Set(routes.map(({ method, path }) => `${method} ${path}`))
  if (unique.size !== routes.length) throw new Error('Duplicate HTTP method/path found in Axum router')
  const openapi = buildOpenApi(routes, schemas)
  validateOpenApi(openapi)
  validateCompatibilityGuard()
  const outputs = new Map([
    [join(outputRoot, 'contracts.schema.json'), json(bundle)],
    [join(outputRoot, 'openapi.json'), json(openapi)],
  ])
  if (check) {
    for (const [path, expected] of outputs) {
      if (!existsSync(path) || readFileSync(path, 'utf8') !== expected) throw new Error(`${relative(repoRoot, path)} is stale; run npm run contracts:generate`)
    }
  } else {
    for (const [path, value] of outputs) { await mkdir(dirname(path), { recursive: true }); await writeFile(path, value) }
  }
  if (existsSync(baselinePath)) {
    const previous = JSON.parse(await readFile(baselinePath, 'utf8'))
    for (const [name, schema] of Object.entries(previous.$defs ?? {})) {
      if (!bundle.$defs[name]) throw new Error(`N-1 contract ${name} was removed`)
      assertCompatible(schema, bundle.$defs[name], `$defs.${name}`)
    }
  } else throw new Error(`${relative(repoRoot, baselinePath)} is missing`)
  console.log(`contract_schemas=${Object.keys(schemas).length}`)
  console.log(`openapi_operations=${routes.length}`)
  console.log(`protocol_window=${minimumCompatibleVersion}..=${protocolVersion}`)
  console.log('n_minus_one_compatibility=ok')
}

await main()
