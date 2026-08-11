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

type GuidedFieldsProps = {
  type: ManagedEntityType
  payload: Record<string, unknown>
  onChange: (payload: Record<string, unknown>) => void
}

function stringOf(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function numberOf(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function GuidedMetadataFields({ type, payload, onChange }: GuidedFieldsProps) {
  const setField = (key: string, value: unknown) => onChange({ ...payload, [key]: value })
  if (type === 'crew') {
    const definition = recordOf(payload.definition) ?? {}
    const setDefinition = (key: string, value: unknown) => onChange({
      ...payload,
      definition: { ...definition, [key]: value },
    })
    return <>
      <strong>Guided crew definition</strong>
      <label>Crew name<input aria-label="Crew name" value={stringOf(definition.name)} onChange={(event) => setDefinition('name', event.target.value)} required /></label>
      <label>Governance<select aria-label="Crew governance" value={stringOf(definition.governanceMode, 'ask-risky')} onChange={(event) => setDefinition('governanceMode', event.target.value)}><option value="ask-risky">Ask for risky actions</option><option value="autonomous">Autonomous</option><option value="strict">Strict approvals</option></select></label>
      <label>Description<textarea aria-label="Crew description" value={stringOf(definition.description)} onChange={(event) => setDefinition('description', event.target.value)} rows={3} /></label>
      <label>Execution subject<textarea aria-label="Crew execution subject" value={stringOf(definition.executionSubject)} onChange={(event) => setDefinition('executionSubject', event.target.value)} rows={3} /></label>
      <label>Execution guidelines<textarea aria-label="Crew execution guidelines" value={stringOf(definition.executionGuidelines)} onChange={(event) => setDefinition('executionGuidelines', event.target.value)} rows={4} /></label>
      <label>Knowledge focus<textarea aria-label="Crew knowledge focus" value={stringOf(definition.knowledgeFocus)} onChange={(event) => setDefinition('knowledgeFocus', event.target.value)} rows={3} /></label>
      <label>Output mode<select aria-label="Crew output mode" value={stringOf(definition.outputMode, 'standard')} onChange={(event) => setDefinition('outputMode', event.target.value)}><option value="standard">Standard</option><option value="concise">Concise</option><option value="detailed">Detailed</option></select></label>
      <label>Retry count<input aria-label="Crew retry count" type="number" min={0} max={10} value={numberOf(definition.retryCount, 0)} onChange={(event) => setDefinition('retryCount', Number(event.target.value))} /></label>
      <label className="remote-management-check"><input aria-label="Crew stops on failure" type="checkbox" checked={definition.stopOnFailure !== false} onChange={(event) => setDefinition('stopOnFailure', event.target.checked)} /> Stop on failure</label>
      <label className="remote-management-check"><input aria-label="Crew manager review" type="checkbox" checked={definition.managerReviewEnabled === true} onChange={(event) => setDefinition('managerReviewEnabled', event.target.checked)} /> Manager review</label>
      <p className="remote-management-form-hint">Agents, tasks, provider references, and advanced governance settings remain unchanged. Use Advanced JSON to edit their complete versioned definition.</p>
    </>
  }
  if (type === 'skill') return <>
    <strong>Guided skill definition</strong>
    <label>Skill name<input aria-label="Skill name" value={stringOf(payload.name)} onChange={(event) => setField('name', event.target.value)} required /></label>
    <label>Run mode<select aria-label="Skill run mode" value={stringOf(payload.run_mode, 'execute')} onChange={(event) => setField('run_mode', event.target.value)}><option value="execute">Execute</option><option value="suggest">Suggest</option></select></label>
    <label>Description<textarea aria-label="Skill description" value={stringOf(payload.description)} onChange={(event) => setField('description', event.target.value)} rows={3} /></label>
    <label>Prompt template<textarea aria-label="Skill prompt template" value={stringOf(payload.prompt_template)} onChange={(event) => setField('prompt_template', event.target.value)} rows={7} required /></label>
    <label>Trigger pattern<input aria-label="Skill trigger pattern" value={stringOf(payload.trigger_pattern)} onChange={(event) => setField('trigger_pattern', event.target.value.trim() ? event.target.value : null)} placeholder="Optional pattern" /></label>
    <label className="remote-management-check"><input aria-label="Skill auto generated" type="checkbox" checked={payload.auto_generated === true} onChange={(event) => setField('auto_generated', event.target.checked)} /> Auto-generated</label>
  </>
  if (type === 'memory') return <>
    <strong>Guided memory entry</strong>
    <label>Memory key<input aria-label="Memory key" value={stringOf(payload.key)} onChange={(event) => setField('key', event.target.value)} required /></label>
    <label>Category<input aria-label="Memory category" value={stringOf(payload.category, 'context')} onChange={(event) => setField('category', event.target.value)} required /></label>
    <label>Scope<select aria-label="Memory scope" value={stringOf(payload.scope, 'user')} onChange={(event) => setField('scope', event.target.value)}><option value="user">User</option><option value="project">Project</option><option value="team">Team</option></select></label>
    <label>Scope reference<input aria-label="Memory scope reference" value={stringOf(payload.scope_ref)} onChange={(event) => setField('scope_ref', event.target.value.trim() ? event.target.value : null)} placeholder="Optional project or team ID" /></label>
    <label>Content<textarea aria-label="Memory content" value={stringOf(payload.content)} onChange={(event) => setField('content', event.target.value)} rows={7} /></label>
    <label>Confidence<input aria-label="Memory confidence" type="number" min={0} max={1} step={0.05} value={numberOf(payload.confidence, 1)} onChange={(event) => setField('confidence', Number(event.target.value))} /></label>
  </>
  return <>
    <strong>Guided MCP metadata</strong>
    <label>Server name<input aria-label="MCP metadata name" value={stringOf(payload.name)} onChange={(event) => setField('name', event.target.value)} required /></label>
    <label>Preferred transport<select aria-label="MCP metadata transport" value={stringOf(payload.transport, 'stdio')} onChange={(event) => setField('transport', event.target.value)}><option value="stdio">stdio</option><option value="streamable_http">Streamable HTTP</option></select></label>
    <label>Executable hint<input aria-label="MCP executable hint" value={stringOf(payload.executable_hint)} onChange={(event) => setField('executable_hint', event.target.value)} placeholder="Example: uvx package-name" /></label>
    <label>Environment variable names<input aria-label="MCP environment names" value={(Array.isArray(payload.environment_keys) ? payload.environment_keys : []).filter((value): value is string => typeof value === 'string').join(', ')} onChange={(event) => setField('environment_keys', [...new Set(event.target.value.split(',').map((value) => value.trim()).filter(Boolean))])} placeholder="TOKEN, REGION" /></label>
    <label className="remote-management-check"><input aria-label="MCP requires device binding" type="checkbox" checked={payload.device_binding_required !== false} onChange={(event) => setField('device_binding_required', event.target.checked)} /> Require an encrypted executor binding</label>
    <p className="remote-management-form-hint">Only safe discovery metadata is synchronized. Configure commands, URLs, headers, environment values, and credentials in the encrypted executor-binding flow.</p>
  </>
}

export default function RemoteMetadataManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [entityType, setEntityType] = useState<ManagedEntityType>('crew')
  const [entities, setEntities] = useState<SyncedEntity[]>([])
  const [editing, setEditing] = useState<SyncedEntity | null>(null)
  const [creating, setCreating] = useState(false)
  const [entityId, setEntityId] = useState('')
  const [editorMode, setEditorMode] = useState<'guided' | 'json'>('guided')
  const [draftPayload, setDraftPayload] = useState<Record<string, unknown>>({})
  const [payloadText, setPayloadText] = useState('{}')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const resetForm = useCallback(() => {
    setEditing(null); setCreating(false); setEntityId(''); setEditorMode('guided')
    setDraftPayload({}); setPayloadText('{}')
    setConfirmDeleteId(null)
  }, [])

  const replaceDraft = (payload: Record<string, unknown>) => {
    setDraftPayload(payload)
    setPayloadText(JSON.stringify(payload, null, 2))
  }

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
    setEditorMode('guided'); replaceDraft(initialPayload(entityType, id))
    setError(null); setConfirmDeleteId(null)
  }
  const startEdit = (entity: SyncedEntity) => {
    const payload = recordOf(entity.payload) ?? {}
    setEditing(entity); setCreating(true); setEntityId(entity.entity_id)
    setEditorMode('guided'); replaceDraft(payload)
    setError(null); setConfirmDeleteId(null)
  }
  const conflict = (entity: SyncedEntity | null) => {
    const activeMode = editorMode
    if (entity && !entity.tombstone) {
      startEdit(entity)
      setEditorMode(activeMode)
    }
    else resetForm()
    setError('This record changed on another device. The latest server revision was loaded; review it before saving again.')
  }
  const selectEditorMode = (mode: 'guided' | 'json') => {
    if (mode === 'json') {
      setEditorMode(mode); setError(null)
      return
    }
    try {
      const payload = recordOf(JSON.parse(payloadText) as unknown)
      if (!payload) throw new Error('Metadata payload must be a JSON object')
      replaceDraft(payload); setEditorMode(mode); setError(null)
    } catch (cause) {
      setError(`Fix the advanced JSON before returning to the guided editor: ${messageOf(cause)}`)
    }
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
      const payload = editorMode === 'guided' ? draftPayload : JSON.parse(payloadText) as unknown
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
        <div className="remote-metadata-editor-modes" role="group" aria-label="Metadata editor mode"><button type="button" aria-pressed={editorMode === 'guided'} onClick={() => selectEditorMode('guided')}>Guided</button><button type="button" aria-pressed={editorMode === 'json'} onClick={() => selectEditorMode('json')}>Advanced JSON</button></div>
        {editorMode === 'guided'
          ? <GuidedMetadataFields type={entityType} payload={draftPayload} onChange={replaceDraft} />
          : <label>Metadata JSON<textarea aria-label="Metadata JSON" value={payloadText} onChange={(event) => setPayloadText(event.target.value)} rows={14} spellCheck={false} required /></label>}
        <div className="remote-management-actions"><button type="submit" disabled={busy}><Save size={14} /> {editing ? 'Save changes' : `Create ${type.singular}`}</button><button type="button" onClick={resetForm}>Cancel</button></div>
      </form> : <button className="remote-management-add" type="button" disabled={busy} onClick={startCreate}><Plus size={14} /> Add {type.singular}</button>}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
