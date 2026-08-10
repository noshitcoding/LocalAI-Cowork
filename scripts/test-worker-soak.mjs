import { randomUUID } from 'node:crypto'
import { spawn } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { createConnection } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

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

async function firstSyncEvent(token, after = 0) {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(new Error('sync SSE timed out')), 10_000)
  try {
    const response = await fetch(`${apiBase}/sync/events`, {
      headers: {
        authorization: `Bearer ${token}`,
        accept: 'text/event-stream',
        'last-event-id': String(after),
      },
      signal: controller.signal,
    })
    if (!response.ok || !response.body) {
      throw new Error(`sync SSE returned ${response.status}`)
    }
    const reader = response.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ''
    while (true) {
      const { value, done } = await reader.read()
      if (done) throw new Error('sync SSE ended before an event arrived')
      buffer += decoder.decode(value, { stream: true }).replaceAll('\r\n', '\n')
      let boundary
      while ((boundary = buffer.indexOf('\n\n')) >= 0) {
        const frame = buffer.slice(0, boundary)
        buffer = buffer.slice(boundary + 2)
        const data = frame.split('\n')
          .filter((line) => line.startsWith('data:'))
          .map((line) => line.slice(5).trimStart())
          .join('\n')
        if (data && data !== 'keep-alive') return JSON.parse(data)
      }
    }
  } finally {
    clearTimeout(timeout)
    controller.abort()
  }
}

async function waitUntil(label, probe, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  let lastError
  while (Date.now() < deadline) {
    try {
      const value = await probe()
      if (value) return value
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`${label} timed out${lastError ? `: ${lastError}` : ''}`)
}

function daemonCall(socketPath, token, method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = randomUUID()
    const socket = createConnection(socketPath)
    let buffer = ''
    socket.setEncoding('utf8')
    socket.setTimeout(5_000)
    socket.on('connect', () => socket.write(`${JSON.stringify({ id, token, method, params })}\n`))
    socket.on('data', (chunk) => {
      buffer += chunk
      const boundary = buffer.indexOf('\n')
      if (boundary < 0) return
      socket.end()
      try {
        const response = JSON.parse(buffer.slice(0, boundary))
        if (response.id !== id) throw new Error('daemon IPC response ID mismatch')
        if (response.error) throw new Error(`${response.error.code}: ${response.error.message}`)
        resolve(response.result)
      } catch (error) {
        reject(error)
      }
    })
    socket.on('timeout', () => socket.destroy(new Error('daemon IPC timed out')))
    socket.on('error', reject)
  })
}

async function stopChild(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return
  child.kill('SIGTERM')
  await Promise.race([
    new Promise((resolve) => child.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 3_000)),
  ])
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL')
}

