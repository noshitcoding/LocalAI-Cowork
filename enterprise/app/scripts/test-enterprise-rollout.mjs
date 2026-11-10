import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Real Docker group update with dedicated fixtures, pause and management restart.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import http from 'node:http'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const root = fileURLToPath(new URL('../../', import.meta.url))
const fixtureFile = new URL('rollout-fixture.json', privateDir)
const management = 'http://localhost:18082'
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const checks = []
const pass = name => { checks.push(name); console.log(`${name}=ok`) }
async function request(origin, route, token, body, expected = 200) {
  const url = new URL(route, origin)
  const response = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search, method: body === undefined ? 'GET' : 'POST', headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let data = ''; res.setEncoding('utf8'); res.on('data', part => { data += part }); res.on('end', () => resolve({ status: res.statusCode, data }))
    }); req.on('error', reject); req.setTimeout(20000, () => req.destroy(new Error('HTTP timeout'))); req.end(body === undefined ? undefined : JSON.stringify(body))
  })
  assert.equal(response.status, expected, `${route}: HTTP ${response.status}`)
  return response.data ? JSON.parse(response.data) : null
}
const token = (await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const admin = (route, body, expected) => request(management, route, token, body, expected)
let fixture
try { fixture = JSON.parse(await readFile(fixtureFile, 'utf8')) } catch (error) { if (error.code !== 'ENOENT') throw error }
const save = () => writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
if (!fixture) {
  const name = 'Rollout-Abnahme ' + new Date().toISOString()
  const org = await admin('/management/v1/organisations', { name })
  fixture = { organisation_id: org.id, name, instances: [] }
  await save()
}
assert.match(fixture.name, /^Rollout-Abnahme /)
assert.equal((await admin('/management/v1/organisations')).items.find(org => org.id === fixture.organisation_id)?.name, fixture.name)
const base = `/management/v1/organisations/${fixture.organisation_id}/instances`
const plans = `/management/v1/organisations/${fixture.organisation_id}/rollouts`
while (fixture.instances.length < 2) {
  const name = 'Rollout fixture ' + (fixture.instances.length + 1)
  const created = await admin(base, { name, backend: 'compose' })
  fixture.instances.push({ id: created.id, name }); await save()
}
assert.equal(fixture.instances.length, 2)
const current = async id => {
  const expected = fixture.instances.find(row => row.id === id)
  assert.ok(expected)
  const row = (await admin(base)).find(value => value.id === id)
  assert.equal(row?.name, expected.name)
  return row
}
async function operation(id, action, extra = {}) {
  const op = await admin(`${base}/${id}/operations`, { action, expected_revision: (await current(id)).revision, idempotency_key: randomUUID(), ...extra })
  console.log(`${action} ${id}=${op.id}`)
  for (let second = 0; second < 600; second++) {
    const row = (await admin(`${base}/${id}/operations`)).find(value => value.id === op.id)
    if (['succeeded', 'failed', 'reconciling'].includes(row.state)) { assert.equal(row.state, 'succeeded', JSON.stringify(row.result)); return row }
    if (second > 0 && second % 60 === 0) console.log(`${action} ${op.id}: ${row.state}`)
    await delay(1000)
  }
  throw new Error(`Observe retained operation ${op.id}, do not replay`)
}
for (const row of fixture.instances) {
  assert.ok(!(await admin(`${base}/${row.id}/operations`)).some(op => ['queued', 'running', 'reconciling'].includes(op.state)), 'Observe existing work before retrying')
  if ((await current(row.id)).state === 'pending') await operation(row.id, 'provision')
}
assert.ok(!(await admin(plans)).some(row => ['running', 'paused', 'canceling'].includes(row.state)), 'Observe the existing rollout before creating another')
const releases = await Promise.all(fixture.instances.map(row => admin(`${base}/${row.id}/releases`)))
const release = releases[0].find(candidate => candidate.name === 'Admission runner acceptance' && releases.every(list => list.some(row => row.id === candidate.id && row.qualified)))
assert.ok(release, 'A completed fixture needs a new approved and qualified target before this test can be repeated')
const login = async id => {
  const launch = await admin(`${base}/${id}/launch`, {})
  return { origin: launch.url, ...(await request(launch.url, '/api/v1/auth/fleet', null, { ticket: launch.ticket, device_id: randomUUID() })) }
}
for (const row of fixture.instances) {
  if ((await current(row.id)).state === 'stopped') await operation(row.id, 'start')
  assert.equal((await current(row.id)).state, 'ready')
  if (!row.project_id) {
    const session = await login(row.id)
    const project = await request(session.origin, '/api/v1/projects', session.access_token, { name: 'Before grouped update', privacy: 'private_local' }, 201)
    row.project_id = project.id; await save()
  }
}
const [first, second] = fixture.instances
await operation(second.id, 'stop')
const body = { name: 'Docker grouped update ' + new Date().toISOString(), release: release.id, batch_size: 1, targets: await Promise.all(fixture.instances.map(async row => ({ instance_id: row.id, expected_revision: (await current(row.id)).revision }))), idempotency_key: randomUUID() }
const rollout = await admin(plans, body)
fixture.rollout_id = rollout.id; await save()
assert.equal((await admin(plans, body)).id, rollout.id)
const plan = async () => (await admin(plans)).find(row => row.id === rollout.id)
console.log('rollout=' + rollout.id)
await admin(`${base}/${first.id}/operations`, { action: 'stop', expected_revision: (await current(first.id)).revision, idempotency_key: randomUUID() }, 409)
let stopping
for (let i = 0; i < 90; i++) {
  const row = await plan()
  assert.equal(row.state, 'running', JSON.stringify(row))
  const target = row.targets[0]
  if (target.operation_id) {
    const op = (await admin(`${base}/${first.id}/operations`)).find(value => value.id === target.operation_id)
    if (op.state === 'running') { stopping = row; break }
  }
  await delay(500)
}
assert.equal(stopping?.targets[0].next_action, 'stop')
await admin(`${plans}/${rollout.id}/control`, { action: 'pause', expected_revision: stopping.revision })
let paused
for (let i = 0; i < 120; i++) {
  const row = await plan()
  assert.equal(row.state, 'paused')
  assert.equal(row.targets[1].operation_id, null)
  if (row.targets[0].state === 'waiting' && row.targets[0].next_action === 'upgrade') { paused = row; break }
  await delay(1000)
}
assert.ok(paused)
assert.equal((await current(first.id)).state, 'stopped')
pass('atomic-reservations-and-pause-before-installation')
execFileSync('docker', ['compose', '-f', 'deploy/enterprise/docker-compose.local.yml', 'restart', 'management'], { cwd: root, timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] })
let restarted
for (let i = 0; i < 120; i++) {
  try { restarted = await plan(); break } catch { await delay(1000) }
}
assert.equal(restarted?.state, 'paused')
assert.deepEqual(restarted.targets, paused.targets)
await admin(`${plans}/${rollout.id}/control`, { action: 'resume', expected_revision: restarted.revision })
pass('management-restart-retains-pause-and-original-operation')
let completed
for (let i = 0; i < 1200; i++) {
  const row = await plan()
  assert.ok(!['paused', 'canceling', 'canceled'].includes(row.state), JSON.stringify(row))
  if (row.targets[1].operation_id) assert.equal(row.targets[0].state, 'succeeded', 'Next group must wait for the first to finish')
  if (row.state === 'succeeded') { completed = row; break }
  if (i > 0 && i % 60 === 0) console.log('rollout states: ' + row.targets.map(target => target.state + '/' + target.next_action).join(', '))
  await delay(1000)
}
assert.ok(completed, 'Continue observing this rollout; do not replay')
const operations = []
for (const [index, row] of fixture.instances.entries()) {
  const info = await current(row.id)
  assert.equal(info.current_release, release.id)
  assert.equal(info.state, index === 0 ? 'ready' : 'stopped')
  assert.equal(info.rollout_id, null)
  const history = (await admin(`${base}/${row.id}/operations`)).filter(op => op.rollout_id === rollout.id).reverse()
  assert.deepEqual(history.map(op => op.action), index === 0 ? ['stop', 'upgrade', 'start'] : ['upgrade'])
  assert.ok(history.every(op => op.state === 'succeeded'))
  const upgrade = history.find(op => op.action === 'upgrade')
  assert.ok((await admin(`${base}/${row.id}/backups`)).some(backup => backup.id === upgrade.id && backup.size_bytes > 0))
  assert.equal((await admin(`${base}/${row.id}/rollbacks`))[0]?.operation_id, upgrade.id)
  operations.push(...history)
}
const session = await login(first.id)
assert.equal((await request(session.origin, `/api/v1/projects/${first.project_id}`, session.access_token)).name, 'Before grouped update')
pass('ordered-groups-backups-rollback-points-and-original-running-states')
await writeFile(new URL('rollout-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), fixture, rollout: completed, operations, checks }, null, 2))
