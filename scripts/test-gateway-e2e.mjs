import { createHash, randomBytes } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { chmodSync, mkdtempSync, rmSync } from 'node:fs'
import { createServer } from 'node:http'
import { connect } from 'node:tls'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const repo = resolve(import.meta.dirname, '..')
const compose = join(repo, 'deploy', 'gateway-e2e', 'docker-compose.yml')
const certificateRoot = mkdtempSync(join(tmpdir(), 'open-cowork-gateway-e2e-'))
const composeEnvironment = { ...process.env, COWORK_GATEWAY_E2E_CERT_ROOT: certificateRoot, COWORK_GATEWAY_E2E_PORT: '18443' }
const framePayload = Buffer.from('gui-frame-v1')
const uploadSize = 20 * 1024 * 1024
const upstreamSockets = new Set()

function command(command, args, options = {}) {
  return execFileSync(command, args, { cwd: repo, env: composeEnvironment, encoding: 'utf8', stdio: options.capture ? 'pipe' : 'inherit' })
}

function websocketAccept(key) {
  return createHash('sha1').update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64')
}

const upstream = createServer((request, response) => {
  if (request.url === '/healthz' || request.url === '/readyz') {
    response.writeHead(200, { 'content-type': 'application/json' }); response.end('{"status":"ready"}'); return
  }
  if (request.url === '/api/v1/events') {
    response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
    response.end('event: run\nid: 42\ndata: {"state":"running"}\n\n'); return
  }
  if (request.url === '/api/v1/upload' && request.method === 'PUT') {
    let bytes = 0
    request.on('data', (chunk) => { bytes += chunk.length })
    request.on('end', () => { response.writeHead(200, { 'content-type': 'application/json' }); response.end(JSON.stringify({ bytes })) })
    return
  }
  response.writeHead(200, { 'content-type': 'text/html' }); response.end('<!doctype html><title>Open Cowork gateway E2E</title>')
})
upstream.on('connection', (socket) => {
  upstreamSockets.add(socket)
  socket.once('close', () => upstreamSockets.delete(socket))
})
upstream.on('upgrade', (request, socket) => {
  if (request.url !== '/api/v1/gui-stream' || !request.headers['sec-websocket-key']) { socket.destroy(); return }
  socket.write([
    'HTTP/1.1 101 Switching Protocols', 'Upgrade: websocket', 'Connection: Upgrade',
    `Sec-WebSocket-Accept: ${websocketAccept(request.headers['sec-websocket-key'])}`, '', '',
  ].join('\r\n'))
  socket.write(Buffer.concat([Buffer.from([0x82, framePayload.length]), framePayload]))
})

function websocketFrame() {
  return new Promise((resolveFrame, reject) => {
    const key = randomBytes(16).toString('base64')
    const socket = connect({ host: '127.0.0.1', port: 18443, servername: 'cowork.test', rejectUnauthorized: false })
    let buffer = Buffer.alloc(0); let headersRead = false
    const timeout = setTimeout(() => { socket.destroy(); reject(new Error('WebSocket proxy timed out')) }, 10_000)
    socket.on('secureConnect', () => socket.write([
      'GET /api/v1/gui-stream HTTP/1.1', 'Host: cowork.test', 'Upgrade: websocket',
      'Connection: Upgrade', `Sec-WebSocket-Key: ${key}`, 'Sec-WebSocket-Version: 13', '', '',
    ].join('\r\n')))
    socket.on('data', (chunk) => {
      buffer = Buffer.concat([buffer, chunk])
      if (!headersRead) {
        const end = buffer.indexOf('\r\n\r\n')
        if (end < 0) return
        const headers = buffer.subarray(0, end).toString('ascii')
        if (!headers.startsWith('HTTP/1.1 101')) { clearTimeout(timeout); socket.destroy(); reject(new Error(`WebSocket upgrade failed: ${headers}`)); return }
        buffer = buffer.subarray(end + 4); headersRead = true
      }
      if (buffer.length < 2) return
      const length = buffer[1] & 0x7f
      if ((buffer[0] & 0x0f) !== 2 || buffer.length < 2 + length) return
      clearTimeout(timeout); socket.end(); resolveFrame(buffer.subarray(2, 2 + length))
    })
    socket.on('error', reject)
  })
}

async function waitReady() {
  let lastFailure = 'no response'
  for (let attempt = 0; attempt < 80; attempt++) {
    try {
      const response = await fetch('https://127.0.0.1:18443/readyz', { headers: { host: 'cowork.test' } })
      if (response.ok) return
      lastFailure = `HTTP ${response.status}: ${await response.text()}`
    } catch (error) {
      lastFailure = error instanceof Error ? `${error.message}${error.cause ? ` (${error.cause})` : ''}` : String(error)
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 250))
  }
  throw new Error(`TLS proxy chain did not become ready: ${lastFailure}`)
}

