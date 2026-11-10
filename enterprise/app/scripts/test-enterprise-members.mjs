import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Live membership acceptance uses only the previously qualified rollout fixture.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomBytes, randomUUID } from 'node:crypto'
import http from 'node:http'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const fixtureFile = new URL('instance-members-fixture.json', privateDir)
const management = 'http://localhost:18082'
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
const rollout = JSON.parse(await readFile(new URL('rollout-fixture.json', privateDir), 'utf8'))
const target = rollout.instances[1], org = rollout.organisation_id
assert.equal(target.id, '9f1b3ba4-ca1b-4999-a428-d23b5de9f4a2')
assert.equal(target.name, 'Rollout fixture 2')
assert.equal((await admin('/management/v1/organisations')).items.find(row => row.id === org)?.name, rollout.name)
const base = `/management/v1/organisations/${org}/instances`, path = `${base}/${target.id}`
const instance = (await admin(base)).find(row => row.id === target.id)
assert.equal(instance?.state, 'ready')
assert.ok(!(await admin(`${path}/operations`)).some(row => ['queued', 'running', 'reconciling'].includes(row.state)))
const launch = async identity => {
  const ticket = await request(management, `${path}/launch`, identity, {})
  return { origin: ticket.url, ...(await request(ticket.url, '/api/v1/auth/fleet', null, { ticket: ticket.ticket, device_id: randomUUID() })) }
}
const owner = await launch(token)
let fixture
try { fixture = JSON.parse(await readFile(fixtureFile, 'utf8')) } catch (error) { if (error.code !== 'ENOENT') throw error }
if (!fixture) {
  fixture = { organisation_id: org, instance_id: target.id, email: `reader-test-${randomUUID()}@cowork.local`, password: randomBytes(30).toString('base64url') }
  await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
}
assert.equal(fixture.instance_id, target.id)
if (!fixture.user_id) {
  const invitation = await admin('/api/v1/auth/invitations', { email: fixture.email }, 201)
  fixture.user_id = (await request(management, '/api/v1/auth/invitations/accept', null, { token: invitation.token, display_name: 'Instanz-Lesetest', password: fixture.password, device_id: randomUUID() }, 201)).user_id
  await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
}
await admin(`/management/v1/organisations/${org}/members`, { email: fixture.email, role: 'member', content_admin: false })
const member = (await request(management, '/api/v1/auth/login', null, { email: fixture.email, password: fixture.password, device_id: randomUUID() })).access_token
assert.ok((await admin(`${path}/members/candidates`)).some(row => row.user_id === fixture.user_id))
await admin(`${path}/members`, { user_id: fixture.user_id, role: 'member' })
let session = await launch(member)
if (!fixture.project_id) {
  fixture.project_id = (await request(session.origin, '/api/v1/projects', session.access_token, { name: 'Membership acceptance project', privacy: 'private_local' }, 201)).id
  await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
}
await request(owner.origin, `/api/v1/projects/${target.project_id}/members`, owner.access_token, { user_id: fixture.user_id, role: 'editor' }, 204)
await admin(`${path}/members`, { user_id: fixture.user_id, role: 'viewer' })
session = await launch(member)
const info = await request(session.origin, '/api/v1/content-access', session.access_token)
assert.equal(info.instance_viewer, true); assert.equal(info.read_only, true)
await request(session.origin, `/api/v1/projects/${target.project_id}`, session.access_token)
await request(session.origin, `/api/v1/projects/${fixture.project_id}`, session.access_token)
await request(session.origin, '/api/v1/projects', session.access_token, { name: 'Must be denied', privacy: 'private_local' }, 403)
await request(session.origin, '/api/v1/executors', session.access_token, undefined, 403)
await request(management, `${path}/members`, member, { user_id: fixture.user_id, role: 'manager' }, 404)
await admin(`${path}/members/${fixture.user_id}`, undefined, 200, 'DELETE')
await request(session.origin, '/api/v1/projects', session.access_token, undefined, 401)
await request(management, `${path}/launch`, member, {}, 404)
assert.ok(!(await admin(`${path}/members`)).some(row => row.user_id === fixture.user_id))
const audit = await admin(`/management/v1/organisations/${org}/audit`)
assert.ok(audit.some(row => row.action === 'instance.member.remove' && row.payload.metadata.user_id === fixture.user_id))
const checks = ['scoped-member-directory', 'viewer-ceiling-over-local-editor-and-owner', 'no-device-or-role-escalation', 'revoked-session-and-new-login-denied', 'chained-membership-audit']
await writeFile(new URL('instance-members-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), instance_id: target.id, release: instance.current_release, user_id: fixture.user_id, checks }, null, 2))
for (const check of checks) console.log(`${check}=ok`)
