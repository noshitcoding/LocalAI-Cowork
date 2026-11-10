import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Real Docker Desktop acceptance. Only creates/mutates the dedicated Backup-Abnahme instance.
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
const password = (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim()
const admin = await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password, device_id: randomUUID(), second_factor: nextManagementSecondFactor() })
const organisations = (await request(management, '/management/v1/organisations', admin.access_token)).items
const org = organisations.find(value => value.name === 'Backup-Abnahme') ?? await request(management, '/management/v1/organisations', admin.access_token, { name: 'Backup-Abnahme' })
const listPath = `/management/v1/organisations/${org.id}/instances`
const nameOption = process.argv.find(value => value.startsWith('--instance-name='))
const name = nameOption ? nameOption.slice('--instance-name='.length) : process.argv.includes('--fresh') ? `Backup-Abnahme ${new Date().toISOString()}` : 'Backup-Abnahme'
assert.ok(name.startsWith('Backup-Abnahme'), 'Only dedicated acceptance instances may be used')
const existing = (await request(management, listPath, admin.access_token)).find(value => value.name === name)
const instance = existing ?? await request(management, listPath, admin.access_token, { name, backend: 'compose' })
const path = `${listPath}/${instance.id}`
const current = async () => (await request(management, listPath, admin.access_token)).find(value => value.id === instance.id)
async function operation(action, extra = {}, expectedState = 'succeeded') {
  const info = await current()
  const op = await request(management, path + '/operations', admin.access_token, { action, expected_revision: info.revision, idempotency_key: randomUUID(), ...extra })
  for (let i = 0; i < 300; i++) {
    const state = (await request(management, path + '/operations', admin.access_token)).find(value => value.id === op.id)
    if (['failed', 'succeeded', 'reconciling'].includes(state.state)) {
      assert.equal(state.state, expectedState, `${action}: ${JSON.stringify(state.result)}`)
      return state
    }
    await delay(1000)
  }
  throw new Error(`${action} timed out`)
}
const initial = await current()
if (initial.state !== 'ready') await operation(initial.state === 'stopped' ? 'start' : 'provision')
assert.equal((await current()).state, 'ready')
const login = async () => {
  const launch = await request(management, path + '/launch', admin.access_token, {})
  return { origin: launch.url, ...(await request(launch.url, '/api/v1/auth/fleet', null, { ticket: launch.ticket, device_id: randomUUID() })) }
}
let session = await login()
const api = (route, body, status, method) => request(session.origin, '/api/v1' + route, session.access_token, body, status, method)
const team = await api('/teams', { name: 'Backup-Abnahme Team' }, 201)
const project = await api('/projects', { name: 'Vor der Sicherung', privacy: 'team_managed', team_id: team.id }, 201)
const thread = await api('/threads', { project_id: project.id, title: 'Datenbank, Dateien und Arbeitsverzeichnis' }, 201)
const marker = 'Backup-Marker-' + randomUUID()
const executor = { kind: 'server_linux', pool_id: null }
const run = await api('/runs', { thread_id: thread.id, project_id: project.id, project_revision: project.revision, project_privacy: 'team_managed', executor_target: executor, required_capabilities: ['shell'], idempotency_key: randomUUID(),
  input: { prompt: 'Backup-Abnahme', sandbox: { schema_version: 2, run_id: randomUUID(), image: 'core', argv: ['python3', '-c', `import pathlib; p=pathlib.Path('/workspace/backup-marker.txt'); p.write_text('${marker}'); print(p.read_text())`], network: 'none' } } }, 201)
