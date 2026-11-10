// Destructive acceptance: only instances created by this invocation are deleted.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import http from 'node:http'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const management = 'http://localhost:18082'
const results = []
const pass = name => { results.push(name); console.log(`${name}=ok`) }
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
async function request(origin, path, token, body, status = 200, method = body === undefined ? 'GET' : 'POST') {
  const url = new URL(path, origin)
  const data = body === undefined ? undefined : Buffer.isBuffer(body) ? body : JSON.stringify(body)
  const response = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search, method,
      headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let text = ''; res.setEncoding('utf8'); res.on('data', part => { text += part }); res.on('end', () => resolve({ status: res.statusCode, text }))
    }); req.on('error', reject); req.setTimeout(20000, () => req.destroy(new Error('HTTP timeout'))); req.end(data)
  })
  assert.equal(response.status, status, `${path}: HTTP ${response.status}${path.includes('/auth/') || path.endsWith('/launch') ? '' : ' ' + response.text.slice(0, 300)}`)
  if (!response.text || status >= 400) return null
  try { return JSON.parse(response.text) } catch { return response.text }
}
const credentials = JSON.parse(await readFile(new URL('organisation-invitations-fixture.json', privateDir), 'utf8'))
const auth = await request(management, '/api/v1/auth/login', null, { email: credentials.email, password: credentials.password, device_id: randomUUID() })
const token = auth.access_token
const org = credentials.organisation_id
const list = `/management/v1/organisations/${org}/instances`
const before = await request(management, list, token)
assert.ok(before.length < 3, 'Do not change an existing instance to make room for this test')
const name = `Lösch-Abnahme ${new Date().toISOString()}`
const instance = await request(management, list, token, { name, backend: 'compose' })
const path = `${list}/${instance.id}`
const compact = instance.id.replaceAll('-', '')
const scope = `cowork-${compact}`
const operator = 'cowork-enterprise-local-provisioner-1'
const root = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const directory = `${root}/instances/${compact}`
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', timeout: 60000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
const current = async () => (await request(management, list, token)).find(row => row.id === instance.id)
const beforeNeighbors = JSON.parse(docker('inspect', ...docker('ps', '-q', '--filter', 'name=^/cowork-').split('\n'))).filter(c => !c.Name.startsWith('/cowork-enterprise-')).map(c => ({ id: c.Id, image: c.Image, name: c.Name })).sort((a,b) => a.name.localeCompare(b.name))
await writeFile(new URL('deletion-fixture.json', privateDir), JSON.stringify({ organisation_id: org, instance_id: instance.id, name, created_at: new Date().toISOString() }, null, 2))
async function operation(action, extra = {}) {
  const info = await current()
  const op = await request(management, path + '/operations', token, { action, expected_revision: info.revision, idempotency_key: randomUUID(), ...extra })
  for (let i = 0; i < 300; i++) {
    const value = (await request(management, path + '/operations', token)).find(row => row.id === op.id)
    if (['succeeded', 'failed', 'reconciling'].includes(value.state)) {
      assert.equal(value.state, action === 'delete' ? 'reconciling' : 'succeeded', `${action}: ${JSON.stringify(value.result)}`)
      return value
    }
    await delay(1000)
  }
  throw new Error(`${action} timed out`)
}
await operation('provision')
const launch = await request(management, path + '/launch', token, {})
const session = await request(launch.url, '/api/v1/auth/fleet', null, { ticket: launch.ticket, device_id: randomUUID() })
const api = (route, body, status) => request(launch.url, '/api/v1' + route, session.access_token, body, status)
const team = await api('/teams', { name: 'Lösch-Abnahme' }, 201)
const project = await api('/projects', { name: 'Nur für Lösch-Abnahme', privacy: 'team_managed', team_id: team.id }, 201)
const thread = await api('/threads', { project_id: project.id, title: 'Disposable deletion acceptance' }, 201)
const run = await api('/runs', { thread_id: thread.id, project_id: project.id, project_revision: project.revision, project_privacy: 'team_managed', executor_target: { kind: 'server_linux', pool_id: null }, required_capabilities: ['shell'], idempotency_key: randomUUID(), input: { prompt: 'Lösch-Abnahme', sandbox: { schema_version: 2, run_id: randomUUID(), image: 'core', argv: ['python3', '-c', "from pathlib import Path;Path('/workspace/delete-fixture.txt').write_text('Disposable deletion fixture');print('ready')"], network: 'none' } } }, 201)
let runState
for (let i = 0; i < 120; i++) {
  runState = await api(`/runs/${run.spec.id}`)
  if (['completed', 'failed', 'canceled'].includes(runState.state)) break
  await delay(1000)
}
assert.equal(runState.state, 'completed')
assert.equal((await api(`/runs/${run.spec.id}/attachments?name=delete-fixture.txt`, Buffer.from('Encrypted disposable fixture'), 201)).storage, 'object_store_encrypted')
await operation('stop')
const backup = await operation('backup')
await operation('restore', { backup_id: backup.result.backup_id })
const plan = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;from pathlib import Path;from provisioner import Infrastructure;from instance_deletion import InstanceDeletion;c=json.load(open(sys.argv[1]+"/config.json"));i=Infrastructure(c);o=json.loads(sys.argv[2]);print(json.dumps(InstanceDeletion(i).inventory(o,i.directory(o["instance"]["id"]))))', root, JSON.stringify({ id: randomUUID(), instance: await current(), arguments: { delete_backup_ids: [backup.id] } })))
assert.equal(plan.resources.length, 2)
assert.ok(plan.volumes.length >= 2)
pass('real_instance_with_encrypted_file_backup_and_two_resource_generations')
// A foreign-to-Compose container holds only this disposable fixture's volume.
// Docker must reject removal, leaving a genuinely partial deletion for recovery.
const heldVolume = plan.volumes.find(value => value.endsWith(run.spec.id.replaceAll('-', '')) && !value.includes('-g-'))
assert.ok(heldVolume)
const holder = `deletion-acceptance-hold-${randomUUID().replaceAll('-', '')}`
const image = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;print(json.dumps(json.load(open(sys.argv[1]+"/config.json"))["images"]["sandbox_core"]))', root))
docker('run', '-d', '--name', holder, '--network', 'none', '--read-only', '--user', '10001:10001', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--mount', `type=volume,src=${heldVolume},target=/workspace`, '--entrypoint', 'python3', image, '-c', 'import time; time.sleep(600)')
let deletion
try {
  await request(management, path + '/operations', token, { action: 'delete', expected_revision: (await current()).revision, idempotency_key: randomUUID(), confirmation_name: name + 'wrong', purge_backups: true }, 422)
  deletion = await operation('delete', { confirmation_name: name, purge_backups: true })
  assert.equal((await current()).state, 'deleting')
  const partial = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;from pathlib import Path;from provisioner import Infrastructure;from instance_deletion import InstanceDeletion;c=json.load(open(sys.argv[1]+"/config.json"));m=InstanceDeletion(Infrastructure(c));p=json.loads(sys.argv[2]);print(json.dumps({"databases_absent":all(not m.sql("SELECT 1 FROM pg_database WHERE datname=\x27"+r["database"]+"\x27;").strip() for r in p["resources"]),"keys_retained":(Path(c["state_directory"])/p["instance_id"].replace("-","")/"secrets").exists()}))', root, JSON.stringify(plan)))
  assert.deepEqual(partial, { databases_absent: true, keys_retained: true })
  pass('real_partial_deletion_remains_locked_with_inventory_and_keys_for_review')
} finally {
  docker('rm', '-f', holder)
}
await delay(7000)
assert.equal((await current()).state, 'deleting')
assert.ok(docker('volume', 'inspect', heldVolume), 'No automatic destructive retry after removing blocker')
await request(management, `${path}/operations/${deletion.id}/recover`, token, { mode: 'stop_if_unproven' }, 422)
await request(management, `${path}/operations/${deletion.id}/recover`, token, { mode: 'continue_deletion', confirmation_name: name, purge_backups: true })
for (let i = 0; i < 120 && await current(); i++) await delay(1000)
assert.equal(await current(), undefined)
const receipt = JSON.parse(docker('exec', operator, 'cat', `${directory}/deleted.json`))
assert.equal(receipt.deletion_verified, true)
assert.equal(receipt.receipt_id, deletion.id)
assert.match(receipt.deletion_inventory_sha256, /^[0-9a-f]{64}$/)
const remaining = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;from pathlib import Path;from provisioner import Infrastructure;from instance_deletion import InstanceDeletion;c=json.load(open(sys.argv[1]+"/config.json"));m=InstanceDeletion(Infrastructure(c));p=json.loads(sys.argv[2]);d=Path(c["state_directory"])/p["instance_id"].replace("-","");print(json.dumps({"docker_empty":all(not m.docker(k,p["scope"]) for k in ["container","volume","network"]),"databases_and_roles_absent":all(not m.sql("SELECT 1 FROM pg_database WHERE datname=\x27"+r["database"]+"\x27 UNION ALL SELECT 1 FROM pg_roles WHERE rolname=\x27"+r["database"]+"\x27;").strip() for r in p["resources"]),"backups_absent":all(not (Path(c["backup_directory"])/(b.replace("-","")+s)).exists() for b in p["backup_ids"] for s in [".json",".age"]),"metadata_only":all(v.name.startswith(("deletion-","operation-","receipt-")) or v.name=="deleted.json" for v in d.iterdir())}))', root, JSON.stringify(plan)))
remaining.storage_absent = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;from provisioner import Infrastructure;from instance_deletion import InstanceDeletion;c=json.load(open(sys.argv[1]+"/config.json"));m=InstanceDeletion(Infrastructure(c));p=json.loads(sys.argv[2]);u={r["accessKey"] for r in m.rows("admin","user","list",m.alias)};b={r["key"].rstrip("/") for r in m.rows("ls",m.alias)};a={r["policy"] for r in m.rows("admin","policy","list",m.alias)};print(json.dumps(all(r["s3_user"] not in u and r["bucket"] not in b and r["bucket"] not in a for r in p["resources"])))', root, JSON.stringify(plan)))
assert.ok(Object.values(remaining).every(Boolean), JSON.stringify(remaining))
const afterNeighbors = JSON.parse(docker('inspect', ...docker('ps', '-q', '--filter', 'name=^/cowork-').split('\n'))).filter(c => !c.Name.startsWith('/cowork-enterprise-')).map(c => ({ id: c.Id, image: c.Image, name: c.Name })).sort((a,b) => a.name.localeCompare(b.name))
assert.deepEqual(afterNeighbors, beforeNeighbors)
assert.deepEqual(await request(management, list, token), before)
pass('explicit_recovery_removes_generations_keys_archives_and_docker_resources_preserving_neighbors')
await writeFile(new URL('deletion-results.json', privateDir), JSON.stringify({ verified_at: new Date().toISOString(), instance_id: instance.id, organisation_id: org, tests: results, receipt, remaining, neighbors: beforeNeighbors.length }, null, 2))
