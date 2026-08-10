import { randomUUID } from 'node:crypto'

const apiBase = process.env.COWORK_TEST_API_URL ?? 'http://127.0.0.1:18099/api/v1'
const runnerBase = process.env.COWORK_TEST_RUNNER_URL ?? 'http://127.0.0.1:18098'
const bootstrapToken = process.env.COWORK_TEST_BOOTSTRAP_TOKEN
const waves = Number.parseInt(process.env.COWORK_TEST_SOAK_WAVES ?? '5', 10)
const runsPerWave = Number.parseInt(process.env.COWORK_TEST_RUNS_PER_WAVE ?? '16', 10)
const expectedWorkers = Number.parseInt(process.env.COWORK_TEST_WORKER_COUNT ?? '4', 10)

if (!bootstrapToken) throw new Error('COWORK_TEST_BOOTSTRAP_TOKEN is required')
if (!Number.isInteger(waves) || waves < 2 || waves > 20) throw new Error('invalid soak wave count')
if (!Number.isInteger(runsPerWave) || runsPerWave < 4 || runsPerWave > 100) throw new Error('invalid runs per wave')
if (!Number.isInteger(expectedWorkers) || expectedWorkers < 2 || expectedWorkers > 16) throw new Error('invalid worker count')

async function request(path, { method = 'GET', token, body } = {}) {
  const response = await fetch(`${apiBase}${path}`, {
    method,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  if (!response.ok) throw new Error(`${method} ${path} returned ${response.status}: ${text}`)
  return text ? JSON.parse(text) : null
}

function sandboxInput(label) {
  return {
    sandbox: {
      schema_version: 2,
      run_id: '00000000-0000-0000-0000-000000000000',
      image: 'core',
      argv: ['/bin/sh', '-lc', `printf '%s' '${label}'`],
      environment: {},
      stdin_base64: null,
      network: 'none',
      limits: {
        memory_bytes: 268435456,
        cpu_nanos: 500000000,
        pids: 32,
        timeout_seconds: 30,
        tmpfs_bytes: 67108864,
        output_bytes: 1048576,
      },
    },
  }
}

async function waitForWave(ids, token) {
  const deadline = Date.now() + 45_000
  let records = []
  do {
    records = await Promise.all(ids.map((id) => request(`/runs/${id}`, { token })))
    const failed = records.find((run) => ['failed', 'canceled', 'expired', 'interrupted'].includes(run.state))
    if (failed) throw new Error(`run ${failed.spec.id} terminated as ${failed.state}: ${JSON.stringify(failed.error)}`)
    if (records.every((run) => run.state === 'completed')) return records
    await new Promise((resolve) => setTimeout(resolve, 100))
  } while (Date.now() < deadline)
  throw new Error(`wave timed out: ${records.map((run) => `${run.spec.id}:${run.state}`).join(', ')}`)
}

const session = await request('/auth/bootstrap', {
  method: 'POST',
  token: bootstrapToken,
  body: {
    email: 'worker-soak@opencowork.invalid',
    display_name: 'Worker Soak',
    password: 'Worker-Soak-Password-42!',
    device_id: randomUUID(),
  },
})
const token = session.access_token
const team = await request('/teams', { method: 'POST', token, body: { name: 'Worker soak team' } })
const project = await request('/projects', {
  method: 'POST',
  token,
  body: {
    name: 'Worker soak project',
    description: '',
    privacy: 'team_managed',
    team_id: team.id,
    preferred_executor_target: { kind: 'server_linux', pool_id: null },
    policy: { tool_policy: 'autonomous' },
  },
})

const completed = []
for (let wave = 0; wave < waves; wave += 1) {
  const created = await Promise.all(Array.from({ length: runsPerWave }, async (_, index) => {
    const label = `wave-${wave}-run-${index}`
    const thread = await request('/threads', {
      method: 'POST',
      token,
      body: {
        project_id: project.id,
        title: label,
        forked_from_thread_id: null,
        forked_from_message_id: null,
      },
    })
    return request('/runs', {
      method: 'POST',
      token,
      body: {
        thread_id: thread.id,
        project_id: project.id,
        project_revision: project.revision,
        project_privacy: 'team_managed',
        task: null,
        executor_target: { kind: 'server_linux', pool_id: null },
        required_capabilities: [],
        input: sandboxInput(label),
        model_profile_id: null,
        snapshot_id: null,
        idempotency_key: `worker-soak-${randomUUID()}`,
      },
    })
  }))
  completed.push(...await waitForWave(created.map((run) => run.spec.id), token))
}

const statsResponse = await fetch(`${runnerBase}/stats`)
if (!statsResponse.ok) throw new Error(`runner stats returned ${statsResponse.status}`)
const stats = await statsResponse.json()
const runIds = new Set(completed.map((run) => run.spec.id))
const assignedWorkers = new Set(completed.map((run) => run.assigned_executor_id).filter(Boolean))
const duplicates = Object.entries(stats.run_counts).filter(([, count]) => count !== 1)
const unexpected = Object.keys(stats.run_counts).filter((id) => !runIds.has(id))

if (stats.jobs !== runIds.size || stats.unique_runs !== runIds.size) {
  throw new Error(`runner observed ${stats.jobs}/${stats.unique_runs} jobs for ${runIds.size} runs`)
}
if (duplicates.length || unexpected.length) {
  throw new Error(`runner dispatch was not exactly once: duplicates=${JSON.stringify(duplicates)} unexpected=${JSON.stringify(unexpected)}`)
}
if (assignedWorkers.size !== expectedWorkers) {
  throw new Error(`expected all ${expectedWorkers} workers to claim work, observed ${assignedWorkers.size}`)
}
if (stats.max_active < expectedWorkers) {
  throw new Error(`expected ${expectedWorkers} concurrent runner jobs, observed ${stats.max_active}`)
}
if (completed.some((run) => !String(run.result?.stdout ?? '').includes(run.spec.id))) {
  throw new Error('one or more completed runs received another run\'s result')
}

console.log(`waves=${waves}`)
console.log(`completed_runs=${completed.length}`)
console.log(`distinct_workers=${assignedWorkers.size}`)
console.log(`runner_max_concurrency=${stats.max_active}`)
console.log('exactly_once_dispatch=ok')
console.log('cross_run_result_isolation=ok')