let state
for (let i = 0; i < 120; i++) {
  state = await api(`/runs/${run.spec.id}`)
  if (['completed', 'failed', 'canceled'].includes(state.state)) break
  await delay(1000)
}
assert.equal(state.state, 'completed', JSON.stringify(state.error))
assert.equal(state.result.sandbox.stdout.trim(), marker)
const artifact = await api(`/runs/${run.spec.id}/attachments?name=backup-marker.txt`, Buffer.from(marker), 201)
assert.equal(artifact.storage, 'object_store_encrypted')
const task = await api('/tasks', { project_id: project.id, name: 'Restore muss Zeitplan anhalten', instructions: 'Backup test', required_capabilities: ['shell'], default_target: executor, release: true }, 201)
const schedule = await api('/schedules', { task_id: task.id, project_id: project.id, thread_id: thread.id, cron: '0 0 1 1 *', timezone: 'UTC', executor_target: executor, enabled: true }, 201)
const privateProject = await api('/projects', { name: 'Wartet beim Backup auf lokale Dateien', privacy: 'private_local' }, 201)
const waitingThread = await api('/threads', { project_id: privateProject.id, title: 'Nicht wiederholen nach Restore' }, 201)
const waitingRun = await api('/runs', { thread_id: waitingThread.id, project_id: privateProject.id, project_revision: privateProject.revision, project_privacy: 'private_local', executor_target: executor, required_capabilities: ['shell'], idempotency_key: randomUUID(), input: { prompt: 'Waiting for local snapshot' } }, 201)
assert.equal((await api(`/runs/${waitingRun.spec.id}`)).state, 'waiting_for_snapshot')
pass('live_database_encrypted_artifact_workspace_schedule_and_unfinished_run_created')
await request(management, path + '/operations', admin.access_token, { action: 'backup', expected_revision: (await current()).revision, idempotency_key: randomUUID() }, 422)
await operation('stop')
const backup = await operation('backup')
assert.equal((await current()).state, 'stopped')
const catalog = await request(management, path + '/backups', admin.access_token)
const entry = catalog.find(value => value.id === backup.result.backup_id)
assert.ok(entry?.size_bytes > 0)
assert.match(entry.sha256, /^[0-9a-f]{64}$/)
pass('stopped_instance_consistent_encrypted_backup_and_verified_catalog')
await operation('start')
session = await login()
await api(`/projects/${project.id}`, { expected_revision: project.revision, name: 'Nach der Sicherung geändert' }, 200, 'PUT')
const after = await api('/projects', { name: 'Erst nach der Sicherung erstellt', privacy: 'private_local' }, 201)
assert.equal((await api(`/projects/${project.id}`)).name, 'Nach der Sicherung geändert')
await operation('stop')
// A real encrypted archive tamper must fail before switching any resources.
const compact = instance.id.replaceAll('-', '')
const operator = 'cowork-enterprise-local-provisioner-1'
const root = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const archive = `${root}/backups/${entry.id.replaceAll('-', '')}.age`
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', timeout: 30000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
docker('exec', operator, 'python3', '-c', 'import pathlib,sys;p=pathlib.Path(sys.argv[1]);p.with_suffix(".test-original").write_bytes(p.read_bytes());p.write_bytes(p.read_bytes()+b"tampered")', archive)
try {
  const rejected = await operation('restore', { backup_id: entry.id }, 'failed')
  assert.equal(rejected.result.error_code, 'backup_catalog_integrity_failed')
  assert.equal((await current()).state, 'stopped')
} finally {
  docker('exec', operator, 'python3', '-c', 'import pathlib,sys;p=pathlib.Path(sys.argv[1]);p.with_suffix(".test-original").replace(p)', archive)
}
pass('tampered_encrypted_archive_rejected_original_instance_remains_startable')
const foreign = organisations.find(value => value.name === 'test c')
assert.ok(foreign)
await request(management, `/management/v1/organisations/${foreign.id}/instances/${instance.id}/backups`, admin.access_token, undefined, 404)
await request(management, path + '/operations', admin.access_token, { action: 'restore', backup_id: randomUUID(), expected_revision: (await current()).revision, idempotency_key: randomUUID() }, 404)
pass('backup_catalog_and_restore_are_scoped_to_organisation_and_instance')
const restored = await operation('restore', { backup_id: entry.id })
assert.equal((await current()).state, 'stopped')
const generation = restored.id.replaceAll('-', '')
const base = `${root}/instances/${compact}`
const resources = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys,uuid;from pathlib import Path;from provisioner import resource_names; p=Path(sys.argv[1]); i={"id":sys.argv[2]}; stage=p/"generations"/sys.argv[3]; previous=json.load(open(stage/"restore.json"))["previous_generation"]; old=p/"generations"/uuid.UUID(previous).hex if previous else p; print(json.dumps({"old":resource_names(i,old),"new":resource_names(i,stage)}))', base, instance.id, generation))
assert.notEqual(resources.old.database, resources.new.database)
assert.notEqual(resources.old.bucket, resources.new.bucket)
const keyChecks = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys,uuid;from pathlib import Path;p=Path(sys.argv[1]);stage=p/"generations"/sys.argv[2];previous=json.load(open(stage/"restore.json"))["previous_generation"];old=p/"generations"/uuid.UUID(previous).hex if previous else p;names=["storage_master_key","runner_signing_key","bootstrap_token","database_password","s3_secret_key"];print(json.dumps({name:(old/"secrets"/name).read_bytes()==(stage/"secrets"/name).read_bytes() for name in names}))', base, generation))
assert.equal(keyChecks.storage_master_key, true)
for (const name of ['runner_signing_key', 'bootstrap_token', 'database_password', 's3_secret_key']) assert.equal(keyChecks[name], false, `${name} must rotate on restore`)
const oldVolume = resources.old.volume_prefix + run.spec.id.replaceAll('-', '')
const newVolume = resources.new.volume_prefix + run.spec.id.replaceAll('-', '')
docker('volume', 'inspect', '--format', '{{.Name}}', oldVolume)
docker('volume', 'inspect', '--format', '{{.Name}}', newVolume)
const originalMarker = docker('run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--mount', `type=volume,src=${newVolume},dst=/workspace,readonly`, 'cowork-sandbox-core:local-review', 'cat', '/workspace/backup-marker.txt')
assert.equal(originalMarker, marker)
pass('restore_uses_fresh_database_bucket_credentials_and_workspace_generation')
await operation('start')
const stale = session
session = await login()
await request(stale.origin, '/api/v1/projects', stale.access_token, undefined, 401)
assert.equal((await api(`/projects/${project.id}`)).name, project.name)
await api(`/projects/${after.id}`, undefined, 404)
assert.equal(await api(`/runs/${run.spec.id}/artifacts/${artifact.id}`), marker)
assert.equal((await api(`/runs/${waitingRun.spec.id}`)).state, 'interrupted')
const schedules = await api(`/schedules?project_id=${project.id}`)
const restoredSchedule = (Array.isArray(schedules) ? schedules : schedules.items).find(value => value.id === schedule.id)
assert.equal(restoredSchedule.enabled, false)
assert.equal(restoredSchedule.blocked_reason, 'instance_restored_requires_review')
pass('restored_database_and_file_decryption_match_backup_old_session_revoked_work_paused')
// The runner must address the new generation, including its transfer/read routes.
const liveConfig = JSON.parse(docker('exec', operator, 'python3', '-c', 'import json,sys;v=json.load(open(sys.argv[1])); print(json.dumps({"generation":v["services"]["runner"]["environment"].get("COWORK_RUNNER_STORAGE_GENERATION")}))', base + '/compose.json'))
assert.equal(liveConfig.generation, restored.id)
const nextRun = await api('/runs', { thread_id: thread.id, project_id: project.id, project_revision: project.revision, project_privacy: 'team_managed', executor_target: executor, required_capabilities: ['shell'], idempotency_key: randomUUID(),
  input: { prompt: 'Ausführung nach Wiederherstellung', sandbox: { schema_version: 2, run_id: randomUUID(), image: 'core', argv: ['python3', '-c', "import pathlib; pathlib.Path('/workspace/after-restore.txt').write_text('ok'); print('restored runner works')"], network: 'none' } } }, 201)
for (let i = 0; i < 120; i++) {
  state = await api(`/runs/${nextRun.spec.id}`)
  if (['completed', 'failed', 'canceled'].includes(state.state)) break
  await delay(1000)
}
assert.equal(state.state, 'completed', JSON.stringify(state.error))
assert.equal(state.result.sandbox.stdout.trim(), 'restored runner works')
docker('volume', 'inspect', '--format', '{{.Name}}', resources.new.volume_prefix + nextRun.spec.id.replaceAll('-', ''))
assert.equal(docker('volume', 'ls', '--filter', 'name=^' + resources.old.volume_prefix + nextRun.spec.id.replaceAll('-', '') + '$', '--format', '{{.Name}}'), '')
pass('post_restore_shell_execution_uses_the_new_workspace_generation')
await request(management, '/api/v1/auth/logout', admin.access_token, {}, 204)
await writeFile(new URL('backup-test-results.json', privateDir), JSON.stringify({ passed: results, instance_id: instance.id, backup_id: entry.id, restore_operation: restored.id, project_id: project.id, run_id: run.spec.id, at: new Date().toISOString() }, null, 2))
