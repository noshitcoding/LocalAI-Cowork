import { Play, Plus, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react'

import type { CapabilityCatalog, ExecutorTarget, ProjectRecord, ProviderProfile, RunRecord, SyncedEntity } from '../runtime/contracts'
import {
  providerModelLabel,
  providerSupportsProject,
  providerSupportsTarget,
  remoteTargetChoices,
  remoteTargetKey,
  remoteTargetSupports,
  selectedMcpServerNames,
} from '../runtime/remoteExecutionOptions'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { listAllSyncedEntities } from '../runtime/syncedEntityPagination'
import RemoteSnapshotUpload from './RemoteSnapshotUpload'
import {snapshotRequired} from '../runtime/snapshotUpload'
import type {ManagedExecutionResource} from '../runtime/managedExecutionResources'

type RemoteRunComposerProps = {
  client: RemoteRuntimeClient
  onCreated: (run: RunRecord) => void | Promise<void>
  compact?: boolean
  threadId?: string
  threadProjectId?: string
  initialTarget?: ExecutorTarget
  initialCapabilities?: string[]
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function metadataLabel(entity: SyncedEntity): string {
  const payload = entity.payload && typeof entity.payload === 'object' && !Array.isArray(entity.payload)
    ? entity.payload as Record<string, unknown>
    : {}
  const label = payload.name ?? payload.key
  return typeof label === 'string' && label.trim() ? label.trim() : entity.entity_id.slice(0, 8)
}

export default function RemoteRunComposer({
  client,
  onCreated,
  compact = false,
  threadId,
  threadProjectId,
  initialTarget,
  initialCapabilities = [],
}: RemoteRunComposerProps) {
  const pendingOperation = useRef<{ signature: string; idempotencyKey: string } | null>(null)
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [profiles, setProfiles] = useState<ProviderProfile[]>([])
  const [skills, setSkills] = useState<SyncedEntity[]>([])
  const [memories, setMemories] = useState<SyncedEntity[]>([])
  const [mcpMetadata, setMcpMetadata] = useState<SyncedEntity[]>([])
  const [catalog, setCatalog] = useState<CapabilityCatalog | null>(null)
  const [projectId, setProjectId] = useState('')
  const [target, setTarget] = useState(() => initialTarget ? remoteTargetKey(initialTarget) : 'server:')
  const [modelProfileId, setModelProfileId] = useState('')
  const [prompt, setPrompt] = useState('')
  const [capabilities, setCapabilities] = useState(() => initialCapabilities.join(', '))
  const [selectedSkillIds, setSelectedSkillIds] = useState<string[]>([])
  const [selectedMemoryIds, setSelectedMemoryIds] = useState<string[]>([])
  const [selectedMcpIds, setSelectedMcpIds] = useState<string[]>([])
  const [managedResources,setManagedResources]=useState<ManagedExecutionResource[]>([])
  const [managedIds,setManagedIds]=useState<string[]>([])
  const [managedError,setManagedError]=useState('')
  const [busy, setBusy] = useState(false)
  const [snapshotId,setSnapshotId]=useState<string|null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProjects, nextCatalog, nextProfiles, nextSkills, nextMemories, nextMcpMetadata,nextManaged] = await Promise.all([
        client.listProjects(), client.capabilities(), client.listProviderProfiles(),
        listAllSyncedEntities(client, 'skill'), listAllSyncedEntities(client, 'memory'), listAllSyncedEntities(client, 'mcp_metadata'),
        client.listManagedExecutionResources().then(rows=>{setManagedError('');return rows}).catch(cause=>{setManagedError(messageOf(cause));return []}),
      ])
      setProjects(nextProjects)
      setCatalog(nextCatalog)
      setProfiles(nextProfiles.filter((profile) => !profile.deleted_at))
      setSkills(nextSkills.filter((entity) => !entity.tombstone))
      setMemories(nextMemories.filter((entity) => !entity.tombstone))
      setMcpMetadata(nextMcpMetadata.filter((entity) => !entity.tombstone))
      setManagedResources(nextManaged)
      setProjectId((current) => threadProjectId || current || nextProjects[0]?.id || '')
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client, threadProjectId])

  useEffect(() => { if (open) void load() }, [load, open])
  const required = useMemo(
    () => [...new Set([
      ...capabilities.split(',').map((item) => item.trim()).filter(Boolean),
      ...(selectedMcpIds.length > 0 ? ['tool.mcp.invoke'] : []),
      ...managedResources.filter(item=>managedIds.includes(item.id)).flatMap(item=>item.required_capabilities),
    ])],
    [capabilities, selectedMcpIds.length,managedIds,managedResources],
  )
  const choices = useMemo(() => catalog ? remoteTargetChoices(catalog) : [], [catalog])
  const requiredMcpServerNames = useMemo(
    () => selectedMcpServerNames(mcpMetadata, selectedMcpIds),
    [mcpMetadata, selectedMcpIds],
  )
  const compatibleChoices = useMemo(
    () => choices.filter((choice) => remoteTargetSupports(
      choice,
      required,
      requiredMcpServerNames,
    )),
    [choices, required, requiredMcpServerNames],
  )

  useEffect(() => {
    const project = projects.find((item) => item.id === (threadProjectId || projectId))
    const preferred = threadId && initialTarget ? initialTarget : project?.preferred_executor_target
    const preferredKey = preferred ? remoteTargetKey(preferred) : null
    if (preferredKey && compatibleChoices.some((choice) => choice.key === preferredKey)) {
      setTarget(preferredKey)
    } else if (!compatibleChoices.some((choice) => choice.key === target)) {
      setTarget(compatibleChoices[0]?.key ?? '')
    }
  }, [compatibleChoices, initialTarget, projectId, projects, target, threadId, threadProjectId])
  const project = projects.find((item) => item.id === (threadProjectId || projectId))
  const selectedTarget = compatibleChoices.find((item) => item.key === target)?.target
  const needsSnapshot=snapshotRequired(project?.privacy??'',selectedTarget)
  useEffect(()=>{setSnapshotId(null)},[projectId,threadProjectId,target])
  const compatibleProfiles = profiles.filter((profile) => (
    project && selectedTarget
      ? providerSupportsProject(profile, project) && providerSupportsTarget(profile, selectedTarget)
      : false
  ))
  useEffect(() => {
    if (modelProfileId && !compatibleProfiles.some((profile) => profile.id === modelProfileId)) {
      setModelProfileId('')
    }
  }, [compatibleProfiles, modelProfileId])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const project = projects.find((item) => item.id === (threadProjectId || projectId))
    const choice = compatibleChoices.find((item) => item.key === target)
    if (!project || !choice || !prompt.trim() || needsSnapshot&&!snapshotId) return
    setBusy(true)
    setError(null)
    try {
      const content = { text: prompt.trim() }
      const runRequest = {
        project_id: project.id,
        project_revision: project.revision,
        project_privacy: project.privacy,
        task: null,
        executor_target: choice.target,
        required_capabilities: required,
        input: {
          prompt: prompt.trim(),
          ...(selectedSkillIds.length > 0 ? { skill_ids: selectedSkillIds } : {}),
          ...(selectedMemoryIds.length > 0 ? { memory_ids: selectedMemoryIds } : {}),
          ...(selectedMcpIds.length > 0 ? { mcp_metadata_ids: selectedMcpIds } : {}),
          ...(managedIds.length>0?{managed_resource_ids:managedIds}:{}),
        },
        model_profile_id: modelProfileId || null,
        snapshot_id: needsSnapshot?snapshotId:null,
      }
      const signature = JSON.stringify({ threadId, content, runRequest })
      if (pendingOperation.current?.signature !== signature) {
        pendingOperation.current = { signature, idempotencyKey: crypto.randomUUID() }
      }
      const idempotencyKey = pendingOperation.current.idempotencyKey
      const { run } = threadId
        ? await client.createThreadMessage(threadId, {
            content,
            run: { thread_id: threadId, ...runRequest, idempotency_key: idempotencyKey },
          })
        : await client.createThreadRun({
            title: prompt.trim().replaceAll(/\s+/g, ' ').slice(0, 100),
            content,
            run: { ...runRequest, idempotency_key: idempotencyKey },
          })
      pendingOperation.current = null
      setPrompt('')
      setSelectedSkillIds([])
      setSelectedMemoryIds([])
      setSelectedMcpIds([])
      setManagedIds([])
      setOpen(false)
      await onCreated(run)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--primary ui-button--sm'} type="button" onClick={() => setOpen(true)}><Plus size={14} /> {threadId ? 'Continue thread' : 'New run'}</button>
  return (
    <section className={`remote-run-composer${compact ? ' compact' : ''}`}>
      <header><div><Play size={15} /><strong>{threadId ? 'Continue thread' : 'Start server run'}</strong></div><button type="button" aria-label="Close run form" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <form onSubmit={submit}>
        <label>Project<select value={projectId} onChange={(event) => setProjectId(event.target.value)} disabled={Boolean(threadId)} required><option value="" disabled>Select project</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name} · {project.privacy.replaceAll('_', ' ')}</option>)}</select></label>
        <label>Run on<select value={target} onChange={(event) => setTarget(event.target.value)} required><option value="" disabled>No compatible executor</option>{compatibleChoices.map((choice) => <option key={choice.key} value={choice.key}>{choice.label}</option>)}</select></label>
        <label>Model profile<select value={modelProfileId} onChange={(event) => setModelProfileId(event.target.value)}><option value="">Server/device default</option>{compatibleProfiles.map((profile) => <option key={profile.id} value={profile.id}>{providerModelLabel(profile)}</option>)}</select></label>
        <label>Required capabilities<input value={capabilities} onChange={(event) => setCapabilities(event.target.value)} placeholder="Optional, comma-separated: browser.headless, office.microsoft" /></label>
        <label>Frozen skills<select aria-label="Frozen skills" multiple value={selectedSkillIds} onChange={(event) => setSelectedSkillIds(Array.from(event.currentTarget.selectedOptions, (option) => option.value))}>{skills.map((entity) => <option key={entity.entity_id} value={entity.entity_id}>{metadataLabel(entity)} (r{entity.revision})</option>)}</select></label>
        <label>Frozen memory<select aria-label="Frozen memory" multiple value={selectedMemoryIds} onChange={(event) => setSelectedMemoryIds(Array.from(event.currentTarget.selectedOptions, (option) => option.value))}>{memories.map((entity) => <option key={entity.entity_id} value={entity.entity_id}>{metadataLabel(entity)} (r{entity.revision})</option>)}</select></label>
        <label>Executor-bound MCP<select aria-label="Executor-bound MCP" multiple value={selectedMcpIds} onChange={(event) => setSelectedMcpIds(Array.from(event.currentTarget.selectedOptions, (option) => option.value))}>{mcpMetadata.map((entity) => <option key={entity.entity_id} value={entity.entity_id}>{metadataLabel(entity)} (r{entity.revision})</option>)}</select></label>
        {managedResources.length>0&&<label>Central execution resources<select aria-label="Central execution resources" multiple value={managedIds} onChange={event=>setManagedIds(Array.from(event.currentTarget.selectedOptions,option=>option.value))}>{managedResources.map(item=><option key={item.id} value={item.id} disabled={!item.available}>{item.name} · {item.kind} · r{item.revision}{item.available?'':' · assignment or target binding unavailable'}</option>)}</select></label>}
        {managedError&&<p role="status">Central resources could not be loaded: {managedError}. Local resources remain available.</p>}
        {required.length > 0 && compatibleChoices.length === 0 ? <p className="remote-inline-error">No online executor satisfies every required capability.</p> : null}
        <label>Message<textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} rows={compact ? 4 : 5} placeholder="What should the agent do?" required /></label>
        {needsSnapshot&&project&&<RemoteSnapshotUpload key={`${client.enterpriseSource}:${project.id}:${target}`} client={client} projectId={project.id} onReady={setSnapshotId}/>}
        <button className={compact ? '' : 'ui-button ui-button--primary'} type="submit" disabled={busy || !projectId || !target || !prompt.trim() || needsSnapshot&&!snapshotId}><Play size={14} /> {busy ? 'Starting…' : 'Start run'}</button>
      </form>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
