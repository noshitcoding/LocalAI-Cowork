// Disposable Docker Desktop acceptance. No management/tenant data is modified.
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { randomUUID, randomBytes, createHmac, createHash } from 'node:crypto'
import http from 'node:http'
import { mkdir, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const privateDir = new URL('../../.internal/enterprise-local/', import.meta.url)
await mkdir(privateDir, { recursive: true })
const instance = randomUUID()
const prefix = `cowork-${instance.replaceAll('-', '')}-`
const stateVolume = `${prefix}admission-test-state`
const network = `${prefix}admission-test-network`
const runners = [`${prefix}admission-test-a`, `${prefix}admission-test-b`]
const key = randomBytes(32).toString('hex')
const secret = new URL(`admission-test-${instance}.key`, privateDir)
await writeFile(secret, key, { mode: 0o600 })
const delayedRun = randomUUID()
const delayedContainer = `${prefix}job-${delayedRun.replaceAll('-', '')}`
const wrapper = new URL(`admission-test-${instance}-docker`, privateDir)
await writeFile(wrapper, `#!/bin/sh
if [ "$1" = "create" ]; then
  for argument in "$@"; do
    if [ "$argument" = "${delayedContainer}" ]; then
      (sleep 4; /usr/local/bin/docker "$@" >/dev/null 2>&1) </dev/null >/dev/null 2>&1 &
      echo 'injected lost Docker create response' >&2
      exit 125
    fi
  done
fi
exec /usr/local/bin/docker "$@"
`, { mode: 0o700 })
const checks = []
const pass = name => { checks.push(name); console.log(`${name}=ok`) }
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', cwd: root, timeout: 120000, stdio: ['pipe', 'pipe', 'pipe'] }).trim()
const bestEffort = (...args) => spawnSync('docker', args, { cwd: root, timeout: 30000, stdio: 'ignore' })
const runnerImage = process.env.COWORK_ADMISSION_TEST_IMAGE || 'cowork-runner:admission-candidate'
const coreImage = process.env.COWORK_ADMISSION_TEST_CORE || 'cowork-sandbox-core:local-review'
const guiImage = process.env.COWORK_ADMISSION_TEST_GUI || 'cowork-sandbox-gui:local-review'
const origins = []
const refreshOrigin = index => { origins[index] = `http://${docker('port', runners[index], '8090/tcp')}` }
const mib = 1024 * 1024
const limits = (cpu = 1, memory = 768, timeout = 30) => ({ cpu_nanos: cpu * 1e9, memory_bytes: memory * mib, pids: 128, tmpfs_bytes: 64 * mib, timeout_seconds: timeout, output_bytes: mib })
const job = (run = randomUUID(), cpu = 1, memory = 768, script = 'print("capacity-ok")') => ({ schema_version: 2, run_id: run, image: 'core', argv: ['python3', '-c', script], environment: {}, stdin_base64: null, network: 'none', limits: limits(cpu, memory) })
function signed(method, path, body = '') {
  const timestamp = Math.floor(Date.now() / 1000).toString()
  const nonce = randomUUID()
  const signature = createHmac('sha256', key).update(`${timestamp}\n${nonce}\n${method}\n${path}\n${body}`).digest('hex')
  return { 'Content-Type': 'application/json', 'x-cowork-timestamp': timestamp, 'x-cowork-nonce': nonce, 'x-cowork-signature': signature }
}
async function api(path, value, expected = 200, origin = origins[0], method = value === undefined ? 'GET' : 'POST') {
  const body = value === undefined ? '' : JSON.stringify(value)
  const response = await fetch(origin + path, { method, headers: signed(method, path, body), ...(body ? { body } : {}), signal: AbortSignal.timeout(90000) })
  const text = await response.text()
  assert.equal(response.status, expected, `${path}: ${response.status} ${text.slice(0, 500)}`)
  return text ? JSON.parse(text) : undefined
}
async function ready(origin) {
  for (let i = 0; i < 60; i++) {
    try { if ((await fetch(origin + '/healthz', { signal: AbortSignal.timeout(2000) })).ok) return } catch {}
    await delay(500)
  }
  throw new Error('runner did not become ready')
}
async function until(check) {
  for (let i = 0; i < 80; i++) { if (check()) return; await delay(150) }
  throw new Error('expected Docker state did not appear')
}
const workloadNames = () => docker('ps', '--all', '--filter', `name=^/${prefix}`, '--format', '{{.Names}}').split('\n').filter(name => ['job-', 'desktop-', 'terminal-', 'transfer-'].some(kind => name.startsWith(prefix + kind)))
const terminal = run => new Promise((resolve, reject) => {
  const path = `/v1/runs/${run}/terminal?columns=80&rows=24`
  const websocketKey = randomBytes(16).toString('base64')
  const request = http.request(origins[0] + path, { headers: { ...signed('GET', path), Connection: 'Upgrade', Upgrade: 'websocket', 'Sec-WebSocket-Key': websocketKey, 'Sec-WebSocket-Version': '13' } })
  request.once('error', reject)
  request.once('response', response => reject(new Error(`terminal upgrade returned ${response.statusCode}`)))
  request.once('upgrade', (response, socket) => {
    assert.equal(response.headers['sec-websocket-accept'], createHash('sha1').update(websocketKey + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64'))
    socket.on('error', () => {}); socket.resume()
    resolve({ close: () => socket.end(Buffer.concat([Buffer.from([0x88, 0x80]), randomBytes(4)])) })
  })
  request.end()
})
const session = (run, id = randomUUID()) => ({ schema_version: 2, session_id: id, run_id: run, network: 'filtered_egress', dimensions: { width: 1024, height: 768, scale_factor: 1 }, limits: limits(1, 1024) })
const stdinDocker = (name, path, text) => {
  assert.ok(name.startsWith(prefix) && /^\/var\/lib\/cowork\/admission\/[a-f0-9-]+\.json$/.test(path))
  execFileSync('docker', ['exec', '-i', name, 'sh', '-c', `cat > ${path}`], { input: text, stdio: ['pipe', 'pipe', 'pipe'] })
}

try {
  docker('volume', 'create', stateVolume)
  docker('network', 'create', '--internal', '--subnet', process.env.COWORK_ADMISSION_TEST_SUBNET || '10.254.250.0/28', network)
  for (const name of runners) {
    docker('run', '-d', '--name', name, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp',
      '-p', '127.0.0.1::8090', '--mount', 'type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock',
      '--mount', `type=volume,source=${stateVolume},target=/var/lib/cowork/admission`,
      '--mount', `type=bind,source=${fileURLToPath(secret)},target=/run/test-key,readonly`,
      '--mount', `type=bind,source=${fileURLToPath(wrapper)},target=/test-bin/docker,readonly`, '-e', 'PATH=/test-bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
      '-e', `COWORK_RUNNER_INSTANCE_ID=${instance}`, '-e', 'COWORK_RUNNER_SIGNING_KEY_FILE=/run/test-key',
      '-e', 'COWORK_RUNNER_ADMISSION_DIRECTORY=/var/lib/cowork/admission', '-e', 'COWORK_RUNNER_CPU_MILLIS=2000', '-e', 'COWORK_RUNNER_MEMORY_MIB=2048', '-e', 'COWORK_RUNNER_CONCURRENT_RUNS=2',
      '-e', `COWORK_RUNNER_CORE_IMAGE=${coreImage}`, '-e', `COWORK_RUNNER_GUI_IMAGE=${guiImage}`, '-e', `COWORK_RUNNER_CREW_IMAGE=${coreImage}`,
      '-e', 'COWORK_SANDBOX_REQUIRE_APPARMOR=false', '-e', `COWORK_SANDBOX_EGRESS_NETWORK=${network}`, '-e', 'COWORK_SANDBOX_HTTP_PROXY=http://127.0.0.1:9', runnerImage)
    origins.push(`http://${docker('port', name, '8090/tcp')}`)
    await ready(origins.at(-1))
  }
  const health = await (await fetch(origins[0] + '/healthz')).json()
  assert.equal(health.sandbox_capacity.cpu_nanos, 2e9)
  const unauth = await fetch(origins[0] + '/v1/jobs', { method: 'POST', body: JSON.stringify(job()) })
  assert.equal(unauth.status, 401)
  await api('/v1/jobs', job(randomUUID(), 3), 400)
  await api('/v1/jobs', job(randomUUID(), 1, 3072), 400)
  assert.equal(docker('volume', 'ls', '--filter', `name=^${prefix}run-`, '--format', '{{.Name}}'), '')
  pass('authentication-and-oversized-requests')

  // Both processes compete for the same durable scope. Admission cannot rely
  // on a mutex in one process or one HTTP worker.
  const responses = Array.from({ length: 8 }, (_, i) => {
    const spec = job(randomUUID(), 1, 768, 'import time; time.sleep(15); print("parallel-ok")')
    const body = JSON.stringify(spec)
    return fetch(origins[i % 2] + '/v1/jobs', { method: 'POST', headers: signed('POST', '/v1/jobs', body), body }).then(async response => ({ status: response.status, result: await response.json() }))
  })
  await delay(1500)
  await until(() => workloadNames().filter(name => name.startsWith(prefix + 'job-')).length === 2)
  const concurrent = await Promise.all(responses)
  assert.equal(concurrent.filter(value => value.status === 200 && value.result.exit_code === 0).length, 2)
  assert.equal(concurrent.filter(value => value.status === 429 && value.result.error === 'sandbox_capacity_exhausted').length, 6)
  assert.equal(workloadNames().length, 0)
  assert.equal(docker('volume', 'ls', '--filter', `name=^${prefix}run-`, '--format', '{{.Name}}').split('\n').length, 2)
  pass('parallel-cross-process-admission')

  const run = randomUUID()
  const stdin = job(run, 0.5, 128, 'import pathlib,sys; pathlib.Path("artifacts").mkdir(exist_ok=True); pathlib.Path("artifacts/result.txt").write_text(sys.stdin.read()); print("stdin-ok")')
  stdin.stdin_base64 = Buffer.from('admission-artifact').toString('base64')
  assert.equal((await api('/v1/jobs', stdin)).exit_code, 0)
  const filePath = `/v1/runs/${run}/file?path=artifacts%2Fresult.txt`
  const artifact = await fetch(origins[0] + filePath, { headers: signed('GET', filePath) })
  assert.equal(artifact.status, 200); assert.equal(await artifact.text(), 'admission-artifact')
  pass('stdin-and-bounded-transfer')

  const activeTerminal = await terminal(run)
  await until(() => workloadNames().some(name => name.startsWith(prefix + 'terminal-')))
  const terminalBlockedJob = job(randomUUID(), 1.5, 128)
  await api('/v1/jobs', terminalBlockedJob, 429)
  const terminalContainer = workloadNames().find(name => name.startsWith(prefix + 'terminal-'))
  const terminalInspect = JSON.parse(docker('inspect', terminalContainer))[0]
  assert.equal(terminalInspect.HostConfig.MemorySwap, 1024 * mib)
  activeTerminal.close()
  await until(() => workloadNames().length === 0)
  pass('terminal-reservation-and-disconnect-release')

  const desktop = session(randomUUID())
  const desktopResult = await api('/v1/desktop-sessions', desktop)
  const inDesktop = job(desktop.run_id, 0.5, 512); inDesktop.image = 'gui'
  assert.equal((await api('/v1/jobs', inDesktop)).container_name, desktopResult.container_name)
  await api('/v1/jobs', job(randomUUID(), 1.5, 128), 429)
  docker('restart', runners[0]); refreshOrigin(0); await ready(origins[0])
  await api('/v1/jobs', job(randomUUID(), 1.5, 128), 429)
  await api(`/v1/desktop-sessions/${desktop.session_id}`, undefined, 204, origins[0], 'DELETE')
  pass('persistent-desktop-and-runner-restart')

  const timed = job(randomUUID(), 1, 128, 'import time; time.sleep(20)'); timed.limits.timeout_seconds = 1
  assert.equal((await api('/v1/jobs', timed)).timed_out, true)
  assert.equal((await api('/v1/jobs', job(randomUUID(), 2, 2048))).exit_code, 0)
  pass('timeout-stops-container-before-capacity-release')

  const crashSpec = job(randomUUID(), 1, 128, 'import time; time.sleep(60)'); crashSpec.limits.timeout_seconds = 90
  const crashedRequest = api('/v1/jobs', crashSpec).catch(() => undefined)
  const crashContainer = `${prefix}job-${crashSpec.run_id.replaceAll('-', '')}`
  await delay(100)
  await until(() => workloadNames().includes(crashContainer))
  docker('kill', runners[0]); await crashedRequest
  docker('start', runners[0]); refreshOrigin(0); await ready(origins[0])
  await api('/v1/jobs', job(randomUUID(), 2, 128), 429)
  docker('rm', '--force', crashContainer)
  assert.equal((await api('/v1/jobs', job(randomUUID(), 2, 128))).exit_code, 0)
  pass('abrupt-runner-death-retains-live-workload-reservation')

  const uncertainId = randomUUID()
  const uncertainPath = `/var/lib/cowork/admission/${uncertainId}.json`
  stdinDocker(runners[0], uncertainPath, JSON.stringify({ id: uncertainId, container_name: `${prefix}job-unproven-test`, run_id: randomUUID(), resources: { cpu_nanos: 2e9, memory_bytes: 2048 * mib }, container_id: null }))
  docker('restart', runners[0]); refreshOrigin(0); await ready(origins[0])
  await api('/v1/jobs', job(), 429)
  await api('/v1/jobs', job(), 429, origins[1])
  // This fixture never issued a Docker create call. Remove only its injected
  // reservation; production ambiguous launches require operator investigation.
  docker('exec', runners[0], 'rm', uncertainPath)
  const corruptPath = `/var/lib/cowork/admission/${randomUUID()}.json`
  stdinDocker(runners[0], corruptPath, '{broken')
  await api('/v1/jobs', job(), 503)
  docker('exec', runners[0], 'rm', corruptPath)
  pass('unproven-and-corrupt-state-fails-closed')

  // The CLI returns failure before a delayed daemon-side creation appears.
  // Failed transport is not proof that Docker did not accept the request.
  await api('/v1/jobs', job(delayedRun, 2, 2048), 500)
  await api('/v1/jobs', job(), 429, origins[1])
  await until(() => workloadNames().includes(delayedContainer))
  assert.equal(JSON.parse(docker('inspect', delayedContainer))[0].State.Status, 'created')
  await api('/v1/jobs', job(), 429)
  docker('rm', delayedContainer)
  assert.equal((await api('/v1/jobs', job(randomUUID(), 2, 2048))).exit_code, 0)
  pass('lost-create-response-with-late-container-holds-capacity')

  const legacyName = `${prefix}job-legacy-test`
  docker('run', '-d', '--rm', '--name', legacyName, '--network', 'none', '--cpus', '1', '--memory', '128m', '--label', `dev.opencowork.run_id=${randomUUID()}`, coreImage, 'sleep', '60')
  await api('/v1/jobs', job(randomUUID(), 2, 128), 429)
  docker('rm', '--force', legacyName)
  assert.equal((await api('/v1/jobs', job(randomUUID(), 2, 2048))).exit_code, 0)
  pass('preexisting-containers-cannot-bypass-accounting')
  await writeFile(new URL('admission-test-results.json', privateDir), JSON.stringify({ date: new Date().toISOString(), instance, runner_image: docker('image', 'inspect', runnerImage, '--format', '{{.Id}}'), checks }, null, 2))
} catch (error) {
  const logs = runners.map(name => ({ name, ...spawnSync('docker', ['logs', '--tail', '100', name], { encoding: 'utf8', timeout: 10000 }) }))
  await writeFile(new URL('admission-test-failure.json', privateDir), JSON.stringify({ date: new Date().toISOString(), checks, error: String(error), logs }, null, 2))
  throw error
} finally {
  for (const name of runners) bestEffort('rm', '--force', name)
  // All names must be within this newly generated disposable scope.
  for (const name of workloadNames()) { assert.ok(name.startsWith(prefix)); bestEffort('rm', '--force', name) }
  for (const name of docker('volume', 'ls', '--filter', `name=^${prefix}`, '--format', '{{.Name}}').split('\n').filter(Boolean)) {
    assert.ok(name.startsWith(prefix)); bestEffort('volume', 'rm', name)
  }
  bestEffort('network', 'rm', network)
}
