import { Play, Plus, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

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

async function allMetadata(client: RemoteRuntimeClient, entityType: 'skill' | 'memory' | 'mcp_metadata'): Promise<SyncedEntity[]> {
  const entities: SyncedEntity[] = []
  let after: string | null = null
  do {
    const page = await client.listSyncedEntities(entityType, after, 500)
    entities.push(...page.items.filter((entity) => !entity.tombstone))
    after = page.next_after
  } while (after)
  return entities
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
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProjects, nextCatalog, nextProfiles, nextSkills, nextMemories, nextMcpMetadata] = await Promise.all([
        client.listProjects(), client.capabilities(), client.listProviderProfiles(),
        allMetadata(client, 'skill'), allMetadata(client, 'memory'), allMetadata(client, 'mcp_metadata'),
      ])
      setProjects(nextProjects)
      setCatalog(nextCatalog)
      setProfiles(nextProfiles.filter((profile) => !profile.deleted_at))
      setSkills(nextSkills)
      setMemories(nextMemories)
      setMcpMetadata(nextMcpMetadata)
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
    ])],
    [capabilities, selectedMcpIds.length],
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
    if (!project || !choice || !prompt.trim()) return
    setBusy(true)
    setError(null)
    try {
      const activeThreadId = threadId ?? (await client.createThread(
        project.id,
        prompt.trim().replaceAll(/\s+/g, ' ').slice(0, 100),
      )).id
      const { run } = await client.createThreadMessage(activeThreadId, {
        content: { text: prompt.trim() },
        run: {
          thread_id: activeThreadId,
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
          },
          model_profile_id: modelProfileId || null,
          snapshot_id: null,
          idempotency_key: crypto.randomUUID(),
        },
      })
      setPrompt('')
      setSelectedSkillIds([])
      setSelectedMemoryIds([])
      setSelectedMcpIds([])
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
        {required.length > 0 && compatibleChoices.length === 0 ? <p className="remote-inline-error">No online executor satisfies every required capability.</p> : null}
        <label>Message<textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} rows={compact ? 4 : 5} placeholder="What should the agent do?" required /></label>
        <button className={compact ? '' : 'ui-button ui-button--primary'} type="submit" disabled={busy || !projectId || !target || !prompt.trim()}><Play size={14} /> {busy ? 'Starting…' : 'Start run'}</button>
      </form>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
