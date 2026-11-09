import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import http from 'node:http'
import { fileURLToPath } from 'node:url'
import { chromium } from '@playwright/test'

const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const management = 'http://localhost:18082'
const password = (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim()
const results = []
const pass = name => { results.push(name); console.log(`${name}=ok`) }
async function request(origin, path, token, body, status = 200) {
  const url = new URL(path, origin)
  const data = body === undefined ? undefined : Buffer.isBuffer(body) ? body : JSON.stringify(body)
  const result = await new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port: url.port, path: url.pathname + url.search,
      method: data === undefined ? 'GET' : 'POST', headers: { Host: url.host, 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) } }, res => {
      let content = ''; res.setEncoding('utf8'); res.on('data', chunk => { content += chunk }); res.on('end', () => resolve({ status: res.statusCode, content }))
    }); req.on('error', reject); req.setTimeout(15000, () => req.destroy(new Error('HTTP timeout'))); req.end(data)
  })
  assert.equal(result.status, status, `${path}: unexpected HTTP status${path.includes('/auth/') || path.endsWith('/launch') ? '' : ' ' + result.content.slice(0, 500)}`)
  if (!result.content || result.status >= 400) return null
  try { return JSON.parse(result.content) } catch { return result.content }
}
const admin = await request(management, '/api/v1/auth/login', null, { email: 'admin@cowork.local', password, device_id: randomUUID(), second_factor: nextManagementSecondFactor() })
const orgs = (await request(management, '/management/v1/organisations', admin.access_token)).items
async function provision(orgName, instanceName) {
  const org = orgs.find(value => value.name === orgName)
  assert.ok(org, `Organisation missing: ${orgName}`)
  const path = `/management/v1/organisations/${org.id}/instances`
  let instance = (await request(management, path, admin.access_token)).find(value => value.name === instanceName)
  if (!instance) {
    await request(management, path, admin.access_token, { name: instanceName, backend: 'compose' })
    instance = (await request(management, path, admin.access_token)).find(value => value.name === instanceName)
  }
  if (['pending', 'failed', 'stopped'].includes(instance.state)) {
    await request(management, `${path}/${instance.id}/operations`, admin.access_token, { action: instance.state === 'stopped' ? 'start' : 'provision', expected_revision: instance.revision, idempotency_key: randomUUID() })
  }
  const until = Date.now() + 240000
  while (Date.now() < until) {
    instance = (await request(management, path, admin.access_token)).find(value => value.name === instanceName)
    if (instance.state === 'ready') return instance
    if (instance.state === 'failed') {
      const ops = await request(management, `${path}/${instance.id}/operations`, admin.access_token)
      if (!ops.some(op => ['queued', 'running'].includes(op.state))) throw new Error(`Provisioning failed: ${JSON.stringify(ops.map(op => ({ state: op.state, result: op.result })))}`)
    }
    await new Promise(resolve => setTimeout(resolve, 2000))
  }
  throw new Error(`Provisioning timed out: ${instance.state}`)
}
const instance = await provision('test c', 'instance 1')
pass('user_instance_provisioned_with_healthy_api_and_runner')
const other = await provision('Testorganisation Alpha', 'Docker-Testinstanz Alpha')
pass('second_organisation_instance_provisioned')
const launch = instance => request(management, `/management/v1/organisations/${instance.organisation_id}/instances/${instance.id}/launch`, admin.access_token, {})
const ticket = await launch(instance)
const otherTicket = await launch(other)
await request(otherTicket.url, '/api/v1/auth/fleet', null, { ticket: ticket.ticket, device_id: randomUUID() }, 401)
const session = await request(ticket.url, '/api/v1/auth/fleet', null, { ticket: ticket.ticket, device_id: randomUUID() })
await request(ticket.url, '/api/v1/auth/fleet', null, { ticket: ticket.ticket, device_id: randomUUID() }, 401)
await request(otherTicket.url, '/api/v1/projects', session.access_token, undefined, 401)
await request(ticket.url, '/api/v1/projects', admin.access_token, undefined, 401)
pass('single_use_instance_bound_tickets_and_separate_sessions')
const team = await request(ticket.url, '/api/v1/teams', session.access_token, { name: 'Docker Desktop Testteam' }, 201)
const project = await request(ticket.url, '/api/v1/projects', session.access_token, { name: 'Docker Desktop Funktionstest', privacy: 'team_managed', team_id: team.id }, 201)
const thread = await request(ticket.url, '/api/v1/threads', session.access_token, { project_id: project.id, title: 'Isolierte Sandbox geprüft' }, 201)
const probe = `import os,pathlib,socket,json
assert os.getuid()==10001
assert not pathlib.Path('/var/run/docker.sock').exists()
assert not pathlib.Path('/run/secrets').exists()
status=pathlib.Path('/proc/self/status').read_text()
assert 'Seccomp:\\t2' in status
assert 'NoNewPrivs:\\t1' in status
assert 'CapEff:\\t0000000000000000' in status
try: socket.create_connection(('1.1.1.1',443),timeout=2)
except OSError: pass
else: raise AssertionError('unapproved internet reachable')
pathlib.Path('/workspace/verified.txt').write_text('Sandbox works')
print(json.dumps({'uid':os.getuid(),'seccomp':True,'no_host_mounts':True,'egress_denied':True,'workspace_writable':True}))`
const run = await request(ticket.url, '/api/v1/runs', session.access_token, {
  thread_id: thread.id, project_id: project.id, project_revision: project.revision, project_privacy: 'team_managed',
  executor_target: { kind: 'server_linux', pool_id: null }, required_capabilities: ['shell'], idempotency_key: randomUUID(),
  input: { prompt: 'Docker Desktop: Isolation und Arbeitsverzeichnis prüfen', sandbox: { schema_version: 2, run_id: randomUUID(), image: 'core', argv: ['python3', '-c', probe], network: 'none' } },
}, 201)
let completed
for (let i = 0; i < 90; i++) {
  completed = await request(ticket.url, `/api/v1/runs/${run.spec.id}`, session.access_token)
  if (['completed', 'failed', 'canceled'].includes(completed.state)) break
  await new Promise(resolve => setTimeout(resolve, 1000))
}
assert.equal(completed.state, 'completed', JSON.stringify(completed.error))
assert.equal(JSON.parse(completed.result.sandbox.stdout).workspace_writable, true)
pass('real_shell_job_seccomp_nonroot_no_host_mounts_no_internet_and_writable_workspace')
const attachment = await request(ticket.url, `/api/v1/runs/${run.spec.id}/attachments?name=storage-test.txt`, session.access_token, Buffer.from('Cowork encrypted storage round trip'), 201)
assert.equal(attachment.storage, 'object_store_encrypted')
assert.equal(await request(ticket.url, `/api/v1/runs/${run.spec.id}/artifacts/${attachment.id}`, session.access_token), 'Cowork encrypted storage round trip')
pass('encrypted_file_upload_and_download')
const memberPassword = (await readFile(new URL('test_member_password', privateDir), 'utf8')).trim()
const member = await request(management, '/api/v1/auth/login', null, { email: 'member-alpha@cowork.local', password: memberPassword, device_id: randomUUID() })
const memberPath = `/management/v1/organisations/${other.organisation_id}/instances/${other.id}`
await request(management, memberPath + '/members', admin.access_token, { user_id: member.user_id, role: 'member' })
const memberLaunch = await request(management, memberPath + '/launch', member.access_token, {})
const memberSession = await request(otherTicket.url, '/api/v1/auth/fleet', null, { ticket: memberLaunch.ticket, device_id: randomUUID() })
await request(otherTicket.url, '/api/v1/projects', memberSession.access_token)
await request(management, memberPath + '/members', admin.access_token, { user_id: member.user_id, role: 'viewer' })
await request(management, memberPath + '/launch', member.access_token, {}, 404)
await request(otherTicket.url, '/api/v1/projects', memberSession.access_token, undefined, 401)
await request(management, '/api/v1/auth/logout', member.access_token, {}, 204)
pass('membership_revocation_invalidates_existing_instance_access')

