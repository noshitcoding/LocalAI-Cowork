import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Real management/provisioner update acceptance on its own retained test instance.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const fixtureFile = new URL('update-api-fixture.json', privateDir)
let token
async function api(path, body, expected = 200) {
  const response = await fetch('http://localhost:18082' + path, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(15000) })
  assert.equal(response.status, expected, `${path}: ${response.status}`)
  return response.json()
}
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
token = (await api('/api/v1/auth/login', { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const orgs = (await api('/management/v1/organisations')).items
const org = orgs.find(value => value.id === '55cc98fc-e9ab-4c21-9359-46bb16d75718' && value.name === 'Backup-Abnahme')
assert.ok(org, 'Dedicated acceptance organisation required')
const base = `/management/v1/organisations/${org.id}/instances`
let fixture
try { fixture = JSON.parse(await readFile(fixtureFile, 'utf8')) } catch (error) { if (error.code !== 'ENOENT') throw error }
if (!fixture) {
  const name = 'Update-Abnahme ' + new Date().toISOString()
  const created = await api(base, { name, backend: 'compose' })
  fixture = { id: created.id, organisation_id: org.id, name }
  await writeFile(fixtureFile, JSON.stringify(fixture, null, 2))
}
assert.equal(fixture.organisation_id, org.id)
assert.match(fixture.name, /^Update-Abnahme /)
const current = async () => {
  const row = (await api(base)).find(value => value.id === fixture.id)
  assert.equal(row?.name, fixture.name)
  return row
}
const operationPath = `${base}/${fixture.id}/operations`
const checks = [], receipts = []
const pass = name => { checks.push(name); console.log(name + '=ok') }
async function operation(action, extra = {}) {
  const op = await api(operationPath, { action, expected_revision: (await current()).revision, idempotency_key: randomUUID(), ...extra })
  console.log(`${action}${extra.qualification_only ? '-canary' : ''}=${op.id}`)
  for (let second = 0; second < 1200; second++) {
    const row = (await api(operationPath)).find(value => value.id === op.id)
    if (['failed', 'succeeded', 'reconciling'].includes(row.state)) {
      assert.equal(row.state, 'succeeded', JSON.stringify(row.result))
      receipts.push(row)
      return row
    }
    if (second > 0 && second % 60 === 0) console.log(`${action}: still ${row.state}`)
    await delay(1000)
  }
  throw new Error('Operation still requires observation; do not replay it')
}
const pending = await api(operationPath)
assert.ok(!pending.some(row => ['queued', 'running', 'reconciling'].includes(row.state)), 'Observe the retained operation before repeating acceptance')
if ((await current()).state === 'pending') await operation('provision')
if ((await current()).state === 'ready') await operation('stop')
assert.equal((await current()).state, 'stopped')
let releases = []
for (let i = 0; i < 45; i++) {
  releases = await api(`${base}/${fixture.id}/releases`)
  if (releases.length) break
  await delay(1000)
}
const release = releases.find(value => value.name === 'Admission runner acceptance')
assert.ok(release, 'Publish a supported candidate before this acceptance; a completed fixture needs a new release')
if (!release.qualified) {
  await api(operationPath, { action: 'upgrade', expected_revision: (await current()).revision, idempotency_key: randomUUID(), release: release.id, qualification_only: false }, 422)
  pass('unqualified-installation-rejected')
}
const canary = await operation('upgrade', { release: release.id, qualification_only: true })
assert.equal(canary.result.rollback_verified, true)
assert.equal((await current()).current_release, release.source_release)
assert.equal((await current()).state, 'stopped')
assert.equal((await api(`${base}/${fixture.id}/releases`)).find(value => value.id === release.id).qualified, true)
pass('canary-preserves-source-and-qualifies-transition')
const installed = await operation('upgrade', { release: release.id, qualification_only: false })
assert.equal((await current()).current_release, release.id)
assert.equal((await current()).state, 'stopped')
const backups = await api(`${base}/${fixture.id}/backups`)
for (const row of [canary, installed]) assert.ok(backups.some(backup => backup.id === row.result.backup_id && backup.operation_id === row.id))
pass('promotion-and-automatic-backups-recorded')
await operation('start')
assert.equal((await current()).state, 'ready')
await operation('stop')
const backup = await operation('backup')
await operation('restore', { backup_id: backup.result.backup_id })
assert.equal((await current()).current_release, release.id)
await operation('start')
assert.equal((await current()).state, 'ready')
pass('updated-instance-start-backup-restore-start')
await writeFile(new URL('update-api-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), fixture, release, checks, receipts }, null, 2))
