import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Actual update, interrupted rollback and data recovery on the guarded fixture.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync, spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import http from 'node:http'

const root = fileURLToPath(new URL('../../', import.meta.url))
const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const org = '55cc98fc-e9ab-4c21-9359-46bb16d75718'
const instance = '9fa8d4c9-cf1f-4e67-a8e8-d65156589ca3'
const base = `/management/v1/organisations/${org}/instances`
const path = `${base}/${instance}`
const management = 'http://localhost:18082'
const stateRoot = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const compose = ['compose', '-f', 'deploy/enterprise/docker-compose.local.yml']
const operator = 'cowork-enterprise-local-provisioner-1'
const docker = (...args) => execFileSync('docker', args, { cwd: root, encoding: 'utf8', timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const checks = [], receipts = []
const pass = name => { checks.push(name); console.log(`${name}=ok`) }
async function request(origin, route, token, body, expected = 200, method = body === undefined ? 'GET' : 'POST') {
  const url = new URL(route, origin)
  const data = body === undefined ? undefined : Buffer.isBuffer(body) ? body : JSON.stringify(body)
  const response = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search, method,
      headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let text = ''; res.setEncoding('utf8'); res.on('data', part => { text += part }); res.on('end', () => resolve({ status: res.statusCode, text }))
    }); req.on('error', reject); req.setTimeout(20000, () => req.destroy(new Error('HTTP timeout'))); req.end(data)
  })
  assert.equal(response.status, expected, `${route}: HTTP ${response.status}`)
  if (!response.text || expected >= 400) return null
  try { return JSON.parse(response.text) } catch { return response.text }
}
const token = (await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const admin = (route, body, status) => request(management, route, token, body, status)
const current = async () => (await admin(base)).find(row => row.id === instance)
const enqueue = (action, extra = {}) => current().then(row => admin(path + '/operations', { action, expected_revision: row.revision, idempotency_key: randomUUID(), ...extra }))
async function waitOperation(id, expected = 'succeeded') {
  for (let i = 0; i < 600; i++) {
    const row = (await admin(path + '/operations')).find(value => value.id === id)
    if (['succeeded', 'failed', 'reconciling'].includes(row.state)) {
      assert.equal(row.state, expected, JSON.stringify(row.result)); receipts.push(row); return row
    }
    if (i > 0 && i % 60 === 0) console.log(`operation ${id}: ${row.state}`)
    await delay(1000)
  }
  throw new Error(`Continue observing retained operation ${id}; do not replay`)
}
const action = async (name, extra = {}) => waitOperation((await enqueue(name, extra)).id)
assert.equal((await current())?.name, 'Backup-Abnahme 2026-09-25T06:17:23.242Z')
for (const organisation of (await admin('/management/v1/organisations')).items) {
  for (const row of await admin(`/management/v1/organisations/${organisation.id}/instances`)) {
    assert.ok(!(await admin(`/management/v1/organisations/${organisation.id}/instances/${row.id}/operations`)).some(op => ['queued', 'running', 'reconciling'].includes(op.state)), 'Fleet work must finish before failure injection')
  }
}
const release = (await admin(path + '/releases')).find(row => row.name === 'Admission runner acceptance')
assert.ok(release, 'Fixture must still be on the supported source release')
if ((await current()).state === 'stopped') await action('start')
assert.equal((await current()).state, 'ready')
const login = async () => {
  const launch = await admin(path + '/launch', {})
  return { origin: launch.url, ...(await request(launch.url, '/api/v1/auth/fleet', null, { ticket: launch.ticket, device_id: randomUUID() })) }
}
let session = await login()
const api = (route, body, status, method) => request(session.origin, '/api/v1' + route, session.access_token, body, status, method)
const team = await api('/teams', { name: 'Rollback-Abnahme' }, 201)
const project = await api('/projects', { name: 'Vor dem Update ' + randomUUID(), privacy: 'team_managed', team_id: team.id }, 201)
const thread = await api('/threads', { project_id: project.id, title: 'Rollback-Daten' }, 201)
const executor = { kind: 'server_linux', pool_id: null }
async function shell(marker) {
  const run = await api('/runs', { thread_id: thread.id, project_id: project.id, project_revision: project.revision, project_privacy: 'team_managed', executor_target: executor, required_capabilities: ['shell'], idempotency_key: randomUUID(), input: { prompt: 'Rollback-Abnahme', sandbox: { schema_version: 2, run_id: randomUUID(), image: 'core', argv: ['python3', '-c', `from pathlib import Path; Path('/workspace/rollback.txt').write_text('${marker}'); print('${marker}')`], network: 'none' } } }, 201)
  for (let i = 0; i < 120; i++) {
    const row = await api(`/runs/${run.spec.id}`)
    if (['completed', 'failed', 'canceled'].includes(row.state)) { assert.equal(row.state, 'completed', JSON.stringify(row.error)); assert.equal(row.result.sandbox.stdout.trim(), marker); return run }
    await delay(1000)
  }
  throw new Error('Observe retained shell run')
}
const marker = 'rollback-' + randomUUID()
const run = await shell(marker)
const artifact = await api(`/runs/${run.spec.id}/attachments?name=rollback.txt`, Buffer.from(marker), 201)
assert.equal(artifact.storage, 'object_store_encrypted')
const task = await api('/tasks', { project_id: project.id, name: 'Rollback schedule fence', instructions: 'Acceptance', required_capabilities: ['shell'], default_target: executor, release: true }, 201)
const schedule = await api('/schedules', { task_id: task.id, project_id: project.id, thread_id: thread.id, cron: '0 0 1 1 *', timezone: 'UTC', executor_target: executor, enabled: true }, 201)
await action('stop')
if (!release.qualified) await action('upgrade', { release: release.id, qualification_only: true })
const promotion = await action('upgrade', { release: release.id })
assert.equal((await current()).current_release, release.id)
assert.equal((await admin(path + '/rollbacks'))[0]?.operation_id, promotion.id)
await action('start')
session = await login()
await api(`/projects/${project.id}`, { expected_revision: project.revision, name: 'Nach dem Update geändert' }, 200, 'PUT')
const after = await api('/projects', { name: 'Nur in der neuen Version ' + randomUUID(), privacy: 'private_local' }, 201)
await action('stop')
pass('promotion-data-change-and-scoped-rollback-point')
const inspect = () => JSON.parse(docker('exec', operator, 'python3', '-c', `import json,pathlib; import provisioner as p; from release_catalog import image_set_id; c=json.loads(pathlib.Path('${stateRoot}/config.json').read_text()); infra=p.Infrastructure(c); d=infra.directory('${instance}'); print(json.dumps({'generation':json.loads((d/'active.json').read_text()),'release':image_set_id(infra.deployed_images(d))}))`))
const promotedState = inspect()
let interrupted
docker(...compose, 'stop', 'provisioner')
try {
  interrupted = await enqueue('rollback', { rollback_operation_id: promotion.id })
  const code = `import json,os,pathlib\nimport provisioner as p\nfrom instance_rollbacks import InstanceRollbacks\noriginal=InstanceRollbacks.probe\ndef cut(self,*args,**kwargs):\n original(self,*args,**kwargs)\n print('rollback-probe-process-interrupted',flush=True)\n os._exit(86)\nInstanceRollbacks.probe=cut\nc=json.loads(pathlib.Path('${stateRoot}/config.json').read_text())\nwith p.operator_lock(pathlib.Path(c['state_directory'])): p.work(c,once=True)\n`
  const exitCode = await new Promise((resolve, reject) => {
    const child = spawn('docker', [...compose, 'run', '--rm', '--no-deps', '--entrypoint', 'python3', 'provisioner', '-c', code], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] })
    child.stdout.pipe(process.stdout); child.stderr.pipe(process.stderr); child.once('error', reject); child.once('exit', resolve)
  })
  assert.equal(exitCode, 86)
} finally { docker(...compose, 'up', '-d', '--no-deps', '--no-build', 'provisioner') }
await waitOperation(interrupted.id, 'reconciling')
assert.deepEqual((await current()).available_actions, [])
await admin(`${path}/operations/${interrupted.id}/recover`, { mode: 'stop_if_unproven' })
let recovered
for (let i = 0; i < 180; i++) {
  const row = (await admin(path + '/operations')).find(value => value.id === interrupted.id)
  if (row.state === 'failed') { recovered = row; break }
  assert.equal(row.state, 'reconciling'); await delay(1000)
}
assert.equal(recovered?.result.error_code, 'operation_interrupted_resources_stopped')
assert.deepEqual(inspect(), promotedState)
await action('start'); session = await login()
assert.equal((await api(`/projects/${project.id}`)).name, 'Nach dem Update geändert')
assert.equal((await api(`/projects/${after.id}`)).id, after.id)
await action('stop')
pass('interrupted-rollback-preserves-newer-generation-and-data-without-replay')
const rolled = await action('rollback', { rollback_operation_id: promotion.id })
assert.equal((await current()).current_release, release.source_release)
assert.equal(rolled.result.restored_backup_id, promotion.id)
assert.deepEqual(await admin(path + '/rollbacks'), [])
const backups = await admin(path + '/backups')
for (const id of [promotion.id, interrupted.id, rolled.id]) assert.ok(backups.some(row => row.id === id && row.size_bytes > 0))
const keyChecks = JSON.parse(docker('exec', operator, 'python3', '-c', `import json,pathlib; p=pathlib.Path('${stateRoot}/instances/${instance.replaceAll('-', '')}/generations'); a=p/'${promotion.id.replaceAll('-', '')}'/'secrets'; b=p/'${rolled.id.replaceAll('-', '')}'/'secrets'; print(json.dumps({k:(a/k).read_bytes()==(b/k).read_bytes() for k in ['storage_master_key','runner_signing_key','bootstrap_token','database_password','s3_secret_key']}))`))
assert.equal(keyChecks.storage_master_key, true)
for (const key of ['runner_signing_key', 'bootstrap_token', 'database_password', 's3_secret_key']) assert.equal(keyChecks[key], false)
await action('start')
const stale = session; session = await login()
await request(stale.origin, '/api/v1/projects', stale.access_token, undefined, 401)
assert.equal((await api(`/projects/${project.id}`)).name, project.name)
await api(`/projects/${after.id}`, undefined, 404)
assert.equal(await api(`/runs/${run.spec.id}/artifacts/${artifact.id}`), marker)
const schedules = await api(`/schedules?project_id=${project.id}`)
const restoredSchedule = (Array.isArray(schedules) ? schedules : schedules.items).find(row => row.id === schedule.id)
assert.equal(restoredSchedule.enabled, false)
assert.equal(restoredSchedule.blocked_reason, 'instance_restored_requires_review')
const volume = JSON.parse(docker('exec', operator, 'python3', '-c', `import json,pathlib; import provisioner as p; c=json.loads(pathlib.Path('${stateRoot}/config.json').read_text()); infra=p.Infrastructure(c); d=infra.directory('${instance}'); print(json.dumps(infra.resource_names({'id':'${instance}'},infra.active_directory(d))))`)).volume_prefix + run.spec.id.replaceAll('-', '')
assert.equal(docker('run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--mount', `type=volume,src=${volume},dst=/workspace,readonly`, 'cowork-sandbox-core:local-review', 'cat', '/workspace/rollback.txt'), marker)
await shell('after-rollback-' + randomUUID())
pass('rollback-restores-database-encrypted-file-workspace-rotates-credentials-and-fences-work')
assert.equal((await current()).state, 'ready')
await writeFile(new URL('rollback-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), instance_id: instance, release, checks, receipts, recovered, keyChecks, project_id: project.id, run_id: run.spec.id }, null, 2))
