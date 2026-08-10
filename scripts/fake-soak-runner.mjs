import { createServer } from 'node:http'

const port = Number.parseInt(process.env.COWORK_SOAK_RUNNER_PORT ?? '18098', 10)
const baseDelayMs = Number.parseInt(process.env.COWORK_SOAK_RUNNER_DELAY_MS ?? '200', 10)
const runCounts = new Map()
let active = 0
let maxActive = 0

function json(response, status, value) {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(JSON.stringify(value))
}

const server = createServer((request, response) => {
  if (request.method === 'GET' && request.url === '/healthz') {
    json(response, 200, { status: 'ready' })
    return
  }
  if (request.method === 'GET' && request.url === '/stats') {
    json(response, 200, {
      jobs: [...runCounts.values()].reduce((sum, count) => sum + count, 0),
      unique_runs: runCounts.size,
      active,
      max_active: maxActive,
      run_counts: Object.fromEntries(runCounts),
    })
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
    let runId
    try { runId = JSON.parse(body).run_id } catch { /* handled below */ }
    if (typeof runId !== 'string' || !runId) {
      json(response, 400, { error: 'run_id is required' })
      return
    }

    runCounts.set(runId, (runCounts.get(runId) ?? 0) + 1)
    active += 1
    maxActive = Math.max(maxActive, active)
    const jitter = [...runId].reduce((sum, character) => sum + character.charCodeAt(0), 0) % 80
    setTimeout(() => {
      active -= 1
      if (response.destroyed || response.writableEnded) return
      json(response, 200, {
        schema_version: 2,
        run_id: runId,
        container_name: `soak-${runId}`,
        workspace_volume: `soak-${runId}`,
        exit_code: 0,
        timed_out: false,
        stdout: `completed ${runId}`,
        stderr: '',
        output_truncated: false,
      })
    }, baseDelayMs + jitter)
  })
})

server.listen(port, '127.0.0.1')

function shutdown() {
  server.closeAllConnections()
  server.close(() => process.exit(0))
}

process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
