import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Abrupt process death against the dedicated local acceptance instance only.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync, spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const origin = 'http://localhost:18082'
const compose = ['compose', '-f', 'deploy/enterprise/docker-compose.local.yml']
const stateRoot = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const checks = []
const pass = name => { checks.push(name); console.log(`${name}=ok`) }
const docker = (...args) => execFileSync('docker', args, { cwd: root, encoding: 'utf8', timeout: 120000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
let token
async function api(path, body, expected = 200) {
  const response = await fetch(origin + path, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) })
  assert.equal(response.status, expected, `${path}: HTTP ${response.status}`)
  return response.json()
}
const password = (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim()
token = (await api('/api/v1/auth/login', { email: 'admin@cowork.local', password, device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const orgs = (await api('/management/v1/organisations')).items
const org = orgs.find(value => value.name === 'Backup-Abnahme')
assert.ok(org, 'Dedicated acceptance organisation must exist')
const list = `/management/v1/organisations/${org.id}/instances`
const fixture = (await api(list)).find(value => value.name === 'Backup-Abnahme')
assert.ok(fixture && fixture.backend === 'compose', 'Dedicated acceptance instance must exist')
const instancePath = `${list}/${fixture.id}`
const current = async () => (await api(list)).find(value => value.id === fixture.id)
const operation = async action => api(instancePath + '/operations', { action, expected_revision: (await current()).revision, idempotency_key: randomUUID() })
async function waitFor(id, state) {
  for (let i = 0; i < 120; i++) {
    const row = (await api(instancePath + '/operations')).find(value => value.id === id)
    if (row?.state === state) return row
    assert.ok(!['succeeded', 'failed'].includes(row?.state), `Unexpected terminal state: ${row?.state}`)
    await delay(1000)
  }
  throw new Error(`Operation did not reach ${state}`)
}
const ensureStopped = async () => {
  if ((await current()).state === 'failed') await waitFor((await operation('provision')).id, 'succeeded')
  if ((await current()).state === 'ready') await waitFor((await operation('stop')).id, 'succeeded')
  assert.equal((await current()).state, 'stopped')
}
const compact = fixture.id.replaceAll('-', '')
const running = () => docker('ps', '--filter', `name=^/cowork-${compact}-`, '--format', '{{.Names}}')
if (process.argv.includes('--verify-review')) {
  const record = JSON.parse(await readFile(new URL('recovery-test-results.json', privateDir), 'utf8'))
  assert.equal(record.instance, fixture.id)
  const rows = await api(instancePath + '/operations')
  const recovered = rows.find(row => row.id === record.incomplete)
  assert.equal(recovered.state, 'failed')
  assert.equal(recovered.result.error_code, 'operation_interrupted_resources_stopped')
  assert.equal(recovered.result.instance_state, 'stopped')
  assert.ok(rows.some(row => row.id !== recovered.id && row.action === 'start' && row.state === 'succeeded' && row.created_at > recovered.created_at))
  assert.equal((await current()).state, 'ready')
  record.checks.push('browser_review_recovery_and_explicit_separate_start_verified')
  record.completed_at = new Date().toISOString()
  await writeFile(new URL('recovery-test-results.json', privateDir), JSON.stringify(record, null, 2))
  console.log('browser_review_recovery_and_explicit_separate_start_verified=ok')
  process.exit(0)
}
async function noOtherWork() {
  for (const org of orgs) for (const instance of await api(`/management/v1/organisations/${org.id}/instances`)) {
    const active = (await api(`/management/v1/organisations/${org.id}/instances/${instance.id}/operations`)).filter(value => ['queued', 'running', 'reconciling'].includes(value.state))
    assert.equal(active.length, 0, 'Fault test requires an idle provisioner')
  }
}
const faultWorker = String.raw`
import json,os,sys
from pathlib import Path
import provisioner as p
from operation_journal import OperationJournal
config=json.loads(Path(sys.argv[1]).read_text())
expected,mode=sys.argv[2:]
original_call=p.Manager.call
original_execute=p.Infrastructure.execute
def call(self,path,payload):
    if path.endswith('/complete'):
        assert path=='/operations/'+expected+'/complete'
        if mode=='after_completion': os._exit(73)
    result=original_call(self,path,payload)
    if path=='/claim': assert result and result['id']==expected, 'Unexpected claim in exclusive acceptance run'
    return result
def execute(self,operation):
    assert operation['id']==expected
    if mode=='after_start': os._exit(72)
    return original_execute(self,operation)
p.Manager.call=call
p.Infrastructure.execute=execute
with p.operator_lock(Path(config['state_directory'])): p.work(config,once=True)
raise AssertionError('Fault was not injected')
`
async function crash(mode) {
  await noOtherWork()
  docker(...compose, 'stop', 'provisioner')
  const op = await operation('start')
  try {
    const result = spawnSync('docker', [...compose, 'run', '--rm', '--no-deps', '-T', '--entrypoint', 'python3', 'provisioner', '-', stateRoot + '/config.json', op.id, mode], { cwd: root, input: faultWorker, encoding: 'utf8', timeout: 120000 })
    assert.equal(result.status, mode === 'after_completion' ? 73 : 72, 'Fault worker did not stop at the expected boundary')
  } finally {
    docker(...compose, 'start', 'provisioner')
  }
  return op
}

await ensureStopped()
try {
  const completed = await crash('after_completion')
  const before = docker('inspect', `cowork-${compact}-api-1`, '--format', '{{.State.StartedAt}}')
  const receipt = await waitFor(completed.id, 'succeeded')
  assert.equal(receipt.result.receipt_id, completed.id)
  await delay(4000)
  assert.equal(docker('inspect', `cowork-${compact}-api-1`, '--format', '{{.State.StartedAt}}'), before)
  assert.equal((await current()).state, 'ready')
  pass('durable_completion_delivered_after_abrupt_exit_without_container_restart')
  const lock = spawnSync('docker', [...compose, 'run', '--rm', '--no-deps', '-T', '--entrypoint', 'python3', 'provisioner', '-c', `from pathlib import Path;from operation_journal import operator_lock;\nwith operator_lock(Path('${stateRoot}/instances')): print('unexpected lock')`], { cwd: root, encoding: 'utf8', timeout: 30000 })
  assert.notEqual(lock.status, 0)
  assert.match(lock.stderr, /another_operator_owns_state_directory/)
  pass('second_operator_process_cannot_acquire_execution_authority')
  await ensureStopped()
  const incomplete = await crash('after_start')
  await waitFor(incomplete.id, 'reconciling')
  await delay(4000)
  assert.equal(running(), '')
  assert.equal((await api(instancePath + '/operations')).find(row => row.id === incomplete.id).state, 'reconciling')
  pass('unproven_operation_stays_blocked_without_reexecution')
  if (process.argv.includes('--prepare-review')) {
    await writeFile(new URL('recovery-test-results.json', privateDir), JSON.stringify({ instance: fixture.id, organisation: org.id, incomplete: incomplete.id, checks }, null, 2))
    console.log('recovery_review_ready=' + incomplete.id)
  } else {
    await api(instancePath + '/operations/' + incomplete.id + '/recover', { mode: 'stop_if_unproven' })
    const stopped = await waitFor(incomplete.id, 'failed')
    assert.equal(stopped.result.error_code, 'operation_interrupted_resources_stopped')
    assert.equal((await current()).state, 'stopped')
    assert.equal(running(), '')
    await waitFor((await operation('start')).id, 'succeeded')
    assert.equal((await current()).state, 'ready')
    pass('authorized_recovery_proves_stopped_and_explicit_new_start_succeeds')
    await writeFile(new URL('recovery-test-results.json', privateDir), JSON.stringify({ instance: fixture.id, checks, completed_at: new Date().toISOString() }, null, 2))
  }
} finally {
  docker(...compose, 'start', 'provisioner')
}
