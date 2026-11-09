import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Live Docker qualification and scoped support access on an existing test fixture.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomBytes, randomUUID } from 'node:crypto'
import http from 'node:http'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const management = 'http://localhost:18082'
const fixtureFile = new URL('content-access-fixture.json', privateDir)
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const checks = [], operations = []
async function request(origin, route, token, body, expected = 200, method) {
  const url = new URL(route, origin)
  const response = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search, method: method ?? (body === undefined ? 'GET' : 'POST'), headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let data = ''; res.setEncoding('utf8'); res.on('data', part => { data += part }); res.on('end', () => resolve({ status: res.statusCode, data }))
    }); req.on('error', reject); req.setTimeout(20000, () => req.destroy(new Error('HTTP timeout'))); req.end(body === undefined ? undefined : JSON.stringify(body))
  })
  assert.equal(response.status, expected, `${route}: HTTP ${response.status}`)
  return response.data && expected < 400 ? JSON.parse(response.data) : null
}
const token = (await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const admin = (route, body, expected, method) => request(management, route, token, body, expected, method)
const rolloutFixture = JSON.parse(await readFile(new URL('rollout-fixture.json', privateDir), 'utf8'))
const target = rolloutFixture.instances[1]
assert.equal(target.id, '9f1b3ba4-ca1b-4999-a428-d23b5de9f4a2')
assert.equal(target.name, 'Rollout fixture 2')
const org = rolloutFixture.organisation_id
assert.match(rolloutFixture.name, /^Rollout-Abnahme /)
assert.equal((await admin('/management/v1/organisations')).items.find(value => value.id === org)?.name, rolloutFixture.name)
const base = `/management/v1/organisations/${org}/instances`
const path = `${base}/${target.id}`
const current = async () => {
  const instance = (await admin(base)).find(row => row.id === target.id)
  assert.equal(instance?.name, target.name)
  return instance
}
assert.ok(!(await admin(`${path}/operations`)).some(op => ['queued', 'running', 'reconciling'].includes(op.state)), 'Observe retained operations before rerunning')
assert.equal((await current()).rollout_id, null)
let fixture
try { fixture = JSON.parse(await readFile(fixtureFile, 'utf8')) } catch (error) { if (error.code !== 'ENOENT') throw error }
if (!fixture) {
  fixture = { organisation_id: org, instance_id: target.id, support_email: `support-test-${randomUUID()}@cowork.local`, support_password: randomBytes(30).toString('base64url') }
  await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
}
assert.equal(fixture.instance_id, target.id)
const save = () => writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
async function operate(action, extra = {}) {
  const op = await admin(`${path}/operations`, { action, expected_revision: (await current()).revision, idempotency_key: randomUUID(), ...extra })
  console.log(`${action}${extra.qualification_only ? '-canary' : ''}=${op.id}`)
  for (let i = 0; i < 1200; i++) {
    const row = (await admin(`${path}/operations`)).find(value => value.id === op.id)
    if (['succeeded', 'failed', 'reconciling'].includes(row.state)) { assert.equal(row.state, 'succeeded', JSON.stringify(row.result)); operations.push(row); return row }
    if (i > 0 && i % 60 === 0) console.log(`${action}: ${row.state}`)
    await delay(1000)
  }
  throw new Error(`Observe operation ${op.id}; do not replay`)
}
const release = JSON.parse(await readFile(new URL('content-release.json', privateDir), 'utf8'))
if ((await current()).current_release !== release.id) {
  if ((await current()).state === 'ready') await operate('stop')
  assert.equal((await current()).state, 'stopped')
  assert.ok(release.supported_sources.includes((await current()).current_release), 'Fixture source must be explicitly supported by the reviewed release')
  for (let i = 0; i < 60; i++) {
    if ((await admin(`${path}/releases`)).some(row => row.id === release.id)) break
    await delay(1000)
  }
  const canary = await operate('upgrade', { release: release.id, qualification_only: true })
  assert.equal(canary.result.rollback_verified, true)
  const promoted = await operate('upgrade', { release: release.id, qualification_only: false })
  assert.ok((await admin(`${path}/backups`)).some(backup => backup.id === promoted.id))
  checks.push('canary-source-restoration-and-backed-up-promotion')
}
if ((await current()).state === 'stopped') await operate('start')
assert.equal((await current()).state, 'ready')
const login = async access => {
  const launch = await request(management, `${path}/launch`, access, {})
  for (let i = 0; i < 30; i++) {
    try { await request(launch.url, '/readyz'); break } catch { await delay(1000) }
  }
  return { origin: launch.url, ...(await request(launch.url, '/api/v1/auth/fleet', null, { ticket: launch.ticket, device_id: randomUUID() })) }
}
const owner = await login(token)
assert.equal((await request(owner.origin, `/api/v1/projects/${target.project_id}`, owner.access_token)).name, 'Before grouped update')
const rule = await request(owner.origin, '/api/v1/content-access', owner.access_token)
assert.equal(rule.content_access, 'support_grant')
assert.equal(rule.read_only, false)
if (!fixture.support_user_id) {
  const invitation = await admin('/api/v1/auth/invitations', { email: fixture.support_email }, 201)
  const account = await request(management, '/api/v1/auth/invitations/accept', null, { token: invitation.token, display_name: 'Support-Abnahme', password: fixture.support_password, device_id: randomUUID() }, 201)
  fixture.support_user_id = account.user_id; await save()
}
await admin(`/management/v1/organisations/${org}/members`, { email: fixture.support_email, role: 'admin', content_admin: false })
const support = await request(management, '/api/v1/auth/login', null, { email: fixture.support_email, password: fixture.support_password, device_id: randomUUID() })
for (let i = 0; i < 45; i++) {
  if ((await request(management, base, support.access_token)).find(row => row.id === target.id)?.can_open_support) break
  await delay(1000)
}
const reader = await login(support.access_token)
assert.equal((await request(reader.origin, '/api/v1/content-access', reader.access_token)).read_only, true)
await request(reader.origin, `/api/v1/projects/${target.project_id}`, reader.access_token, undefined, 401)
const grant = await request(owner.origin, '/api/v1/support-grants', owner.access_token, { support_user_id: fixture.support_user_id, project_id: target.project_id, thread_id: null, reason: 'Docker support acceptance', expires_at: new Date(Date.now() + 3600000).toISOString() }, 201)
assert.equal((await request(reader.origin, `/api/v1/projects/${target.project_id}`, reader.access_token)).name, 'Before grouped update')
await request(reader.origin, '/api/v1/projects', reader.access_token, { name: 'Not allowed', privacy: 'private_local' }, 403)
await request(owner.origin, `/api/v1/support-grants/${grant.id}`, owner.access_token, undefined, 204, 'DELETE')
await request(reader.origin, `/api/v1/projects/${target.project_id}`, reader.access_token, undefined, 401)
checks.push('existing-project-preserved-scoped-read-only-grant-and-revocation')
const audit = await admin(`/management/v1/organisations/${org}/audit`)
assert.ok(audit.some(event => event.action === 'content.read_authorized' && event.payload.metadata.support_grant_id === grant.id))
checks.push('central-chained-audit')
await writeFile(new URL('content-access-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), organisation_id: org, instance_id: target.id, release: release.id, support_user_id: fixture.support_user_id, checks, operations }, null, 2))
for (const name of checks) console.log(`${name}=ok`)
