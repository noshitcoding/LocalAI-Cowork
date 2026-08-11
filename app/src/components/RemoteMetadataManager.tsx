import { Boxes, Pencil, Plus, Save, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useState, type FormEvent } from 'react'

import { SCHEMA_VERSION, type SyncChange, type SyncedEntity } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { remoteDeviceId } from '../stores/remoteRuntimeStore'
import './RemoteManagement.css'

type Props = { client: RemoteRuntimeClient; compact?: boolean }
type ManagedEntityType = 'crew' | 'skill' | 'memory' | 'mcp_metadata'

const TYPES: Array<{ value: ManagedEntityType; label: string; singular: string }> = [
  { value: 'crew', label: 'Crews', singular: 'crew' },
  { value: 'skill', label: 'Skills', singular: 'skill' },
  { value: 'memory', label: 'Memory', singular: 'memory entry' },
  { value: 'mcp_metadata', label: 'MCP', singular: 'MCP metadata record' },
]

const SECRET_KEYS = new Set([
  'api_key', 'access_token', 'refresh_token', 'password', 'secret',
  'client_secret', 'authorization',
])

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}
function containsSecret(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(containsSecret)
  const record = recordOf(value)
  return record ? Object.entries(record).some(([key, nested]) => (
    SECRET_KEYS.has(key.toLowerCase()) || containsSecret(nested)
  )) : false
}
function entityLabel(entity: SyncedEntity): string {
  const payload = recordOf(entity.payload)
  const definition = recordOf(payload?.definition)
  const candidate = definition?.name ?? payload?.name ?? payload?.key ?? payload?.title
  return typeof candidate === 'string' && candidate.trim()
    ? candidate.trim()
    : entity.entity_id.slice(0, 8)
}
function initialPayload(type: ManagedEntityType, id: string): Record<string, unknown> {
  if (type === 'crew') {
    const agentId = crypto.randomUUID()
    return {
      definition: {
        id, name: 'New crew', description: '', executionSubject: '',
        executionGuidelines: '', knowledgeFocus: '', governanceMode: 'ask-risky',
        outputMode: 'standard', stopOnFailure: true, retryCount: 0,
        managerReviewEnabled: false, managerReviewGuidelines: '',
        shareAllTaskOutputs: true, sharedOutputCharLimit: 12_000,
        agents: [{
          id: agentId, name: 'Assistant', role: 'executor',
          goal: 'Complete the assigned work.', backstory: '', skillsMarkdown: '',
          personalityId: null, tools: [], mcpServerNames: [], enabled: true,
          allowDelegation: false, verbose: false, maxIterations: 20,
        }],
        tasks: [{
          id: crypto.randomUUID(), description: 'Complete the requested work.',
          expectedOutput: 'A complete result.', agentId, context: [], dependencies: [],
          asyncExecution: false,
        }],
        process: 'sequential', managerAgentId: null, verbose: false,
        maxRpm: 60, maxParallelTasks: 1, createdAt: Date.now(),
      },
      source: 'remote_client',
    }
  }
  if (type === 'skill') return {
    name: 'New skill', description: '', prompt_template: '{{input}}',
    trigger_pattern: null, run_mode: 'execute', auto_generated: false,
    parent_skill_id: null, source_task_ids: null,
  }
  if (type === 'memory') return {
    scope: 'user', scope_ref: null, category: 'context', key: 'new-memory',
    content: '', target: 'user', source_run_id: null, confidence: 1,
  }
  return {
    name: 'New MCP server', transport: 'stdio', executable_hint: '',
    environment_keys: [], device_binding_required: true, source: 'remote_client',
  }
}

function validatePayload(type: ManagedEntityType, id: string, payload: Record<string, unknown>): void {
  if (type === 'crew') {
    const definition = recordOf(payload.definition)
    if (!definition) throw new Error('Crew metadata requires a definition object')
    if (definition.id !== id) throw new Error('Crew definition.id must match the immutable record ID')
    if (typeof definition.name !== 'string' || !definition.name.trim()) throw new Error('Crew definition.name is required')
    const agents = Array.isArray(definition.agents) ? definition.agents.map(recordOf) : []
    const activeAgentIds = new Set(agents.filter((agent) => agent?.enabled !== false).map((agent) => agent?.id).filter((value): value is string => typeof value === 'string' && Boolean(value.trim())))
    if (activeAgentIds.size === 0) throw new Error('Crew metadata requires at least one enabled agent with an ID')
    const tasks = Array.isArray(definition.tasks) ? definition.tasks.map(recordOf) : []
    if (!tasks.some((task) => typeof task?.id === 'string' && activeAgentIds.has(String(task?.agentId)))) {
      throw new Error('Crew metadata requires at least one task assigned to an enabled agent')
    }
    return
  }
  if (type === 'skill') {
    if (typeof payload.name !== 'string' || !payload.name.trim()) throw new Error('Skill name is required')
    if (typeof payload.prompt_template !== 'string' || !payload.prompt_template.trim()) throw new Error('Skill prompt_template is required')
    return
  }
  if (type === 'memory') {
    if (typeof payload.key !== 'string' || !payload.key.trim()) throw new Error('Memory key is required')
    if (typeof payload.content !== 'string') throw new Error('Memory content must be a string')
    return
  }
  if (typeof payload.name !== 'string' || !payload.name.trim()) throw new Error('MCP metadata name is required')
  if (!Array.isArray(payload.environment_keys) || payload.environment_keys.some((key) => typeof key !== 'string')) {
    throw new Error('MCP environment_keys must be an array of names; values remain device-bound')
  }
}

