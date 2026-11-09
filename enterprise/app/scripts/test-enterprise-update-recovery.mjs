import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Interrupt only a canary on the established backup acceptance fixture.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync, spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const compose = ['compose', '-f', 'deploy/enterprise/docker-compose.local.yml']
const state = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const instanceId = '9fa8d4c9-cf1f-4e67-a8e8-d65156589ca3'
const orgId = '55cc98fc-e9ab-4c21-9359-46bb16d75718'
const base = `/management/v1/organisations/${orgId}/instances`
const docker = (...args) => execFileSync('docker', args, { cwd: root, encoding: 'utf8', timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
let token
async function api(path, body) {
  const response = await fetch('http://localhost:18082' + path, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(15000) })
  assert.equal(response.status, 200, `${path}: ${response.status}`)
  return response.json()
}
const current = async () => (await api(base)).find(row => row.id === instanceId)
const path = `${base}/${instanceId}/operations`
async function waitOperation(id, expected) {
  for (let i = 0; i < 360; i++) {
    const row = (await api(path)).find(item => item.id === id)
    if (['failed', 'succeeded', 'reconciling'].includes(row.state)) { assert.equal(row.state, expected, JSON.stringify(row.result)); return row }
    await delay(1000)
  }
  throw new Error('Retained operation still needs observation; do not replay')
}
async function action(name) {
  const op = await api(path, { action: name, expected_revision: (await current()).revision, idempotency_key: randomUUID() })
  return waitOperation(op.id, 'succeeded')
}
token = (await api('/api/v1/auth/login', { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
assert.equal((await current())?.name, 'Backup-Abnahme 2026-09-25T06:17:23.242Z')
for (const org of (await api('/management/v1/organisations')).items) {
  for (const instance of await api(`/management/v1/organisations/${org.id}/instances`)) {
    assert.ok(!(await api(`/management/v1/organisations/${org.id}/instances/${instance.id}/operations`)).some(row => ['queued', 'running', 'reconciling'].includes(row.state)), 'Other fleet work must finish first')
  }
}
if ((await current()).state === 'ready') await action('stop')
assert.equal((await current()).state, 'stopped')
const source = (await current()).current_release
const release = (await api(`${base}/${instanceId}/releases`)).find(row => row.name === 'Admission runner acceptance')
assert.equal(release?.source_release, source)
const inspect = () => JSON.parse(docker('exec', 'cowork-enterprise-local-provisioner-1', 'python3', '-c',
  `import json,pathlib; import provisioner as p; from release_catalog import image_set_id; c=json.loads(pathlib.Path('${state}/config.json').read_text()); infra=p.Infrastructure(c); d=infra.directory('${instanceId}'); print(json.dumps({'generation':json.loads((d/'active.json').read_text()) if (d/'active.json').exists() else None,'release':image_set_id(infra.deployed_images(d))}))`))
const before = inspect()
let operation
docker(...compose, 'stop', 'provisioner')
try {
  operation = await api(path, { action: 'upgrade', release: release.id, qualification_only: true, expected_revision: (await current()).revision, idempotency_key: randomUUID() })
  const code = `import json,os,pathlib\nimport provisioner as p\nfrom instance_updates import InstanceUpdates\noriginal=InstanceUpdates.probe\ndef cut(self,*args,**kwargs):\n original(self,*args,**kwargs)\n print('candidate-probed-process-interrupted',flush=True)\n os._exit(86)\nInstanceUpdates.probe=cut\nc=json.loads(pathlib.Path('${state}/config.json').read_text())\nwith p.operator_lock(pathlib.Path(c['state_directory'])): p.work(c,once=True)\n`
  const exitCode = await new Promise((resolve, reject) => {
    const child = spawn('docker', [...compose, 'run', '--rm', '--no-deps', '--entrypoint', 'python3', 'provisioner', '-c', code], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] })
    child.stdout.pipe(process.stdout); child.stderr.pipe(process.stderr)
    child.once('error', reject); child.once('exit', resolve)
  })
  assert.equal(exitCode, 86, 'Expected the injected process exit')
} finally {
  docker(...compose, 'up', '-d', '--no-deps', '--no-build', 'provisioner')
}
await waitOperation(operation.id, 'reconciling')
assert.deepEqual((await current()).available_actions, [])
assert.equal(inspect().release, release.id, 'Retain the interrupted candidate for reconciliation')
await api(`${path}/${operation.id}/recover`, { mode: 'stop_if_unproven' })
let recovered
for (let i = 0; i < 180; i++) {
  const row = (await api(path)).find(item => item.id === operation.id)
  if (row.state === 'failed') { recovered = row; break }
  assert.equal(row.state, 'reconciling')
  await delay(1000)
}
assert.equal(recovered?.result.error_code, 'operation_interrupted_resources_stopped')
assert.equal((await current()).state, 'stopped')
assert.deepEqual(inspect(), before)
assert.ok((await api(`${base}/${instanceId}/backups`)).some(row => row.id === recovered.result.backup_id))
await action('start')
assert.equal((await current()).state, 'ready')
await writeFile(new URL('update-recovery-results.json', privateDir), JSON.stringify({ completed_at: new Date().toISOString(), instance_id: instanceId, operation_id: operation.id, source, release: release.id, before, after: inspect(), result: recovered.result }, null, 2))
console.log('interrupted-update-restores-recorded-source-without-replay=ok')
