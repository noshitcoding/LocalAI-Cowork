import { Play, Plus, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type { CapabilityCatalog, ExecutorTarget, ProjectRecord, ProviderProfile, RunRecord } from '../runtime/contracts'
import {
  providerModelLabel,
  providerSupportsProject,
  providerSupportsTarget,
  remoteTargetChoices,
  remoteTargetKey,
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
  const [catalog, setCatalog] = useState<CapabilityCatalog | null>(null)
  const [projectId, setProjectId] = useState('')
  const [target, setTarget] = useState(() => initialTarget ? remoteTargetKey(initialTarget) : 'server:')
  const [modelProfileId, setModelProfileId] = useState('')
  const [prompt, setPrompt] = useState('')
  const [capabilities, setCapabilities] = useState(() => initialCapabilities.join(', '))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProjects, nextCatalog, nextProfiles] = await Promise.all([
        client.listProjects(), client.capabilities(), client.listProviderProfiles(),
      ])
      setProjects(nextProjects)
      setCatalog(nextCatalog)
      setProfiles(nextProfiles.filter((profile) => !profile.deleted_at))
      setProjectId((current) => threadProjectId || current || nextProjects[0]?.id || '')
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client, threadProjectId])

  useEffect(() => { if (open) void load() }, [load, open])
  const required = useMemo(
    () => [...new Set(capabilities.split(',').map((item) => item.trim()).filter(Boolean))],
    [capabilities],
  )
  const choices = useMemo(() => catalog ? remoteTargetChoices(catalog) : [], [catalog])
  const compatibleChoices = useMemo(
    () => choices.filter((choice) => required.every((capability) => choice.capabilities.has(capability))),
    [choices, required],
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
          input: { prompt: prompt.trim() },
          model_profile_id: modelProfileId || null,
          snapshot_id: null,
          idempotency_key: crypto.randomUUID(),
        },
      })
      setPrompt('')
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
        {required.length > 0 && compatibleChoices.length === 0 ? <p className="remote-inline-error">No online executor satisfies every required capability.</p> : null}
        <label>Message<textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} rows={compact ? 4 : 5} placeholder="What should the agent do?" required /></label>
        <button className={compact ? '' : 'ui-button ui-button--primary'} type="submit" disabled={busy || !projectId || !target || !prompt.trim()}><Play size={14} /> {busy ? 'Starting…' : 'Start run'}</button>
      </form>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
