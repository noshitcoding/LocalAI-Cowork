import { PlugZap, Save, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type { ProjectRecord, ServerMcpBindingRecord, SyncedEntity } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteManagement.css'

type Props = { client: RemoteRuntimeClient; compact?: boolean }

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function recordOf(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
}
function metadataName(entity: SyncedEntity): string {
  const name = recordOf(entity.payload).name
  return typeof name === 'string' && name.trim() ? name.trim() : entity.entity_id.slice(0, 8)
}
async function allMcpMetadata(client: RemoteRuntimeClient): Promise<SyncedEntity[]> {
  const result: SyncedEntity[] = []
  let after: string | null = null
  do {
    const page = await client.listSyncedEntities('mcp_metadata', after, 500)
    result.push(...page.items.filter((entity) => !entity.tombstone))
    after = page.next_after
  } while (after)
  return result
}

export default function ServerMcpBindingManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [metadata, setMetadata] = useState<SyncedEntity[]>([])
  const [bindings, setBindings] = useState<ServerMcpBindingRecord[]>([])
  const [projectId, setProjectId] = useState('')
  const [entityId, setEntityId] = useState('')
  const [command, setCommand] = useState('')
  const [argsText, setArgsText] = useState('[]')
  const [environmentText, setEnvironmentText] = useState('{}')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const clearSecretForm = useCallback(() => {
    setEntityId(''); setCommand(''); setArgsText('[]'); setEnvironmentText('{}')
    setConfirmDeleteId(null)
  }, [])
  const loadBindings = useCallback(async (selectedProjectId: string) => {
    if (!selectedProjectId) { setBindings([]); return }
    setBindings(await client.listServerMcpBindings(selectedProjectId))
  }, [client])
  const load = useCallback(async () => {
    setBusy(true)
    try {
      const [nextProjects, nextMetadata] = await Promise.all([
        client.listProjects(), allMcpMetadata(client),
      ])
      const nextProjectId = projectId || nextProjects[0]?.id || ''
      setProjects(nextProjects); setMetadata(nextMetadata); setProjectId(nextProjectId)
      await loadBindings(nextProjectId)
      setError(null)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }, [client, loadBindings, projectId])
  useEffect(() => { if (open) void load() }, [load, open])

  const selectedMetadata = useMemo(
    () => metadata.find((entity) => entity.entity_id === entityId) ?? null,
    [entityId, metadata],
  )
  const existing = bindings.find((binding) => binding.mcp_entity_id === entityId) ?? null
  const edit = (binding: ServerMcpBindingRecord) => {
    setEntityId(binding.mcp_entity_id); setCommand(''); setArgsText('[]'); setEnvironmentText('{}')
    setConfirmDeleteId(null); setError(null)
  }
  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      if (!selectedMetadata) throw new Error('Select synchronized MCP metadata first')
      const args = JSON.parse(argsText) as unknown
      const environment = JSON.parse(environmentText) as unknown
      if (!Array.isArray(args) || args.some((argument) => typeof argument !== 'string')) {
        throw new Error('Arguments must be a JSON array of strings')
      }
      if (!environment || typeof environment !== 'object' || Array.isArray(environment)) {
        throw new Error('Environment must be a JSON object containing only string values')
      }
      const environmentRecord = recordOf(environment)
      if (Object.values(environmentRecord).some((value) => typeof value !== 'string')) {
        throw new Error('Environment must be a JSON object containing only string values')
      }
      await client.setServerMcpBinding(projectId, entityId, {
        expected_revision: existing?.revision ?? null,
        name: metadataName(selectedMetadata),
        command,
        args,
        environment: environmentRecord as Record<string, string>,
      })
      clearSecretForm(); await loadBindings(projectId)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async (binding: ServerMcpBindingRecord) => {
    if (confirmDeleteId !== binding.mcp_entity_id) {
      setConfirmDeleteId(binding.mcp_entity_id)
      return
    }
    setBusy(true); setError(null)
    try {
      await client.deleteServerMcpBinding(projectId, binding.mcp_entity_id, binding.revision)
      clearSecretForm(); await loadBindings(projectId)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }
  const close = () => { clearSecretForm(); setOpen(false) }

  if (!open) return <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => setOpen(true)}><PlugZap size={14} /> Server MCP</button>
  return <section className={`remote-management-panel${compact ? ' compact' : ''}`}>
    <header><div><PlugZap size={16} /><strong>Linux server MCP bindings</strong></div><button type="button" aria-label="Close server MCP" onClick={close}><X size={15} /></button></header>
    <p className="remote-management-hint">Commands, arguments, and environment values are envelope-encrypted for this project. Clients receive only safe binding metadata. The command must already exist in the pinned Core sandbox image.</p>
    <label>Project<select aria-label="MCP binding project" value={projectId} onChange={(event) => { const value = event.target.value; setProjectId(value); clearSecretForm(); void loadBindings(value) }}>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label>
    <div className="remote-management-list">
      {!busy && bindings.length === 0 ? <p>No Linux server MCP bindings.</p> : null}
      {bindings.map((binding) => {
        const confirming = confirmDeleteId === binding.mcp_entity_id
        return <article key={binding.mcp_entity_id}><span><strong>{binding.name}</strong><small>revision {binding.revision} - {binding.executable_hint} - {binding.environment_keys.length} environment key(s)</small></span><div><button type="button" disabled={busy} onClick={() => edit(binding)}>Replace</button><button type="button" aria-label={confirming ? `Confirm delete ${binding.name} binding` : `Delete ${binding.name} binding`} disabled={busy} onClick={() => { void remove(binding) }}><Trash2 size={14} />{confirming ? ' Confirm' : null}</button></div></article>
      })}
    </div>
    <form onSubmit={save} autoComplete="off">
      <label>Synchronized MCP metadata<select aria-label="MCP binding metadata" value={entityId} onChange={(event) => { setEntityId(event.target.value); setCommand(''); setArgsText('[]'); setEnvironmentText('{}') }} required><option value="" disabled>Select metadata</option>{metadata.map((entity) => <option key={entity.entity_id} value={entity.entity_id}>{metadataName(entity)} (r{entity.revision})</option>)}</select></label>
      {existing ? <p className="remote-management-hint">Replacing revision {existing.revision} requires the complete command, arguments, and environment again; stored secrets are never returned.</p> : null}
      <label>Sandbox command<input aria-label="MCP sandbox command" value={command} onChange={(event) => setCommand(event.target.value)} placeholder="/opt/mcp/example-server" autoComplete="off" required /></label>
      <label>Arguments JSON<textarea aria-label="MCP arguments JSON" value={argsText} onChange={(event) => setArgsText(event.target.value)} rows={4} spellCheck={false} required /></label>
      <label>Environment JSON<textarea aria-label="MCP environment JSON" value={environmentText} onChange={(event) => setEnvironmentText(event.target.value)} rows={5} spellCheck={false} autoComplete="off" required /></label>
      <div className="remote-management-actions"><button type="submit" disabled={busy || !projectId || !entityId || !command.trim()}><Save size={14} /> {existing ? 'Replace encrypted binding' : 'Create encrypted binding'}</button><button type="button" onClick={clearSecretForm}>Clear</button></div>
    </form>
    {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
  </section>
}
