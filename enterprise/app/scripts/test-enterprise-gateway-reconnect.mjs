// Real Docker DNS replacement: the gateway must reconnect without being restarted.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, dirname, resolve, basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import { randomUUID } from 'node:crypto'

const root = fileURLToPath(new URL('../../', import.meta.url))
const scope = `cowork-gateway-check-${randomUUID().replaceAll('-', '')}`
const directory = await mkdtemp(join(tmpdir(), 'cowork-gateway-check-'))
const image = 'cowork-web:local-review'
const names = []
const docker = (...args) => execFileSync('docker', args, { cwd: root, encoding: 'utf8', timeout: 60000, stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const inspect = name => JSON.parse(docker('inspect', name))[0]
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const launch = (name, args, command = []) => {
  names.push(name)
  return docker('run', '--detach', '--name', name, '--network', scope, '--read-only', '--tmpfs', '/tmp',
    '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', ...args, image, ...command)
}
async function expectResponse(port, expected) {
  let observed = 'unreachable'
  for (let attempt = 0; attempt < 40; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/v1/probe?value=a%2Fb`, { signal: AbortSignal.timeout(1500) })
      observed = `${response.status}:${await response.text()}`
      if (observed === `200:${expected}:/api/v1/probe?value=a%2Fb`) return
    } catch (error) { observed = error.message }
    await delay(500)
  }
  assert.fail(`Gateway did not recover: ${observed}`)
}
try {
  docker('network', 'create', '--subnet', process.env.COWORK_GATEWAY_TEST_SUBNET || '10.254.202.0/28', scope)
  for (const [backend, config] of [['management', 'management-nginx.conf'], ['workspace', 'workspace-local-nginx.conf']]) {
    const backendName = `${scope}-${backend}`
    const gatewayName = `${scope}-${backend}-gateway`
    const first = join(directory, `${backend}-first.conf`)
    const second = join(directory, `${backend}-second.conf`)
    await writeFile(first, 'server { listen 8080; location / { return 200 "first:$request_uri"; } }\n')
    await writeFile(second, 'server { listen 8080; location / { return 200 "second:$request_uri"; } }\n')
    const backendArgs = path => ['--network-alias', backend, '--mount', `type=bind,src=${path},dst=/etc/nginx/conf.d/default.conf,readonly`]
    launch(backendName, backendArgs(first))
    launch(gatewayName, ['--publish', '127.0.0.1::8080', '--mount', `type=bind,src=${join(root, 'deploy/enterprise', config)},dst=/etc/nginx/conf.d/default.conf,readonly`])
    const before = inspect(gatewayName)
    const port = before.NetworkSettings.Ports['8080/tcp'][0].HostPort
    await expectResponse(port, 'first')
    const oldAddress = inspect(backendName).NetworkSettings.Networks[scope].IPAddress
    docker('rm', '--force', backendName)
    // Occupy the old address so successful recovery cannot merely reuse it.
    launch(`${scope}-${backend}-old-address`, ['--ip', oldAddress, '--entrypoint', 'sleep'], ['120'])
    launch(backendName, backendArgs(second))
    const newAddress = inspect(backendName).NetworkSettings.Networks[scope].IPAddress
    assert.notEqual(newAddress, oldAddress)
    await expectResponse(port, 'second')
    const after = inspect(gatewayName)
    assert.equal(after.Id, before.Id)
    assert.equal(after.State.StartedAt, before.State.StartedAt)
    assert.equal(after.RestartCount, before.RestartCount)
    for (const path of ['/management/runner/claim', '/management/runner/operations']) {
      assert.equal((await fetch(`http://127.0.0.1:${port}${path}`)).status, 404)
    }
    console.log(`${backend}: changed backend IP, preserved request URI, no gateway restart, private routes denied=ok`)
  }
} finally {
  for (const name of [...new Set(names)].reverse()) {
    try { docker('rm', '--force', name) } catch { /* Only this test's random scope. */ }
  }
  try { docker('network', 'rm', scope) } catch { /* Report Docker failures above. */ }
  assert.equal(dirname(resolve(directory)), resolve(tmpdir()))
  assert.ok(basename(directory).startsWith('cowork-gateway-check-'))
  await rm(directory, { recursive: true, force: true })
}
