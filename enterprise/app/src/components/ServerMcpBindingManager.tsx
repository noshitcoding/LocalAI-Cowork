import { PlugZap, Save, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react'

import type { ProjectRecord, ServerMcpBindingRecord, SyncedEntity } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { listAllSyncedEntities } from '../runtime/syncedEntityPagination'
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
export default function ServerMcpBindingManager({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [metadata, setMetadata] = useState<SyncedEntity[]>([])
  const [bindings, setBindings] = useState<ServerMcpBindingRecord[]>([])
  const [bindingsProjectId, setBindingsProjectId] = useState<string | null>(null)
  const [bindingsLoading, setBindingsLoading] = useState(false)
  const [projectId, setProjectId] = useState('')
  const [entityId, setEntityId] = useState('')
  const [transport, setTransport] = useState<'stdio' | 'streamable_http' | 'sse'>('stdio')
  const [command, setCommand] = useState('')
  const [argsText, setArgsText] = useState('[]')
  const [environmentText, setEnvironmentText] = useState('{}')
  const [url, setUrl] = useState('')
  const [headersText, setHeadersText] = useState('{}')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const projectIdRef = useRef(projectId)
  const bindingRequestGeneration = useRef(0)

  useEffect(() => {
    projectIdRef.current = projectId
  }, [projectId])

  const clearSecretForm = useCallback(() => {
    setEntityId(''); setTransport('stdio'); setCommand(''); setArgsText('[]'); setEnvironmentText('{}')
    setUrl(''); setHeadersText('{}')
    setConfirmDeleteId(null)
  }, [])
  const loadBindings = useCallback(async (selectedProjectId: string) => {
    const generation = ++bindingRequestGeneration.current
    setBindings([]); setBindingsProjectId(null)
    if (!selectedProjectId) { setBindingsLoading(false); return }
    setBindingsLoading(true)
    try {
      const nextBindings = await client.listServerMcpBindings(selectedProjectId)
      if (bindingRequestGeneration.current !== generation || projectIdRef.current !== selectedProjectId) return
      setBindings(nextBindings); setBindingsProjectId(selectedProjectId); setError(null)
    } catch (cause) {
      if (bindingRequestGeneration.current !== generation || projectIdRef.current !== selectedProjectId) return
      setError(messageOf(cause))
    } finally {
      if (bindingRequestGeneration.current === generation && projectIdRef.current === selectedProjectId) {
        setBindingsLoading(false)
      }
    }
  }, [client])
  const load = useCallback(async () => {
    setBusy(true)
    try {
      const [nextProjects, nextMetadata] = await Promise.all([
        client.listProjects(), listAllSyncedEntities(client, 'mcp_metadata'),
      ])
      const currentProjectId = projectIdRef.current
      const nextProjectId = currentProjectId && nextProjects.some((project) => project.id === currentProjectId)
        ? currentProjectId
        : nextProjects[0]?.id || ''
      projectIdRef.current = nextProjectId
      setProjects(nextProjects); setMetadata(nextMetadata.filter((entity) => !entity.tombstone)); setProjectId(nextProjectId)
      await loadBindings(nextProjectId)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }, [client, loadBindings])
  useEffect(() => { if (open) void load() }, [load, open])

  const selectedMetadata = useMemo(
    () => metadata.find((entity) => entity.entity_id === entityId) ?? null,
    [entityId, metadata],
  )
  const bindingsMatchProject = bindingsProjectId === projectId && !bindingsLoading
  const existing = bindingsMatchProject
    ? bindings.find((binding) => binding.mcp_entity_id === entityId) ?? null
    : null
  const edit = (binding: ServerMcpBindingRecord) => {
    setEntityId(binding.mcp_entity_id)
    setTransport(binding.transport === 'sse' ? 'sse' : binding.transport === 'streamable_http' ? 'streamable_http' : 'stdio')
    setCommand(''); setArgsText('[]'); setEnvironmentText('{}'); setUrl(''); setHeadersText('{}')
    setConfirmDeleteId(null); setError(null)
  }
  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const targetProjectId = projectId
      if (!targetProjectId || !bindingsMatchProject) throw new Error('Bindings for the selected project are still loading')
      if (!selectedMetadata) throw new Error('Select synchronized MCP metadata first')
      const args = transport === 'stdio' ? JSON.parse(argsText) as unknown : []
      const environment = transport === 'stdio' ? JSON.parse(environmentText) as unknown : {}
      const headers = transport !== 'stdio' ? JSON.parse(headersText) as unknown : {}
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
      if (!headers || typeof headers !== 'object' || Array.isArray(headers)) {
        throw new Error('Headers must be a JSON object containing only string values')
      }
      const headersRecord = recordOf(headers)
      if (Object.values(headersRecord).some((value) => typeof value !== 'string')) {
        throw new Error('Headers must be a JSON object containing only string values')
      }
      await client.setServerMcpBinding(targetProjectId, entityId, {
        expected_revision: existing?.revision ?? null,
        name: metadataName(selectedMetadata),
        transport,
        command: transport === 'stdio' ? command : '',
        args: transport === 'stdio' ? args : [],
        environment: transport === 'stdio' ? environmentRecord as Record<string, string> : {},
        url: transport !== 'stdio' ? url : '',
        headers: transport !== 'stdio' ? headersRecord as Record<string, string> : {},
      })
      if (projectIdRef.current === targetProjectId) {
        clearSecretForm(); await loadBindings(targetProjectId)
      }
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async (binding: ServerMcpBindingRecord) => {
    if (!bindingsMatchProject || binding.project_id !== projectId) {
      setError('This binding no longer belongs to the selected project')
      return
    }
    if (confirmDeleteId !== binding.mcp_entity_id) {
      setConfirmDeleteId(binding.mcp_entity_id)
      return
    }
    setBusy(true); setError(null)
    try {
      const targetProjectId = projectId
      await client.deleteServerMcpBinding(targetProjectId, binding.mcp_entity_id, binding.revision)
      if (projectIdRef.current === targetProjectId) {
        clearSecretForm(); await loadBindings(targetProjectId)
      }
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }
  const close = () => {
    bindingRequestGeneration.current += 1
    setBindings([]); setBindingsProjectId(null); setBindingsLoading(false)
    clearSecretForm(); setOpen(false)
  }

  if (!open) return <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => setOpen(true)}><PlugZap size={14} /> Server MCP</button>
  return <section className={`remote-management-panel${compact ? ' compact' : ''}`}>
    <header><div><PlugZap size={16} /><strong>Linux server MCP bindings</strong></div><button type="button" aria-label="Close server MCP" onClick={close}><X size={15} /></button></header>
    <p className="remote-management-hint">Stdio commands or HTTPS endpoints and their credential values are envelope-encrypted for this project. Clients receive only safe binding metadata. Stdio commands must already exist in the pinned Core sandbox image.</p>
    <label>Project<select aria-label="MCP binding project" value={projectId} onChange={(event) => { const value = event.target.value; projectIdRef.current = value; setProjectId(value); clearSecretForm(); void loadBindings(value) }}>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label>
    <div className="remote-management-list">
      {!busy && !bindingsLoading && bindingsMatchProject && bindings.length === 0 ? <p>No Linux server MCP bindings.</p> : null}
      {bindings.map((binding) => {
        const confirming = confirmDeleteId === binding.mcp_entity_id
        const transportLabel = binding.transport === 'sse' ? 'Legacy HTTP+SSE' : binding.transport === 'streamable_http' ? 'Streamable HTTP' : 'stdio'
        return <article key={binding.mcp_entity_id}><span><strong>{binding.name}</strong><small>revision {binding.revision} - {transportLabel} - {binding.executable_hint} - {binding.environment_keys.length} secret key(s)</small></span><div><button type="button" disabled={busy} onClick={() => edit(binding)}>Replace</button><button type="button" aria-label={confirming ? `Confirm delete ${binding.name} binding` : `Delete ${binding.name} binding`} disabled={busy} onClick={() => { void remove(binding) }}><Trash2 size={14} />{confirming ? ' Confirm' : null}</button></div></article>
      })}
    </div>
    <form onSubmit={save} autoComplete="off">
      <label>Synchronized MCP metadata<select aria-label="MCP binding metadata" value={entityId} onChange={(event) => { setEntityId(event.target.value); setCommand(''); setArgsText('[]'); setEnvironmentText('{}'); setUrl(''); setHeadersText('{}') }} required><option value="" disabled>Select metadata</option>{metadata.map((entity) => <option key={entity.entity_id} value={entity.entity_id}>{metadataName(entity)} (r{entity.revision})</option>)}</select></label>
      {existing ? <p className="remote-management-hint">Replacing revision {existing.revision} requires the complete transport configuration and credentials again; stored secrets are never returned.</p> : null}
      <label>Transport<select aria-label="MCP binding transport" value={transport} onChange={(event) => setTransport(event.target.value as 'stdio' | 'streamable_http' | 'sse')}><option value="stdio">stdio sandbox process</option><option value="streamable_http">Streamable HTTP (HTTPS)</option><option value="sse">Legacy HTTP+SSE (HTTPS)</option></select></label>
      {transport === 'stdio' ? <>
        <label>Sandbox command<input aria-label="MCP sandbox command" value={command} onChange={(event) => setCommand(event.target.value)} placeholder="/opt/mcp/example-server" autoComplete="off" required /></label>
        <label>Arguments JSON<textarea aria-label="MCP arguments JSON" value={argsText} onChange={(event) => setArgsText(event.target.value)} rows={4} spellCheck={false} required /></label>
        <label>Environment JSON<textarea aria-label="MCP environment JSON" value={environmentText} onChange={(event) => setEnvironmentText(event.target.value)} rows={5} spellCheck={false} autoComplete="off" required /></label>
      </> : <>
        <label>{transport === 'sse' ? 'HTTPS SSE endpoint' : 'HTTPS MCP endpoint'}<input type="url" aria-label="MCP HTTPS endpoint" value={url} onChange={(event) => setUrl(event.target.value)} placeholder={transport === 'sse' ? 'https://mcp.example.com/events' : 'https://mcp.example.com/mcp'} autoComplete="off" required /></label>
        <label>Credential headers JSON<textarea aria-label="MCP credential headers JSON" value={headersText} onChange={(event) => setHeadersText(event.target.value)} rows={5} spellCheck={false} autoComplete="off" required /></label>
        <p className="remote-management-hint">Use headers such as Authorization for credentials. Only public DNS endpoints on HTTPS port 443 are accepted; userinfo, query strings, IP literals, local hostnames, and redirects are rejected.</p>
      </>}
      <div className="remote-management-actions"><button type="submit" disabled={busy || bindingsLoading || !bindingsMatchProject || !projectId || !entityId || (transport === 'stdio' ? !command.trim() : !url.trim())}><Save size={14} /> {existing ? 'Replace encrypted binding' : 'Create encrypted binding'}</button><button type="button" onClick={clearSecretForm}>Clear</button></div>
    </form>
    {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
  </section>
}