if (process.argv.includes('--lifecycle')) {
  const fresh = await launch(other)
  const otherSession = await request(fresh.url, '/api/v1/auth/fleet', null, { ticket: fresh.ticket, device_id: randomUUID() })
  const kept = await request(fresh.url, '/api/v1/projects', otherSession.access_token, { name: 'Neustart-Persistenztest', privacy: 'private_local' }, 201)
  const listPath = `/management/v1/organisations/${other.organisation_id}/instances`
  const operationPath = `${listPath}/${other.id}/operations`
  for (const [action, target] of [['stop', 'stopped'], ['start', 'ready']]) {
    const current = (await request(management, listPath, admin.access_token)).find(value => value.id === other.id)
    const op = await request(management, operationPath, admin.access_token, { action, expected_revision: current.revision, idempotency_key: randomUUID() })
    let finished = false
    for (let i = 0; i < 120; i++) {
      const status = (await request(management, operationPath, admin.access_token)).find(value => value.id === op.id)
      assert.notEqual(status.state, 'failed', JSON.stringify(status.result))
      if (status.state === 'succeeded') { finished = true; break }
      await new Promise(resolve => setTimeout(resolve, 1000))
    }
    assert.ok(finished, `${action} completion timeout`)
    assert.equal((await request(management, listPath, admin.access_token)).find(value => value.id === other.id).state, target)
  }
  assert.equal((await request(fresh.url, `/api/v1/projects/${kept.id}`, otherSession.access_token)).name, kept.name)
  pass('stop_start_preserves_database_and_existing_instance_session')
}

