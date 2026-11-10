import { createServer } from 'node:http'

const port = Number.parseInt(process.env.COWORK_CHAOS_RUNNER_PORT ?? '18097', 10)
const delayMs = Number.parseInt(process.env.COWORK_CHAOS_RUNNER_DELAY_MS ?? '60000', 10)
let jobs = 0

const server = createServer((request, response) => {
  if (request.method === 'GET' && request.url === '/healthz') {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end('{"status":"ready"}')
    return
  }
  if (request.method === 'GET' && request.url === '/count') {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ jobs }))
    return
  }
  if (request.method !== 'POST' || request.url !== '/v1/jobs') {
    response.writeHead(404).end()
    return
  }
  let body = ''
  request.setEncoding('utf8')
  request.on('data', (chunk) => { body += chunk })
  request.on('end', () => {
    jobs += 1
    let runId = '00000000-0000-0000-0000-000000000000'
    try { runId = JSON.parse(body).run_id ?? runId } catch { /* worker validates first */ }
    setTimeout(() => {
      if (response.destroyed || response.writableEnded) return
      response.writeHead(200, { 'content-type': 'application/json' })
      response.end(JSON.stringify({
        schema_version: 2,
        run_id: runId,
        container_name: 'chaos-runner',
        workspace_volume: 'chaos-runner',
        exit_code: 0,
        timed_out: false,
        stdout: 'late success',
        stderr: '',
        output_truncated: false,
      }))
    }, delayMs)
  })
})

server.listen(port, '127.0.0.1')

function shutdown() {
  server.closeAllConnections()
  server.close(() => process.exit(0))
}

process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