export default function RemoteMetadataManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [entityType, setEntityType] = useState<ManagedEntityType>('crew')
  const [entities, setEntities] = useState<SyncedEntity[]>([])
  const [editing, setEditing] = useState<SyncedEntity | null>(null)
  const [creating, setCreating] = useState(false)
  const [entityId, setEntityId] = useState('')
  const [payloadText, setPayloadText] = useState('{}')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const resetForm = useCallback(() => {
    setEditing(null); setCreating(false); setEntityId(''); setPayloadText('{}')
    setConfirmDeleteId(null)
  }, [])

  const load = useCallback(async () => {
    setBusy(true)
    try {
      const loaded: SyncedEntity[] = []
      let after: string | null = null
      const seenCursors = new Set<string>()
      do {
        const page = await client.listSyncedEntities(entityType, after, 500)
        loaded.push(...page.items)
        after = page.next_after
        if (after && seenCursors.has(after)) throw new Error('Server returned a repeated metadata cursor')
        if (after) seenCursors.add(after)
      } while (after)
      setEntities(loaded.filter((entity) => !entity.tombstone))
      setError(null)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }, [client, entityType])

  useEffect(() => {
    resetForm()
    if (open) void load()
  }, [entityType, load, open, resetForm])

  const startCreate = () => {
    const id = crypto.randomUUID()
    setEditing(null); setCreating(true); setEntityId(id)
    setPayloadText(JSON.stringify(initialPayload(entityType, id), null, 2))
    setError(null); setConfirmDeleteId(null)
  }
  const startEdit = (entity: SyncedEntity) => {
    setEditing(entity); setCreating(true); setEntityId(entity.entity_id)
    setPayloadText(JSON.stringify(entity.payload ?? {}, null, 2))
    setError(null); setConfirmDeleteId(null)
  }
  const conflict = (entity: SyncedEntity | null) => {
    if (entity && !entity.tombstone) startEdit(entity)
    else resetForm()
    setError('This record changed on another device. The latest server revision was loaded; review it before saving again.')
  }
  const submitChange = async (operation: SyncChange['operation'], entity: SyncedEntity | null, payload: unknown) => {
    const change: SyncChange = {
      schema_version: SCHEMA_VERSION,
      operation_id: crypto.randomUUID(),
      device_id: remoteDeviceId(),
      entity_type: entityType,
      entity_id: entity?.entity_id ?? entityId,
      base_revision: entity?.revision ?? 0,
      operation,
      payload,
      client_timestamp: new Date().toISOString(),
    }
    const result = (await client.pushSyncChanges([change])).results[0]
    if (!result) throw new Error('Server returned no metadata result')
    if (result.status === 'conflict') {
      conflict(result.entity)
      return false
    }
    return true
  }
  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const payload = JSON.parse(payloadText) as unknown
      const payloadRecord = recordOf(payload)
      if (!payloadRecord) throw new Error('Metadata payload must be a JSON object')
      if (containsSecret(payload)) {
        throw new Error('Secrets and credentials must use the encrypted profile/device binding flow, not synchronized metadata.')
      }
      validatePayload(entityType, entityId, payloadRecord)
      if (await submitChange('upsert', editing, payload)) {
        resetForm(); await load()
      }
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async (entity: SyncedEntity) => {
    if (confirmDeleteId !== entity.entity_id) {
      setConfirmDeleteId(entity.entity_id)
      return
    }
    setBusy(true); setError(null)
    try {
      if (await submitChange('delete', entity, null)) {
        setConfirmDeleteId(null); await load()
      }
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }

  if (!open) return <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => setOpen(true)}><Boxes size={14} /> Metadata</button>
  const type = TYPES.find((item) => item.value === entityType) ?? TYPES[0]

  return (
    <section className={`remote-management-panel${compact ? ' compact' : ''}`}>
      <header><div><Boxes size={16} /><strong>Shared metadata</strong></div><button type="button" aria-label="Close metadata" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <p className="remote-management-hint">Versioned definitions synchronize between Desktop, Web, and Android. Commands, endpoints, and credentials remain device-bound.</p>
      <label>Record type<select aria-label="Metadata type" value={entityType} onChange={(event) => setEntityType(event.target.value as ManagedEntityType)}>{TYPES.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label>
      <div className="remote-management-list">
        {!busy && entities.length === 0 ? <p>No synchronized {type.label.toLowerCase()}.</p> : null}
        {entities.map((entity) => {
          const label = entityLabel(entity)
          const confirming = confirmDeleteId === entity.entity_id
          return <article key={entity.entity_id}><span><strong>{label}</strong><small>revision {entity.revision} - {entity.entity_id}</small></span><div><button type="button" aria-label={`Edit ${label}`} disabled={busy} onClick={() => startEdit(entity)}><Pencil size={14} /></button><button type="button" aria-label={confirming ? `Confirm delete ${label}` : `Delete ${label}`} disabled={busy} onClick={() => { void remove(entity) }}><Trash2 size={14} />{confirming ? ' Confirm' : null}</button></div></article>
        })}
      </div>
      {creating ? <form onSubmit={save}>
        <label>Immutable ID<input value={entityId} readOnly /></label>
        <label>Revision<input value={editing?.revision ?? 0} readOnly /></label>
        <label>Metadata JSON<textarea aria-label="Metadata JSON" value={payloadText} onChange={(event) => setPayloadText(event.target.value)} rows={14} spellCheck={false} required /></label>
        <div className="remote-management-actions"><button type="submit" disabled={busy}><Save size={14} /> {editing ? 'Save changes' : `Create ${type.singular}`}</button><button type="button" onClick={resetForm}>Cancel</button></div>
      </form> : <button className="remote-management-add" type="button" disabled={busy} onClick={startCreate}><Plus size={14} /> Add {type.singular}</button>}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