process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0'
await new Promise((resolveListen, reject) => upstream.listen(18100, '0.0.0.0', resolveListen).once('error', reject))
let succeeded = false
try {
  command('openssl', ['req', '-config', process.platform === 'win32' ? 'NUL' : '/dev/null', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=cowork.test', '-addext', 'subjectAltName=DNS:cowork.test', '-keyout', join(certificateRoot, 'server.key'), '-out', join(certificateRoot, 'server.crt')], { capture: true })
  chmodSync(certificateRoot, 0o555)
  chmodSync(join(certificateRoot, 'server.key'), 0o444)
  chmodSync(join(certificateRoot, 'server.crt'), 0o444)
  command('docker', ['compose', '-f', compose, 'up', '-d'])
  await waitReady()

  const web = await fetch('https://127.0.0.1:18443/', { headers: { host: 'cowork.test' } })
  if (!web.ok || !(await web.text()).includes('Open Cowork gateway E2E')) throw new Error('Web app did not traverse Nginx and Caddy')
  const csp = web.headers.get('content-security-policy') ?? ''
  if (!csp.includes("default-src 'self'") || !csp.includes("object-src 'none'") || !csp.includes("frame-ancestors 'none'")) {
    throw new Error(`Gateway did not apply the fail-closed web CSP: ${csp}`)
  }
  if (web.headers.get('strict-transport-security') !== 'max-age=31536000'
    || web.headers.get('x-frame-options') !== 'DENY'
    || web.headers.get('cross-origin-opener-policy') !== 'same-origin'
    || web.headers.get('cross-origin-resource-policy') !== 'same-origin') {
    throw new Error('Gateway did not apply the required browser isolation headers')
  }
  const sse = await fetch('https://127.0.0.1:18443/api/v1/events', { headers: { host: 'cowork.test', accept: 'text/event-stream' } })
  const sseText = await sse.text()
  if (!sseText.includes('event: run') || !sseText.includes('id: 42')) throw new Error('SSE frame was changed by the proxy chain')
  const upload = await fetch('https://127.0.0.1:18443/api/v1/upload', { method: 'PUT', headers: { host: 'cowork.test', 'content-type': 'application/octet-stream' }, body: Buffer.alloc(uploadSize, 0x5a) })
  const uploadResult = await upload.json()
  if (uploadResult.bytes !== uploadSize) throw new Error(`Large upload was truncated to ${uploadResult.bytes}`)
  const receivedFrame = await websocketFrame()
  if (!receivedFrame.equals(framePayload)) throw new Error('Binary GUI WebSocket frame was changed')

  const proxyPorts = JSON.parse(command('docker', ['inspect', 'open-cowork-gateway-e2e-npm-proxy-1', '--format', '{{json .HostConfig.PortBindings}}'], { capture: true }))
  const caddyPorts = JSON.parse(command('docker', ['inspect', 'open-cowork-gateway-e2e-caddy-1', '--format', '{{json .HostConfig.PortBindings}}'], { capture: true }))
  if (Object.keys(proxyPorts ?? {}).length !== 1 || Object.keys(caddyPorts ?? {}).length !== 0) throw new Error('Gateway E2E exposed more than the single TLS proxy port')
  console.log('single_tls_port=ok')
  console.log('web_through_npm_caddy=ok')
  console.log('browser_security_headers=ok')
  console.log('sse_through_npm_caddy=ok')
  console.log('binary_gui_websocket_through_npm_caddy=ok')
  console.log(`large_upload_bytes=${uploadResult.bytes}`)
  succeeded = true
} finally {
  if (!succeeded) {
    try { command('docker', ['compose', '-f', compose, 'ps', '--all'], { capture: false }) } catch { /* diagnostics only */ }
    try { command('docker', ['port', 'open-cowork-gateway-e2e-npm-proxy-1'], { capture: false }) } catch { /* diagnostics only */ }
    try { command('docker', ['compose', '-f', compose, 'logs', '--no-color'], { capture: false }) } catch { /* diagnostics only */ }
  }
  try { command('docker', ['compose', '-f', compose, 'down', '--volumes', '--remove-orphans'], { capture: true }) } catch { /* retain primary failure */ }
  for (const socket of upstreamSockets) socket.destroy()
  await new Promise((resolveClose) => upstream.close(resolveClose))
  try { chmodSync(certificateRoot, 0o700) } catch { /* best-effort temporary cleanup */ }
  rmSync(certificateRoot, { recursive: true, force: true })
}
