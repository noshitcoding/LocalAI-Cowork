// Real PostgreSQL, instance API/worker and Docker runner. Generated, isolated
// fixtures only; no live management account, tenant or MFA setting is changed.
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { randomUUID, randomBytes, createHmac } from 'node:crypto'
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import http from 'node:http'

const root = process.env.COWORK_WORKSPACE_TEST_ROOT || fileURLToPath(new URL('../../', import.meta.url))
const instance = randomUUID(), prefix = `cowork-${instance.replaceAll('-', '')}-`
const scope = `${prefix}workspace-test`, label = `dev.opencowork.workspace_test=${scope}`
const stage = mkdtempSync(join(tmpdir(), 'cowork-chat-workspace-test-'))
const names = { db: `${prefix}test-db`, api: `${prefix}test-api`, runner: `${prefix}test-runner`, worker: `${prefix}test-worker` }
const control = `${prefix}test-control`, egress = `${prefix}test-egress`, ledger = `${prefix}test-ledger`
const key = randomBytes(32).toString('hex'), bootstrap = randomBytes(32).toString('hex')
const password = randomBytes(32).toString('hex'), checks = []
const images = {
  server: process.env.COWORK_WORKSPACE_SERVER_IMAGE || 'cowork-server:chat-workspaces-candidate',
  runner: process.env.COWORK_WORKSPACE_RUNNER_IMAGE || 'cowork-runner:chat-workspaces-candidate',
  core: process.env.COWORK_WORKSPACE_CORE_IMAGE || 'cowork-sandbox-core:local-review',
  gui: process.env.COWORK_WORKSPACE_GUI_IMAGE || 'cowork-sandbox-gui:local-review',
}
writeFileSync(join(stage, 'runner-key'), key)
const docker = (...args) => execFileSync('docker', args, { cwd: stage, encoding: 'utf8', timeout: 120000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
const inspect = name => JSON.parse(docker('inspect', name))[0]
const live = docker('ps', '--quiet').split('\n').filter(Boolean).map(id => ({ id, image: inspect(id).Image }))
const pass = name => { checks.push(name); console.log(`${name}=ok`) }
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
async function until(predicate, label, seconds = 60) {
  const deadline = Date.now() + seconds * 1000
  while (Date.now() < deadline) { if (await predicate()) return; await sleep(150) }
  throw new Error(label)
}
let origin, runnerOrigin, token, successfulReport
const imageIds = Object.fromEntries(Object.entries(images).map(([name, reference]) => [name, docker('image', 'inspect', reference, '--format', '{{.Id}}')]))
const requestedImages = { ...images }
// Freeze tags before any fixture starts; a later build cannot replace a runtime
// halfway through the restart checks.
Object.assign(images, imageIds)
async function api(path, body, expected = 200, method = body === undefined ? 'GET' : 'POST', bearer = token) {
  const result = await fetch(origin + path, { method, headers: { 'Content-Type': 'application/json', ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(20000) })
  const text = await result.text()
  assert.equal(result.status, expected, `${path}: ${text.slice(0, 900)}`)
  return result.ok && text ? JSON.parse(text) : null
}
const sqlResult = statement => spawnSync('docker', ['exec', '-i', names.db, 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', 'cowork'], { input: statement, encoding: 'utf8', timeout: 20000, cwd: stage })
const sql = statement => { const r = sqlResult(statement); assert.equal(r.status, 0, r.stderr); return r.stdout.trim() }
function signed(method, path, body = '') {
  const timestamp = String(Math.floor(Date.now() / 1000)), nonce = randomUUID()
  return { 'Content-Type': 'application/json', 'x-cowork-timestamp': timestamp, 'x-cowork-nonce': nonce,
    'x-cowork-signature': createHmac('sha256', key).update(`${timestamp}\n${nonce}\n${method}\n${path}\n${body}`).digest('hex') }
}
async function runner(path, body, expected = 200, method = body === undefined ? 'GET' : 'POST') {
  const text = body === undefined ? '' : JSON.stringify(body)
  const result = await fetch(runnerOrigin + path, { method, headers: signed(method, path, text), ...(text ? { body: text } : {}), signal: AbortSignal.timeout(90000) })
  const response = await result.text()
  assert.equal(result.status, expected, `${path}: ${response.slice(0, 600)}`)
  return result.ok && response ? JSON.parse(response) : null
}
const limits = { cpu_nanos: 500000000, memory_bytes: 256 * 1024 ** 2, pids: 128, timeout_seconds: 60, tmpfs_bytes: 64 * 1024 ** 2, output_bytes: 1024 ** 2 }
const sandbox = (script, workspace = null) => ({ schema_version: 2, run_id: randomUUID(), workspace_id: workspace, image: 'core', argv: ['python3', '-c', script], network: 'none', limits })
async function readTerminal(url, headers, marker, command = 'cat artifacts/result.txt\n') {
  await new Promise((resolvePromise, reject) => {
    let text = '', socket
    const request = http.request(url, { headers: { ...headers, Connection: 'Upgrade', Upgrade: 'websocket',
      'Sec-WebSocket-Key': randomBytes(16).toString('base64'), 'Sec-WebSocket-Version': '13' } })
    const timeout = setTimeout(() => { socket?.destroy(); request.destroy(); reject(new Error('terminal workspace read timed out')) }, 15000)
    request.on('error', error => { clearTimeout(timeout); reject(error) })
    request.on('response', response => { clearTimeout(timeout); reject(new Error(`terminal upgrade: ${response.statusCode}`)) })
    request.on('upgrade', (_response, connection, head) => {
      socket = connection
      const payload = Buffer.from(command), mask = randomBytes(4)
      assert.ok(payload.length < 126, 'fixture terminal command fits a single short frame')
      const masked = Buffer.from(payload.map((byte, i) => byte ^ mask[i % 4]))
      socket.write(Buffer.concat([Buffer.from([0x82, 0x80 | payload.length]), mask, masked]))
      const receive = chunk => {
        text += chunk.toString()
        if (text.includes(marker)) { clearTimeout(timeout); socket.end(Buffer.concat([Buffer.from([0x88, 0x80]), randomBytes(4)])); resolvePromise() }
      }
      socket.on('error', error => { clearTimeout(timeout); reject(error) })
      socket.on('data', receive); receive(head)
    })
    request.end()
  })
}
async function ready(url, path = '/readyz') {
  await until(async () => { try { return (await fetch(url + path, { signal: AbortSignal.timeout(1500) })).ok } catch { return false } }, `readiness: ${url}`, 90)
}
async function startApi(enabled) {
  docker('run', '-d', '--name', names.api, '--label', label, '--network', control, '-p', '127.0.0.1::8080',
    '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp',
    '--mount', `type=bind,src=${join(stage, 'runner-key')},dst=/run/runner-key,readonly`,
    '-e', `DATABASE_URL=postgres://postgres:${password}@${names.db}:5432/cowork`, '-e', 'COWORK_MODE=all',
    '-e', 'COWORK_LISTEN_ADDR=0.0.0.0:8080', '-e', `COWORK_BOOTSTRAP_TOKEN=${bootstrap}`,
    '-e', 'COWORK_WEB_PUSH_ENABLED=false', '-e', 'COWORK_SERVER_CAPABILITIES=shell', '-e', 'COWORK_WORKER_POLL_MS=100',
    '-e', `COWORK_RUNNER_URL=http://${names.runner}:8090`, '-e', 'COWORK_RUNNER_SIGNING_KEY_FILE=/run/runner-key',
    '-e', `COWORK_CHAT_WORKSPACES_ENABLED=${enabled}`, images.server)
  origin = 'http://' + docker('port', names.api, '8080/tcp'); await ready(origin)
}
async function replaceApi(enabled) {
  assert.equal(inspect(names.api).Config.Labels['dev.opencowork.workspace_test'], scope)
  docker('stop', '--time', '15', names.api); docker('rm', names.api); await startApi(enabled)
}
let project
const thread = async title => (await api('/api/v1/threads', { project_id: project.id, title }, 201)).id
async function submit(chat, script, workspace = null, extra = {}) {
  return api('/api/v1/runs', { project_id: project.id, thread_id: chat, project_revision: project.revision,
    project_privacy: 'team_managed', executor_target: { kind: 'server_linux', pool_id: null }, required_capabilities: ['shell'],
    input: { sandbox: sandbox(script, workspace) }, idempotency_key: randomUUID(), ...extra }, 201)
}
async function completed(run) {
  let record
  await until(async () => { record = await api(`/api/v1/runs/${run.spec.id}`); return ['completed', 'failed', 'interrupted'].includes(record.state) }, 'run completion')
  assert.equal(record.state, 'completed', JSON.stringify(record.error))
  return record
}
const binding = run => sql(`SELECT coalesce(sandbox_workspace_id::text,'legacy') FROM runs WHERE id='${run.spec.id}';`)

try {
  // Host-loopback HTTP drives the trusted test services; sandbox egress stays
  // on the separate internal bridge and core jobs explicitly use network=none.
  docker('network', 'create', '--subnet', process.env.COWORK_WORKSPACE_TEST_CONTROL_CIDR || '10.254.247.0/28', '--label', label, control)
  docker('network', 'create', '--internal', '--subnet', process.env.COWORK_WORKSPACE_TEST_EGRESS_CIDR || '10.254.247.16/28', '--label', label, egress)
  docker('volume', 'create', '--label', label, ledger)
  docker('run', '-d', '--name', names.db, '--label', label, '--network', control,
    '-e', `POSTGRES_PASSWORD=${password}`, '-e', 'POSTGRES_DB=cowork', 'postgres:16')
  await until(() => sqlResult('SELECT 1').status === 0, 'PostgreSQL readiness')
  docker('run', '-d', '--name', names.runner, '--label', label, '--network', control, '-p', '127.0.0.1::8090',
    '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp',
    '--mount', 'type=bind,src=/var/run/docker.sock,dst=/var/run/docker.sock',
    '--mount', `type=bind,src=${join(stage, 'runner-key')},dst=/run/runner-key,readonly`,
    '--mount', `type=volume,src=${ledger},dst=/var/lib/cowork/admission`,
    '-e', `COWORK_RUNNER_INSTANCE_ID=${instance}`, '-e', 'COWORK_RUNNER_SIGNING_KEY_FILE=/run/runner-key',
    '-e', 'COWORK_RUNNER_ADMISSION_DIRECTORY=/var/lib/cowork/admission', '-e', 'COWORK_SANDBOX_REQUIRE_APPARMOR=false',
    '-e', `COWORK_RUNNER_CORE_IMAGE=${images.core}`, '-e', `COWORK_RUNNER_GUI_IMAGE=${images.gui}`, '-e', `COWORK_RUNNER_CREW_IMAGE=${images.core}`,
    '-e', `COWORK_SANDBOX_EGRESS_NETWORK=${egress}`, '-e', 'COWORK_SANDBOX_HTTP_PROXY=http://127.0.0.1:9', images.runner)
  runnerOrigin = 'http://' + docker('port', names.runner, '8090/tcp'); await ready(runnerOrigin, '/healthz')
  await startApi(false)
  const account = await api('/api/v1/auth/bootstrap', { email: 'workspace@example.test', display_name: 'Workspace fixture', password: randomBytes(32).toString('hex'), device_id: randomUUID() }, 201, 'POST', bootstrap)
  token = account.access_token
  const team = await api('/api/v1/teams', { name: 'Workspace fixture' }, 201)
  project = await api('/api/v1/projects', { name: 'Workspace fixture', privacy: 'team_managed', team_id: team.id }, 201)
  const legacyChat = await thread('Legacy workspace')
  const old = await completed(await submit(legacyChat, "from pathlib import Path; Path('legacy.txt').write_text('legacy-preserved')"))
  assert.equal(binding(old), 'legacy')
  const oldNext = await completed(await submit(legacyChat, "from pathlib import Path; assert not Path('legacy.txt').exists()"))
  assert.notEqual(old.result.sandbox.workspace_volume, oldNext.result.sandbox.workspace_volume)
  pass('disabled_feature_preserves_legacy_per_run_storage')
  await replaceApi(true)
  const chatA = await thread('Persistent A'), chatB = await thread('Persistent B')
  const marker = randomBytes(16).toString('hex')
  const first = await completed(await submit(chatA, `from pathlib import Path; import os; assert os.getuid()==10001; assert not Path('/var/run/docker.sock').exists(); assert not Path('/run/runner-key').exists(); Path('artifacts').mkdir(); Path('artifacts/result.txt').write_text('${marker}'); Path('tool.py').write_text('print(42)')`))
  const workspace = binding(first)
  assert.notEqual(workspace, 'legacy')
  const next = await completed(await submit(chatA, `from pathlib import Path; import subprocess; assert Path('artifacts/result.txt').read_text()=='${marker}'; assert subprocess.check_output(['python3','tool.py']).strip()==b'42'`))
  assert.equal(binding(next), workspace); assert.equal(next.result.sandbox.workspace_volume, first.result.sandbox.workspace_volume)
  pass('real_followup_keeps_files_and_workspace_installed_tool')
  const foreign = await completed(await submit(chatB, "from pathlib import Path; assert not Path('artifacts/result.txt').exists(); assert not Path('tool.py').exists()", workspace))
  assert.notEqual(binding(foreign), workspace)
  assert.notEqual(foreign.result.sandbox.workspace_volume, first.result.sandbox.workspace_volume)
  pass('another_chat_is_isolated_and_forged_workspace_input_is_ignored')
  const otherProject = await api('/api/v1/projects', { name: 'Separate project', privacy: 'team_managed', team_id: team.id }, 201)
  await api('/api/v1/runs', { project_id: otherProject.id, thread_id: chatA, project_revision: otherProject.revision,
    project_privacy: 'team_managed', executor_target: { kind: 'server_linux', pool_id: null }, required_capabilities: ['shell'],
    input: { sandbox: sandbox('print(1)', workspace) }, idempotency_key: randomUUID() }, 422)
  assert.equal(binding(first), workspace)
  pass('project_and_chat_context_cannot_be_mixed')
  docker('run', '-d', '--name', names.worker, '--label', label, '--network', control,
    '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp',
    '--mount', `type=bind,src=${join(stage, 'runner-key')},dst=/run/runner-key,readonly`,
    '-e', `DATABASE_URL=postgres://postgres:${password}@${names.db}:5432/cowork`, '-e', 'COWORK_MODE=worker',
    '-e', `COWORK_BOOTSTRAP_TOKEN=${bootstrap}`, '-e', 'COWORK_WEB_PUSH_ENABLED=false', '-e', 'COWORK_SERVER_CAPABILITIES=shell',
    '-e', 'COWORK_WORKER_POLL_MS=100', '-e', `COWORK_RUNNER_URL=http://${names.runner}:8090`,
    '-e', 'COWORK_RUNNER_SIGNING_KEY_FILE=/run/runner-key', images.server)
  const serialChat = await thread('Ordered shared workspace')
  const slow = await submit(serialChat, "from pathlib import Path; import time; Path('busy').write_text('working'); time.sleep(4); Path('busy').unlink(); Path('finished').write_text('done')")
  await until(() => sql(`SELECT state FROM runs WHERE id='${slow.spec.id}'`) === 'running', 'first serialized task starts')
  const queued = await submit(serialChat, "from pathlib import Path; assert not Path('busy').exists(); assert Path('finished').read_text()=='done'")
  // Simulate a request whose transaction committed late despite an earlier
  // creation timestamp. Both actual workers must still serialize this chat.
  sql(`UPDATE runs SET created_at=(SELECT created_at-interval '1 second' FROM runs WHERE id='${slow.spec.id}') WHERE id='${queued.spec.id}'`)
  await completed(slow); await completed(queued)
  docker('stop', '--time', '15', names.worker); docker('rm', names.worker)
  pass('two_workers_serialize_one_chat_even_with_reordered_commit_timestamps')
  for (const statement of [
    `UPDATE runs SET sandbox_workspace_id=NULL WHERE id='${first.spec.id}'`,
    `UPDATE runs SET sandbox_workspace_id='${binding(foreign)}' WHERE id='${first.spec.id}'`,
    `UPDATE chat_workspaces SET workspace_id='${randomUUID()}' WHERE thread_id='${chatA}'`,
    `UPDATE runs SET lease_token='${randomUUID()}' WHERE id='${first.spec.id}'`,
  ]) assert.notEqual(sqlResult(statement).status, 0, statement)
  assert.equal(binding(first), workspace)
  pass('database_rejects_rebinding_and_old_worker_dispatch')
  const selected = sandbox('print(1)', workspace)
  await runner('/v1/jobs', selected, 400)
  await runner('/v2/jobs', sandbox('print(1)'), 400)
  const unauthenticated = await fetch(runnerOrigin + '/v2/jobs', { method: 'POST', body: JSON.stringify(selected) })
  assert.equal(unauthenticated.status, 401)
  const artifactPath = `/v2/runs/${next.spec.id}/file?path=artifacts%2Fresult.txt&workspace_id=${workspace}`
  const artifact = await fetch(runnerOrigin + artifactPath, { headers: signed('GET', artifactPath) })
  assert.equal(artifact.status, 200); assert.equal(await artifact.text(), marker)
  pass('versioned_runner_contract_authentication_and_artifact_export')
  const terminalPath = `/v2/runs/${next.spec.id}/terminal?columns=100&rows=30&workspace_id=${workspace}`
  await readTerminal(runnerOrigin + terminalPath, signed('GET', terminalPath), marker)
  pass('real_terminal_uses_the_same_chat_workspace')
  const session = randomUUID()
  await runner('/v2/desktop-sessions', { schema_version: 2, session_id: session, run_id: next.spec.id, workspace_id: workspace,
    dimensions: { width: 1024, height: 768, scale_factor: 1 }, network: 'filtered_egress', limits: { ...limits, memory_bytes: 1024 ** 3 } })
  const guiResult = await runner('/v2/jobs', { ...sandbox(`from pathlib import Path; assert Path('artifacts/result.txt').read_text()=='${marker}'`, workspace), run_id: next.spec.id, image: 'gui' })
  assert.equal(guiResult.exit_code, 0)
  docker('restart', names.runner); runnerOrigin = 'http://' + docker('port', names.runner, '8090/tcp'); await ready(runnerOrigin, '/healthz')
  const recovered = await runner('/v2/jobs', { ...sandbox("from pathlib import Path; assert Path('tool.py').exists()", workspace), run_id: next.spec.id, image: 'gui' })
  assert.equal(recovered.exit_code, 0)
  await runner(`/v1/desktop-sessions/${session}`, undefined, 204, 'DELETE')
  pass('desktop_and_recovered_runner_keep_chat_storage')
  const interactive = await submit(chatA, "from pathlib import Path; import time; end=time.monotonic()+45\nwhile not Path('interactive-finished').exists() and time.monotonic()<end: time.sleep(.1)\nassert Path('interactive-finished').exists()")
  await until(() => sql(`SELECT state FROM runs WHERE id='${interactive.spec.id}'`) === 'running', 'interactive run starts')
  const ticket = await api(`/api/v1/runs/${interactive.spec.id}/terminal-sessions`, { columns: 100, rows: 30 }, 201)
  await readTerminal(`${origin}/api/v1/terminal-sessions/${ticket.session_id}/stream?ticket=${encodeURIComponent(ticket.token)}`, {}, marker)
  const publicDesktop = await api(`/api/v1/runs/${interactive.spec.id}/desktop-sessions`, { width: 1024, height: 768 }, 201)
  const throughDesktop = await runner('/v2/jobs', { ...sandbox(`from pathlib import Path; assert Path('artifacts/result.txt').read_text()=='${marker}'`, workspace), run_id: interactive.spec.id, image: 'gui' })
  assert.equal(throughDesktop.exit_code, 0)
  await api(`/api/v1/runs/${interactive.spec.id}/desktop-sessions/${publicDesktop.id}`, undefined, 204, 'DELETE')
  const finishTicket = await api(`/api/v1/runs/${interactive.spec.id}/terminal-sessions`, { columns: 100, rows: 30 }, 201)
  await readTerminal(`${origin}/api/v1/terminal-sessions/${finishTicket.session_id}/stream?ticket=${encodeURIComponent(finishTicket.token)}`, {}, marker, 'touch interactive-finished; cat artifacts/result.txt\n')
  await completed(interactive)
  pass('authenticated_server_terminal_and_desktop_resolve_the_frozen_binding')
  await replaceApi(false)
  const afterRestart = await completed(await submit(chatA, `from pathlib import Path; assert Path('artifacts/result.txt').read_text()=='${marker}'`))
  assert.equal(binding(afterRestart), workspace)
  const newLegacyChat = await thread('New legacy after flag change')
  assert.equal(binding(await completed(await submit(newLegacyChat, 'print(1)'))), 'legacy')
  pass('api_restart_and_disabled_creation_do_not_change_existing_bindings')
  for (const before of live) { const after = inspect(before.id); assert.equal(after.Image, before.image); assert.equal(after.State.Running, true) }
  pass('preexisting_running_containers_unchanged')
  successfulReport = { scope, checks, requestedImages, images, deployed: false, tested_at: new Date().toISOString(), live_containers_preserved: live.length, limitations: ['No Kubernetes, VM, offline-device or 40-task acceptance.', 'This fixture does not qualify the complete enterprise backup/restore or live tenant upgrade.'] }
} catch (error) {
  for (const name of Object.values(names)) {
    const log = spawnSync('docker', ['logs', '--tail', '150', name], { encoding: 'utf8', cwd: stage })
    writeFileSync(join(stage, name + '.log'), log.stdout + log.stderr)
  }
  console.error(`Private diagnostics retained: ${stage}`)
  throw error
} finally {
  const ids = docker('ps', '--all', '--quiet', '--filter', `name=^/${prefix}`).split('\n').filter(Boolean)
  for (const id of ids) {
    const info = inspect(id), name = info.Name.slice(1)
    assert.ok(Object.values(names).includes(name) || ['job-', 'terminal-', 'transfer-', 'desktop-'].some(kind => name.startsWith(prefix + kind)), name)
    docker('rm', '--force', '--volumes', id)
  }
  for (const name of docker('volume', 'ls', '--format', '{{.Name}}', '--filter', `name=^${prefix}`).split('\n').filter(Boolean)) {
    assert.ok(name === ledger || new RegExp(`^${prefix}run-[a-f0-9]{32}$`).test(name), name)
    docker('volume', 'rm', name)
  }
  for (const name of [control, egress]) {
    const found = spawnSync('docker', ['network', 'inspect', name], { encoding: 'utf8' })
    if (found.status === 0) { assert.equal(JSON.parse(found.stdout)[0].Labels['dev.opencowork.workspace_test'], scope); docker('network', 'rm', name) }
  }
  // Keep failure logs, but remove the ephemeral signing key in all outcomes.
  assert.equal(dirname(resolve(stage)), resolve(tmpdir()))
  rmSync(join(stage, 'runner-key'), { force: true })
  if (successfulReport) {
    assert.equal(docker('ps', '--all', '--quiet', '--filter', `name=^/${prefix}`), '')
    assert.equal(docker('volume', 'ls', '--format', '{{.Name}}', '--filter', `name=^${prefix}`), '')
    successfulReport.fixture_resources_removed = true
    writeFileSync(join(root, '.internal/enterprise-local/chat-workspaces-docker-results.json'), JSON.stringify(successfulReport, null, 2) + '\n')
  }
}
