import { randomUUID } from 'node:crypto'

const apiBase = process.env.COWORK_TEST_API_URL ?? 'http://127.0.0.1:18101/api/v1'
const bootstrapToken = process.env.COWORK_TEST_BOOTSTRAP_TOKEN

if (!bootstrapToken) throw new Error('COWORK_TEST_BOOTSTRAP_TOKEN is required')

async function api(path, { method = 'GET', token, body } = {}) {
  const response = await fetch(`${apiBase}${path}`, {
    method,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  if (!response.ok) {
    throw new Error(`${method} ${path} returned ${response.status}: ${text}`)
  }
  return text ? JSON.parse(text) : null
}

async function expectStatus(path, expectedStatus, { method = 'GET', token, body } = {}) {
  const response = await fetch(`${apiBase}${path}`, {
    method,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (response.status !== expectedStatus) {
    throw new Error(
      `${method} ${path} returned ${response.status}, expected ${expectedStatus}: ${await response.text()}`,
    )
  }
}

function syncMcpMetadata(deviceId, entityId, name, baseRevision = 0, transport = 'stdio') {
  return {
    schema_version: 2,
    operation_id: randomUUID(),
    device_id: deviceId,
    entity_type: 'mcp_metadata',
    entity_id: entityId,
    base_revision: baseRevision,
    operation: 'upsert',
    payload: {
      name,
      transport,
      executable_hint: transport === 'streamable_http' ? 'https://mcp.example.com' : 'filesystem-mcp',
      environment_keys: transport === 'streamable_http' ? ['Authorization'] : ['MCP_TOKEN', 'SAFE_MODE'],
      device_binding_required: true,
      source: 'server_mcp_acceptance',
    },
    client_timestamp: new Date().toISOString(),
  }
}

const deviceId = randomUUID()
const session = await api('/auth/bootstrap', {
  method: 'POST',
  token: bootstrapToken,
  body: {
    email: 'server-mcp-ci@opencowork.invalid',
    display_name: 'Server MCP CI',
    password: 'Server-MCP-CI-Password-42!',
    device_id: deviceId,
  },
})
const token = session.access_token
const team = await api('/teams', {
  method: 'POST', token, body: { name: 'Server MCP acceptance team' },
})
const project = await api('/projects', {
  method: 'POST',
  token,
  body: {
    name: 'Server MCP acceptance project',
    description: '',
    privacy: 'team_managed',
    team_id: team.id,
    preferred_executor_target: { kind: 'server_linux', pool_id: null },
    policy: { tool_policy: 'autonomous' },
  },
})

const primaryEntityId = randomUUID()
const disposableEntityId = randomUUID()
const duplicateNameEntityId = randomUUID()
const metadata = await api('/sync/changes', {
  method: 'POST',
  token,
  body: {
    changes: [
      syncMcpMetadata(deviceId, primaryEntityId, 'CI filesystem MCP'),
      syncMcpMetadata(deviceId, disposableEntityId, 'CI disposable MCP', 0, 'streamable_http'),
      syncMcpMetadata(deviceId, duplicateNameEntityId, 'CI filesystem MCP'),
    ],
  },
})
if (metadata.results?.length !== 3
    || metadata.results.some((result) => result.status !== 'applied')) {
  throw new Error(`MCP metadata was not synchronized: ${JSON.stringify(metadata)}`)
}

const primaryPath = `/projects/${project.id}/mcp-bindings/${primaryEntityId}`
const created = await api(primaryPath, {
  method: 'PUT',
  token,
  body: {
    expected_revision: null,
    name: 'CI filesystem MCP',
    command: '/opt/cowork/bin/filesystem-mcp',
    args: ['--stdio', '/workspace'],
    environment: { MCP_TOKEN: 'mcp-binding-initial-secret-ci-value', SAFE_MODE: '1' },
  },
})
if (created.revision !== 1
    || created.transport !== 'stdio'
    || created.executable_hint !== 'filesystem-mcp'
    || created.argument_count !== 2
    || JSON.stringify(created.environment_keys) !== JSON.stringify(['MCP_TOKEN', 'SAFE_MODE'])) {
  throw new Error(`created MCP binding metadata is invalid: ${JSON.stringify(created)}`)
}
if (JSON.stringify(created).includes('mcp-binding-initial-secret-ci-value')) {
  throw new Error('MCP binding creation response disclosed its environment secret')
}

const listed = await api(`/projects/${project.id}/mcp-bindings`, { token })
if (listed.length !== 1 || listed[0].mcp_entity_id !== primaryEntityId
    || JSON.stringify(listed).includes('mcp-binding-initial-secret-ci-value')) {
  throw new Error(`MCP binding list is unsafe or incomplete: ${JSON.stringify(listed)}`)
}
await expectStatus(`/projects/${project.id}/mcp-bindings/${duplicateNameEntityId}`, 409, {
  method: 'PUT',
  token,
  body: {
    expected_revision: null,
    name: 'CI filesystem MCP',
    command: '/opt/cowork/bin/duplicate-mcp',
    args: [],
    environment: {},
  },
})
await expectStatus(primaryPath, 409, {
  method: 'PUT',
  token,
  body: {
    expected_revision: null,
    name: 'CI filesystem MCP',
    command: '/opt/cowork/bin/filesystem-mcp',
    args: [],
    environment: {},
  },
})
await expectStatus(primaryPath, 422, {
  method: 'PUT',
  token,
  body: {
    expected_revision: created.revision,
    name: 'CI filesystem MCP',
    command: '/opt/cowork/bin/filesystem-mcp',
    args: [],
    environment: { HTTP_PROXY: 'http://private-network.invalid' },
  },
})
await expectStatus(primaryPath, 422, {
  method: 'PUT',
  token,
  body: {
    expected_revision: created.revision,
    name: 'Wrong synchronized name',
    command: '/opt/cowork/bin/filesystem-mcp',
    args: [],
    environment: {},
  },
})

const updated = await api(primaryPath, {
  method: 'PUT',
  token,
  body: {
    expected_revision: created.revision,
    name: 'CI filesystem MCP',
    command: '/opt/cowork/bin/filesystem-mcp-v2',
    args: ['--stdio'],
    environment: { MCP_TOKEN: 'mcp-binding-rotated-secret-ci-value' },
  },
})
if (updated.revision !== 2 || updated.executable_hint !== 'filesystem-mcp-v2'
    || updated.argument_count !== 1
    || JSON.stringify(updated.environment_keys) !== JSON.stringify(['MCP_TOKEN'])
    || JSON.stringify(updated).includes('mcp-binding-rotated-secret-ci-value')) {
  throw new Error(`updated MCP binding metadata is invalid: ${JSON.stringify(updated)}`)
}

function crewInput(mcpServerName) {
  return {
    task_runner: 'crew',
    crew_id: 'ci-mcp-crew',
    mcp_metadata_ids: [primaryEntityId],
    task_config: {
      crew_definition: {
        id: 'ci-mcp-crew',
        name: 'CI MCP Crew',
        agents: [{
          id: 'researcher',
          name: 'Researcher',
          role: 'Researcher',
          goal: 'Exercise the selected MCP server',
          backstory: 'Acceptance-test agent',
          tools: [],
          mcpServerNames: [mcpServerName],
          enabled: true,
        }],
        tasks: [{
          id: 'research',
          name: 'Research',
          description: 'Use the executor-bound MCP server.',
          expectedOutput: 'A short result.',
          agentId: 'researcher',
          dependencies: [],
        }],
      },
    },
  }
}

async function createCrewMessage(title, input) {
  const crewThread = await api('/threads', {
    method: 'POST',
    token,
    body: {
      project_id: project.id,
      title,
      forked_from_thread_id: null,
      forked_from_message_id: null,
    },
  })
  const body = {
    content: { text: title },
    run: {
      thread_id: crewThread.id,
      project_id: project.id,
      project_revision: project.revision,
      project_privacy: 'team_managed',
      task: null,
      executor_target: { kind: 'server_linux', pool_id: null },
      required_capabilities: ['crew.python'],
      input,
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `crew-mcp-${randomUUID()}`,
    },
  }
  return { crewThread, body }
}

const acceptedCrew = await createCrewMessage(
  'Bound Crew MCP acceptance',
  crewInput('CI filesystem MCP'),
)
const acceptedPair = await api(`/threads/${acceptedCrew.crewThread.id}/messages`, {
  method: 'POST', token, body: acceptedCrew.body,
})
if (acceptedPair.run.spec.input.crew_definition?.agents?.[0]?.mcpServerNames?.[0]
      !== 'CI filesystem MCP'
    || acceptedPair.run.spec.input.frozen_runtime_context?.mcp_metadata?.[0]?.definition?.name
      !== 'CI filesystem MCP'
    || !acceptedPair.run.spec.required_capabilities.includes('tool.mcp.invoke')) {
  throw new Error(`Crew MCP run was not frozen safely: ${JSON.stringify(acceptedPair.run)}`)
}

const rejectedCrew = await createCrewMessage(
  'Unselected Crew MCP rejection',
  crewInput('Unselected MCP'),
)
await expectStatus(`/threads/${rejectedCrew.crewThread.id}/messages`, 422, {
  method: 'POST', token, body: rejectedCrew.body,
})

const renamedMetadata = await api('/sync/changes', {
  method: 'POST',
  token,
  body: {
    changes: [syncMcpMetadata(deviceId, primaryEntityId, 'CI filesystem MCP renamed', 1)],
  },
})
if (renamedMetadata.results?.[0]?.status !== 'applied'
    || renamedMetadata.results?.[0]?.entity?.revision !== 2) {
  throw new Error(`MCP metadata rename was not synchronized: ${JSON.stringify(renamedMetadata)}`)
}

const disposablePath = `/projects/${project.id}/mcp-bindings/${disposableEntityId}`
for (const invalid of [
  { url: 'http://mcp.example.com/mcp', headers: {} },
  { url: 'https://127.0.0.1/mcp', headers: {} },
  { url: 'https://mcp.example.com:8443/mcp', headers: {} },
  { url: 'https://mcp.example.com/mcp?token=unsafe', headers: {} },
  { url: 'https://mcp.example.com/mcp', headers: { 'MCP-Session-Id': 'override' } },
]) {
  await expectStatus(disposablePath, 422, {
    method: 'PUT',
    token,
    body: {
      expected_revision: null,
      name: 'CI disposable MCP',
      transport: 'streamable_http',
      command: '', args: [], environment: {},
      url: invalid.url, headers: invalid.headers,
    },
  })
}
const disposable = await api(disposablePath, {
  method: 'PUT',
  token,
  body: {
    expected_revision: null,
    name: 'CI disposable MCP',
    transport: 'streamable_http',
    command: '', args: [], environment: {},
    url: 'https://mcp.example.com/mcp',
    headers: { Authorization: 'Bearer mcp-http-secret-ci-value' },
  },
})
if (disposable.transport !== 'streamable_http'
    || disposable.executable_hint !== 'HTTPS endpoint'
    || disposable.argument_count !== 0
    || JSON.stringify(disposable.environment_keys) !== JSON.stringify(['Authorization'])
    || JSON.stringify(disposable).includes('mcp-http-secret-ci-value')) {
  throw new Error(`streamable HTTP binding metadata is unsafe or invalid: ${JSON.stringify(disposable)}`)
}
await expectStatus(`${disposablePath}?expected_revision=${disposable.revision + 1}`, 409, {
  method: 'DELETE', token,
})
await expectStatus(`${disposablePath}?expected_revision=${disposable.revision}`, 204, {
  method: 'DELETE', token,
})

const thread = await api('/threads', {
  method: 'POST',
  token,
  body: {
    project_id: project.id,
    title: 'Missing MCP binding rejection',
    forked_from_thread_id: null,
    forked_from_message_id: null,
  },
})
await expectStatus(`/threads/${thread.id}/messages`, 422, {
  method: 'POST',
  token,
  body: {
    content: { text: 'This run must fail closed before queueing.' },
    run: {
      thread_id: thread.id,
      project_id: project.id,
      project_revision: project.revision,
      project_privacy: 'team_managed',
      task: null,
      executor_target: { kind: 'server_linux', pool_id: null },
      required_capabilities: [],
      input: { mcp_metadata_ids: [disposableEntityId] },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `missing-mcp-binding-${randomUUID()}`,
    },
  },
})
const renamedThread = await api('/threads', {
  method: 'POST',
  token,
  body: {
    project_id: project.id,
    title: 'Renamed MCP binding rejection',
    forked_from_thread_id: null,
    forked_from_message_id: null,
  },
})
await expectStatus(`/threads/${renamedThread.id}/messages`, 422, {
  method: 'POST',
  token,
  body: {
    content: { text: 'This stale binding must fail closed before queueing.' },
    run: {
      thread_id: renamedThread.id,
      project_id: project.id,
      project_revision: project.revision,
      project_privacy: 'team_managed',
      task: null,
      executor_target: { kind: 'server_linux', pool_id: null },
      required_capabilities: [],
      input: { mcp_metadata_ids: [primaryEntityId] },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `renamed-mcp-binding-${randomUUID()}`,
    },
  },
})

const finalBindings = await api(`/projects/${project.id}/mcp-bindings`, { token })
if (finalBindings.length !== 1 || finalBindings[0].mcp_entity_id !== primaryEntityId
    || finalBindings[0].revision !== 2) {
  throw new Error(`final MCP binding state is invalid: ${JSON.stringify(finalBindings)}`)
}

console.log('mcp_binding_encrypted_api=ok')
console.log('mcp_binding_safe_metadata=ok')
console.log('mcp_binding_revision_conflicts=ok')
console.log('mcp_binding_name_identity=ok')
console.log('mcp_binding_missing_run_rejection=ok')
console.log('mcp_binding_crew_selection=ok')
console.log('mcp_binding_streamable_http_validation=ok')
