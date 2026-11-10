import { nextManagementSecondFactor } from './enterprise-test-auth.mjs'
// Backend canary acceptance. Public update/release APIs remain gated separately.
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { execFileSync, spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
const compose = ['compose', '-f', 'deploy/enterprise/docker-compose.local.yml']
const stateRoot = '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data'
const docker = (...args) => execFileSync('docker', args, { cwd: root, encoding: 'utf8', timeout: 120000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
let token
async function api(path, body) {
  const response = await fetch('http://localhost:18082' + path, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(15000) })
  assert.equal(response.status, 200, `${path}: ${response.status}`)
  return response.json()
}
token = (await api('/api/v1/auth/login', { email: 'admin@cowork.local', password: (await readFile(new URL('management_admin_password', privateDir), 'utf8')).trim(), device_id: randomUUID(), second_factor: nextManagementSecondFactor() })).access_token
const orgs = (await api('/management/v1/organisations')).items
let fixture, base
for (const org of orgs) {
  const path = `/management/v1/organisations/${org.id}/instances`
  for (const instance of await api(path)) {
    const operations = await api(path + `/${instance.id}/operations`)
    assert.ok(!operations.some(value => ['queued', 'running', 'reconciling'].includes(value.state)), 'Wait for active management work before acceptance')
    if (instance.id === '9fa8d4c9-cf1f-4e67-a8e8-d65156589ca3') { fixture = instance; base = path }
  }
}
assert.equal(fixture?.name, 'Backup-Abnahme 2026-09-25T06:17:23.242Z')
const current = async () => (await api(base)).find(value => value.id === fixture.id)
async function operation(action) {
  const op = await api(`${base}/${fixture.id}/operations`, { action, expected_revision: (await current()).revision, idempotency_key: randomUUID() })
  for (let i = 0; i < 180; i++) {
    const row = (await api(`${base}/${fixture.id}/operations`)).find(value => value.id === op.id)
    if (['failed', 'succeeded', 'reconciling'].includes(row.state)) { assert.equal(row.state, 'succeeded', JSON.stringify(row.result)); return }
    await delay(1000)
  }
  throw new Error(`Operation ${action} did not finish`)
}
if (fixture.state === 'ready') await operation('stop')
assert.equal((await current()).state, 'stopped')
const images = JSON.parse(docker('image', 'inspect', 'cowork-runner:admission-candidate', '--format', '{{json .RepoDigests}}'))
const runner_image = images.find(value => value.startsWith('cowork-runner@sha256:'))
assert.ok(runner_image)
const requestFile = new URL('update-acceptance-request.json', privateDir)
await writeFile(requestFile, JSON.stringify({ instance: await current(), runner_image }))
docker(...compose, 'stop', 'provisioner')
try {
  await new Promise((resolve, reject) => {
    const process = spawn('docker', [...compose, 'run', '--rm', '--no-deps', '--entrypoint', 'python3',
      '--volume', `${fileURLToPath(new URL('../../server/cowork-provisioner', import.meta.url))}:/opt/cowork-provisioner:ro`,
      '--volume', `${fileURLToPath(requestFile)}:/request.json:ro`, 'provisioner', '/opt/cowork-provisioner/accept_instance_updates.py',
      '--config', `${stateRoot}/config.json`, '--request', '/request.json', '--output', `${stateRoot}/update-acceptance-results.json`], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] })
    process.stdout.pipe(globalThis.process.stdout); process.stderr.pipe(globalThis.process.stderr)
    process.once('error', reject); process.once('exit', code => code === 0 ? resolve() : reject(new Error(`Update acceptance exited ${code}`)))
  })
  const result = docker(...compose, 'run', '--rm', '--no-deps', '--entrypoint', 'cat', 'provisioner', `${stateRoot}/update-acceptance-results.json`)
  await writeFile(new URL('update-acceptance-results.json', privateDir), result)
} finally {
  docker(...compose, 'up', '-d', '--no-deps', '--no-build', 'provisioner')
}
await operation('start')
assert.equal((await current()).state, 'ready')
console.log('original-fixture-ready=ok')