async function exerciseBackgroundMetadataSync({ token, userId, executorId, credentialToken, serverEntityId }) {
  const root = await mkdtemp(join(tmpdir(), 'cowork-metadata-sync-'))
  const socketPath = join(root, 'daemon.sock')
  const ipcToken = `sync-ipc-${randomUUID()}`
  const logs = { daemon: '', agent: '' }
  let daemon
  let agent
  const capture = (child, name) => {
    child.stdout.on('data', (chunk) => { logs[name] = `${logs[name]}${chunk}`.slice(-20_000) })
    child.stderr.on('data', (chunk) => { logs[name] = `${logs[name]}${chunk}`.slice(-20_000) })
  }
  try {
    daemon = spawn('target/debug/cowork-local-daemon', [], {
      env: {
        ...process.env,
        COWORK_DAEMON_DATA_DIR: join(root, 'daemon-data'),
        COWORK_DAEMON_IPC_ENDPOINT: socketPath,
        COWORK_DAEMON_IPC_TOKEN: ipcToken,
        COWORK_DAEMON_DEVICE_ID: executorId,
        COWORK_DAEMON_USER_ID: userId,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    capture(daemon, 'daemon')
    await waitUntil('local daemon startup', () => daemonCall(socketPath, ipcToken, 'health'))
    const localEntityId = randomUUID()
    await daemonCall(socketPath, ipcToken, 'entities.upsert', {
      entity_type: 'memory',
      id: localEntityId,
      payload: { content: 'Created offline before the background agent connected', scope: 'user' },
      expected_revision: 0,
    })
    agent = spawn('target/debug/cowork-device-agent', [], {
      env: {
        ...process.env,
        COWORK_SERVER_URL: apiBase.replace(/\/api\/v1$/, ''),
        COWORK_AGENT_TOKEN: credentialToken,
        COWORK_EXECUTOR_ID: executorId,
        COWORK_AGENT_KIND: 'personal_device',
        COWORK_EXECUTOR_NAME: 'Metadata sync soak device',
        COWORK_AGENT_CAPABILITIES: 'files',
        COWORK_PERSONAL_REMOTE_CONTROL: 'off',
        COWORK_LOCAL_DAEMON_IPC_ENDPOINT: socketPath,
        COWORK_LOCAL_DAEMON_IPC_TOKEN: ipcToken,
        COWORK_AGENT_WORKSPACE_ROOT: join(root, 'workspaces'),
        COWORK_AGENT_POLL_MS: '100',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    capture(agent, 'agent')
    await waitUntil('daemon outbox drain', async () => {
      const snapshot = await request('/sync/entities/memory?limit=1000', { token })
      return snapshot.items.some((item) => item.entity_id === localEntityId)
    })
    await waitUntil('daemon inbox apply', async () => {
      const entities = await daemonCall(socketPath, ipcToken, 'entities.list', {
        entity_type: 'skill', include_tombstones: true,
      })
      return entities.some((item) => item.id === serverEntityId)
    })
    const peerId = `${apiBase.replace(/\/api\/v1$/, '')}#${executorId}`
    const state = await daemonCall(socketPath, ipcToken, 'sync.state', { peer_id: peerId })
    const conflicts = await daemonCall(socketPath, ipcToken, 'sync.conflicts', { peer_id: peerId })
    if (state.local_cursor < 1 || state.remote_cursor < 1 || conflicts.length !== 0) {
      throw new Error(`background sync cursors/conflicts are invalid: ${JSON.stringify({ state, conflicts })}`)
    }
  } catch (error) {
    throw new Error(`${error}\ndaemon log:\n${logs.daemon}\nagent log:\n${logs.agent}`)
  } finally {
    await stopChild(agent)
    await stopChild(daemon)
    await rm(root, { recursive: true, force: true })
  }
}

const soakDeviceId = randomUUID()
const session = await request('/auth/bootstrap', {
  method: 'POST',
  token: bootstrapToken,
  body: {
    email: 'worker-soak@opencowork.invalid',
    display_name: 'Worker Soak',
    password: 'Worker-Soak-Password-42!',
    device_id: soakDeviceId,
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
let retriedMessageRun = false
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
    const body = {
      content: { text: label },
      run: {
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
    }
    const pair = await request(`/threads/${thread.id}/messages`, {
      method: 'POST',
      token,
      body,
    })
    if (wave === 0 && index === 0) {
      const retry = await request(`/threads/${thread.id}/messages`, { method: 'POST', token, body })
      if (retry.message.id !== pair.message.id || retry.run.spec.id !== pair.run.spec.id) {
        throw new Error('message/run idempotency retry returned a different durable pair')
      }
      retriedMessageRun = true
    }
    return { thread, label, pair }
  }))
  const waveRuns = await waitForWave(created.map(({ pair }) => pair.run.spec.id), token)
  const completedById = new Map(waveRuns.map((run) => [run.spec.id, run]))
  const threadMessages = await Promise.all(created.map(({ thread }) => (
    request(`/threads/${thread.id}/messages?limit=10`, { token })
  )))
  for (let index = 0; index < created.length; index += 1) {
    const { label, pair } = created[index]
    const messages = threadMessages[index]
    const run = completedById.get(pair.run.spec.id)
    if (!run) throw new Error(`completed run ${pair.run.spec.id} was not returned`)
    if (messages.length !== 2 || messages[0].role !== 'user' || messages[1].role !== 'assistant') {
      throw new Error(`thread ${pair.message.thread_id} did not contain one user/assistant pair: ${JSON.stringify(messages)}`)
    }
    if (messages.some((message) => message.run_id !== run.spec.id)) {
      throw new Error(`thread ${pair.message.thread_id} contains a message linked to the wrong run`)
    }
    if (messages[0].content?.text !== label) {
      throw new Error(`thread ${pair.message.thread_id} lost its submitted user content`)
    }
    if (messages[1].content?.sandbox?.run_id !== run.spec.id) {
      throw new Error(`thread ${pair.message.thread_id} lost or crossed its assistant result`)
    }
  }
  completed.push(...waveRuns)
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
const mismatchedResults = completed
  .filter((run) => (
    run.result?.sandbox?.run_id !== run.spec.id
    || !String(run.result?.sandbox?.stdout ?? '').includes(run.spec.id)
  ))
  .map((run) => ({
    run_id: run.spec.id,
    result_run_id: run.result?.sandbox?.run_id ?? null,
    stdout: run.result?.sandbox?.stdout ?? null,
  }))
if (mismatchedResults.length) {
  throw new Error(`one or more completed runs received another run's result: ${JSON.stringify(mismatchedResults)}`)
}
if (!retriedMessageRun) throw new Error('message/run idempotency retry was not exercised')

const syncEntityId = randomUUID()
const syncTimestamp = new Date().toISOString()
const syncUpsert = {
  schema_version: 2,
  operation_id: randomUUID(),
  device_id: soakDeviceId,
  entity_type: 'memory',
  entity_id: syncEntityId,
  base_revision: 0,
  operation: 'upsert',
  payload: { text: 'worker soak metadata' },
  client_timestamp: syncTimestamp,
}
const firstSync = await request('/sync/changes', {
  method: 'POST', token, body: { changes: [syncUpsert] },
})
const replayedSync = await request('/sync/changes', {
  method: 'POST', token, body: { changes: [syncUpsert] },
})
if (JSON.stringify(firstSync) !== JSON.stringify(replayedSync)
    || firstSync.results?.[0]?.status !== 'applied'
    || firstSync.results?.[0]?.entity?.revision !== 1) {
  throw new Error(`sync operation replay was not idempotent: ${JSON.stringify({ firstSync, replayedSync })}`)
}
const conflictSync = await request('/sync/changes', {
  method: 'POST',
  token,
  body: {
    changes: [{
      ...syncUpsert,
      operation_id: randomUUID(),
      payload: { text: 'stale writer' },
    }],
  },
})
if (conflictSync.results?.[0]?.status !== 'conflict'
    || conflictSync.results?.[0]?.entity?.revision !== 1) {
  throw new Error(`stale sync writer did not receive the current entity: ${JSON.stringify(conflictSync)}`)
}
const deleteSync = await request('/sync/changes', {
  method: 'POST',
  token,
  body: {
    changes: [{
      ...syncUpsert,
      operation_id: randomUUID(),
      base_revision: 1,
      operation: 'delete',
      payload: null,
    }],
  },
})
if (deleteSync.results?.[0]?.status !== 'applied'
    || deleteSync.results?.[0]?.entity?.revision !== 2
    || deleteSync.results?.[0]?.entity?.tombstone !== true) {
  throw new Error(`sync tombstone was not persisted: ${JSON.stringify(deleteSync)}`)
}
const pulledSync = await request('/sync/changes?after=0&limit=10', { token })
const entityChanges = pulledSync.changes?.filter((change) => change.entity_id === syncEntityId) ?? []
if (entityChanges.length !== 2
    || entityChanges[0]?.operation !== 'upsert'
    || entityChanges[1]?.operation !== 'delete'
    || pulledSync.next_cursor < entityChanges[1].cursor) {
  throw new Error(`sync cursor feed is incomplete or unordered: ${JSON.stringify(pulledSync)}`)
}
const streamedSync = await firstSyncEvent(token, 0)
if (streamedSync.entity_id !== syncEntityId || streamedSync.cursor !== entityChanges[0].cursor) {
  throw new Error(`sync SSE did not resume from its durable cursor: ${JSON.stringify(streamedSync)}`)
}
const syncSnapshot = await request('/sync/entities/memory?limit=10', { token })
const syncedEntity = syncSnapshot.items?.find((entity) => entity.entity_id === syncEntityId)
if (!syncedEntity?.tombstone || syncedEntity.revision !== 2
    || syncSnapshot.watermark_cursor < entityChanges[1].cursor) {
  throw new Error(`sync bootstrap snapshot is stale or incomplete: ${JSON.stringify(syncSnapshot)}`)
}

const syncedProjectId = randomUUID()
const syncedThreadId = randomUUID()
const syncedMessageId = randomUUID()
const syncChange = (entityType, entityId, baseRevision, operation, payload) => ({
  schema_version: 2,
  operation_id: randomUUID(),
  device_id: soakDeviceId,
  entity_type: entityType,
  entity_id: entityId,
  base_revision: baseRevision,
  operation,
  payload,
  client_timestamp: new Date().toISOString(),
})
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('thread', syncedThreadId, 0, 'upsert', {
    title: 'Synced offline thread',
    source: 'desktop',
  })] },
})
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('message', syncedMessageId, 0, 'upsert', {
    thread_id: syncedThreadId,
    role: 'user',
    content: 'Materialized after its project arrives',
    timestamp: Date.now(),
    source: 'desktop',
  })] },
})
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('project', syncedProjectId, 0, 'upsert', {
    title: 'Synced offline project',
    instructions: 'Private files stay on the personal device.',
    project_kind: 'private',
    files_location: 'personal_device',
    thread_ids: [syncedThreadId],
  })] },
})
const materializedProjects = await request('/projects', { token })
const materializedProject = materializedProjects.find((item) => item.id === syncedProjectId)
if (materializedProject?.privacy !== 'private_local'
    || materializedProject.name !== 'Synced offline project') {
  throw new Error(`synced project was not safely materialized: ${JSON.stringify(materializedProject)}`)
}
const materializedThreads = await request(`/projects/${syncedProjectId}/threads`, { token })
if (materializedThreads.length !== 1 || materializedThreads[0]?.id !== syncedThreadId) {
  throw new Error(`out-of-order synced thread did not converge: ${JSON.stringify(materializedThreads)}`)
}
const materializedMessages = await request(`/threads/${syncedThreadId}/messages?limit=10`, { token })
if (materializedMessages.length !== 1
    || materializedMessages[0]?.id !== syncedMessageId
    || materializedMessages[0]?.content?.content !== 'Materialized after its project arrives') {
  throw new Error(`out-of-order synced message did not converge: ${JSON.stringify(materializedMessages)}`)
}
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('project', syncedProjectId, 1, 'upsert', {
    title: 'Synced offline project',
    instructions: 'Private files stay on the personal device.',
    project_kind: 'private',
    files_location: 'personal_device',
    thread_ids: [],
  })] },
})
const detachedThreads = await request(`/projects/${syncedProjectId}/threads`, { token })
if (detachedThreads.length !== 0) {
  throw new Error(`detached synced thread remained visible: ${JSON.stringify(detachedThreads)}`)
}
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('project', syncedProjectId, 2, 'upsert', {
    title: 'Synced offline project',
    instructions: 'Private files stay on the personal device.',
    project_kind: 'private',
    files_location: 'personal_device',
    thread_ids: [syncedThreadId],
  })] },
})
const restoredThreads = await request(`/projects/${syncedProjectId}/threads`, { token })
if (restoredThreads.length !== 1 || restoredThreads[0]?.id !== syncedThreadId) {
  throw new Error(`reattached synced thread was not restored: ${JSON.stringify(restoredThreads)}`)
}
await request('/sync/changes', {
  method: 'POST', token, body: {
    changes: [syncChange('message', syncedMessageId, 1, 'delete', null)],
  },
})
const messagesAfterTombstone = await request(`/threads/${syncedThreadId}/messages?limit=10`, { token })
if (messagesAfterTombstone.length !== 0) {
  throw new Error(`synced message tombstone was not materialized: ${JSON.stringify(messagesAfterTombstone)}`)
}
const beforeServerProjection = await request('/sync/changes?after=0&limit=1000', { token })
const serverThread = await request('/threads', {
  method: 'POST',
  token,
  body: {
    project_id: syncedProjectId,
    title: 'Created from the web control plane',
    forked_from_thread_id: null,
    forked_from_message_id: null,
  },
})
const latestPrivateProject = (await request('/projects', { token }))
  .find((item) => item.id === syncedProjectId)