const browser = await chromium.launch({ channel: 'msedge', headless: true })
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
  const page = await context.newPage()
  const errors = []; page.on('pageerror', error => errors.push(error.message))
  await page.goto(management + '/management')
  await page.getByLabel('Email', { exact: true }).fill('admin@cowork.local')
  await page.getByLabel('Password', { exact: true }).fill(password)
  await page.getByLabel('Authenticator or recovery code', { exact: true }).fill(nextManagementSecondFactor())
  await page.locator('button[type=submit]').filter({ hasText: 'Sign in' }).click()
  await page.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
  await page.locator('.enterprise-management > section select').first().selectOption({ label: 'test c' })
  await page.getByRole('button', { name: 'Open instance', exact: true }).click()
  await page.getByRole('heading', { name: 'Runs', exact: true }).waitFor({ timeout: 30000 })
  assert.equal(new URL(page.url()).origin, ticket.url)
  assert.equal(new URL(page.url()).hash, '')
  const cookie = (await context.cookies(ticket.url)).find(value => value.name === '__Host-cowork_refresh')
  assert.ok(cookie?.httpOnly && cookie.secure)
  await page.reload()
  await page.getByRole('heading', { name: 'Runs', exact: true }).waitFor()
  await page.screenshot({ path: fileURLToPath(new URL('instance-working.png', privateDir)), fullPage: true })
  assert.deepEqual(errors, [])
  pass('browser_open_instance_cookie_isolation_and_session_restore')
  if (process.argv.includes('--desktop')) {
    const desktop = await chromium.connectOverCDP('http://127.0.0.1:9223')
    const native = desktop.contexts()[0].pages().find(value => value.url().includes('127.0.0.1:5173'))
    assert.ok(native, 'Development desktop is not running')
    await native.goto('http://127.0.0.1:5173/management')
    // The same persisted native session may already be signed in to management.
    await native.waitForTimeout(1500)
    if (await native.getByRole('button', { name: /Sign out/ }).count()) await native.getByRole('button', { name: /Sign out/ }).first().click()
    await native.getByLabel('Server URL', { exact: true }).fill(management)
    await native.getByLabel('Email', { exact: true }).fill('admin@cowork.local')
    await native.getByLabel('Password', { exact: true }).fill(password)
  await native.getByLabel('Authenticator or recovery code', { exact: true }).fill(nextManagementSecondFactor())
    await native.locator('button[type=submit]').filter({ hasText: 'Sign in' }).click()
    await native.getByRole('heading', { name: 'Enterprise management', exact: true }).waitFor()
    await native.locator('.enterprise-management > section select').first().selectOption({ label: 'test c' })
    await native.getByRole('button', { name: 'Open instance', exact: true }).click()
    await native.getByRole('heading', { name: 'Runs', exact: true }).waitFor({ timeout: 30000 })
    pass('native_desktop_connects_to_managed_instance')
    await desktop.close()
  }
} finally { await browser.close() }
await request(management, '/api/v1/auth/logout', admin.access_token, {}, 204)
await writeFile(new URL('provisioning-test-results.json', privateDir), JSON.stringify({ passed: results, instance_id: instance.id, instance_url: ticket.url, run_id: run.spec.id, at: new Date().toISOString() }, null, 2))