if (!latestPrivateProject) throw new Error('materialized private project disappeared before reverse projection')
const serverMessageRun = await request(`/threads/${serverThread.id}/messages`, {
  method: 'POST',
  token,
  body: {
    content: { text: 'Server-originated message for the desktop inbox' },
    run: {
      thread_id: serverThread.id,
      project_id: syncedProjectId,
      project_revision: latestPrivateProject.revision,
      project_privacy: 'private_local',
      task: null,
      executor_target: { kind: 'server_linux', pool_id: null },
      required_capabilities: [],
      input: sandboxInput('server-originated-private-message'),
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `server-projection-${randomUUID()}`,
    },
  },
})
if (serverMessageRun.run.state !== 'waiting_for_snapshot') {
  throw new Error(`private server run did not wait for its explicit snapshot: ${JSON.stringify(serverMessageRun.run)}`)
}
const serverProjection = await request(
  `/sync/changes?after=${beforeServerProjection.next_cursor}&limit=100`,
  { token },
)
const projectedIds = new Set(serverProjection.changes.map((change) => change.entity_id))
if (!projectedIds.has(syncedProjectId)
    || !projectedIds.has(serverThread.id)
    || !projectedIds.has(serverMessageRun.message.id)) {
  throw new Error(`canonical server changes did not reach the device feed: ${JSON.stringify(serverProjection)}`)
}
const serverThreadSnapshot = await request('/sync/entities/thread?limit=1000', { token })
const projectedThread = serverThreadSnapshot.items.find((item) => item.entity_id === serverThread.id)
if (!projectedThread || projectedThread.tombstone) {
  throw new Error(`server-created thread is missing from the sync snapshot: ${JSON.stringify(serverThreadSnapshot)}`)
}
await request('/sync/changes', {
  method: 'POST',
  token,
  body: { changes: [syncChange('thread', serverThread.id, projectedThread.revision, 'upsert', {
    ...projectedThread.payload,
    title: 'Renamed offline after server creation',
  })] },
})
const roundTripThreads = await request(`/projects/${syncedProjectId}/threads`, { token })
if (!roundTripThreads.some((item) => (
  item.id === serverThread.id && item.title === 'Renamed offline after server creation'
))) {
  throw new Error(`server-created thread did not round-trip through device sync: ${JSON.stringify(roundTripThreads)}`)
}
const syncAgentId = randomUUID()
await request('/executors', {
  method: 'POST',
  token,
  body: {
    schema_version: 2,
    executor_id: syncAgentId,
    kind: 'personal_device',
    pool_id: null,
    owner_user_id: null,
    display_name: 'Metadata sync soak device',
    protocol_version: 2,
    capabilities: [],
    labels: { os: 'soak' },
    personal_device_remote_control: 'off',
    max_concurrent_runs: 1,
  },
})
const syncAgentCredential = await request(`/executors/${syncAgentId}/credentials`, {
  method: 'POST',
  token,
  body: {
    label: 'Metadata sync soak credential',
    expires_at: new Date(Date.now() + 2 * 60 * 60 * 1000).toISOString(),
  },
})
const agentFeed = await request(`/agent/executors/${syncAgentId}/sync/changes?after=0&limit=1000`, {
  token: syncAgentCredential.token,
})
if (!agentFeed.changes.some((change) => change.entity_id === serverThread.id)) {
  throw new Error(`personal executor could not read its owner's sync feed: ${JSON.stringify(agentFeed)}`)
}
const agentEntityId = randomUUID()
await request(`/agent/executors/${syncAgentId}/sync/changes`, {
  method: 'POST',
  token: syncAgentCredential.token,
  body: { changes: [{
    schema_version: 2,
    operation_id: randomUUID(),
    device_id: syncAgentId,
    entity_type: 'skill',
    entity_id: agentEntityId,
    base_revision: 0,
    operation: 'upsert',
    payload: { name: 'Pushed by the background personal executor' },
    client_timestamp: new Date().toISOString(),
  }] },
})
const agentSnapshot = await request(
  `/agent/executors/${syncAgentId}/sync/entities/skill?limit=100`,
  { token: syncAgentCredential.token },
)
if (!agentSnapshot.items.some((item) => item.entity_id === agentEntityId)) {
  throw new Error(`personal executor push was not materialized: ${JSON.stringify(agentSnapshot)}`)
}
await exerciseBackgroundMetadataSync({
  token,
  userId: session.user_id,
  executorId: syncAgentId,
  credentialToken: syncAgentCredential.token,
  serverEntityId: agentEntityId,
})

console.log(`waves=${waves}`)
console.log(`completed_runs=${completed.length}`)
console.log(`distinct_workers=${assignedWorkers.size}`)
console.log(`runner_max_concurrency=${stats.max_active}`)
console.log('exactly_once_dispatch=ok')
console.log('cross_run_result_isolation=ok')
console.log('atomic_message_run=ok')
console.log('assistant_message_persistence=ok')
console.log('message_run_idempotency=ok')
console.log('metadata_sync_idempotency=ok')
console.log('metadata_sync_conflict=ok')
console.log('metadata_sync_tombstone=ok')
console.log('metadata_sync_cursor=ok')
console.log('metadata_sync_sse_resume=ok')
console.log('metadata_sync_bootstrap_snapshot=ok')
console.log('metadata_sync_canonical_materialization=ok')
console.log('metadata_sync_out_of_order_convergence=ok')
console.log('metadata_sync_reverse_projection=ok')
console.log('metadata_sync_bidirectional_roundtrip=ok')
console.log('metadata_sync_personal_executor_channel=ok')
console.log('metadata_sync_background_daemon_pump=ok')
