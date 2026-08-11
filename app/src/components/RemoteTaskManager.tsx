import { ClipboardList, Pencil, Play, Plus, Save, Trash2, Upload, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type {
  CapabilityCatalog,
  ProjectRecord,
  ProviderProfile,
  RunRecord,
  TaskDefinition,
} from '../runtime/contracts'
import {
  providerModelLabel,
  providerSupportsProject,
  providerSupportsTarget,
  remoteTargetChoices,
  remoteTargetKey,
  remoteTargetSupports,
} from '../runtime/remoteExecutionOptions'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteManagement.css'

type Props = {
  client: RemoteRuntimeClient
  compact?: boolean
  onRunCreated: (run: RunRecord) => void | Promise<void>
}

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function capabilitiesFrom(value: string): string[] {
  return [...new Set(value.split(',').map((item) => item.trim()).filter(Boolean))]
}

export default function RemoteTaskManager({ client, compact = false, onRunCreated }: Props) {
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [tasks, setTasks] = useState<TaskDefinition[]>([])
  const [profiles, setProfiles] = useState<ProviderProfile[]>([])
  const [catalog, setCatalog] = useState<CapabilityCatalog | null>(null)
  const [editing, setEditing] = useState<TaskDefinition | null>(null)
  const [creating, setCreating] = useState(false)
  const [projectId, setProjectId] = useState('')
  const [name, setName] = useState('')
  const [instructions, setInstructions] = useState('')
  const [capabilityText, setCapabilityText] = useState('')
  const [defaultTarget, setDefaultTarget] = useState('')
  const [configText, setConfigText] = useState('{}')
  const [release, setRelease] = useState(true)
  const [runTask, setRunTask] = useState<TaskDefinition | null>(null)
  const [runTarget, setRunTarget] = useState('')
  const [runProfileId, setRunProfileId] = useState('')
  const [runInput, setRunInput] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProjects, nextCatalog, nextProfiles] = await Promise.all([
        client.listProjects(), client.capabilities(), client.listProviderProfiles(),
      ])
      const taskGroups = await Promise.all(nextProjects.map((project) => client.listTasks(project.id)))
      setProjects(nextProjects)
      setTasks(taskGroups.flat().filter((task) => !task.deleted_at))
      setCatalog(nextCatalog)
      setProfiles(nextProfiles.filter((profile) => !profile.deleted_at))
      setProjectId((current) => current || nextProjects[0]?.id || '')
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])
  useEffect(() => { if (open) void load() }, [load, open])

  const choices = useMemo(() => catalog ? remoteTargetChoices(catalog) : [], [catalog])
  const required = capabilitiesFrom(capabilityText)
  const formChoices = choices.filter((choice) => (
    remoteTargetSupports(choice, required)
  ))
  useEffect(() => {
    if (defaultTarget && !formChoices.some((choice) => choice.key === defaultTarget)) {
      setDefaultTarget('')
    }
  }, [defaultTarget, formChoices])

  const runProject = projects.find((project) => project.id === runTask?.project_id)
  const runChoices = choices.filter((choice) => (
    runTask ? remoteTargetSupports(choice, runTask.required_capabilities) : false
  ))
  const selectedRunTarget = runChoices.find((choice) => choice.key === runTarget)?.target
  const runProfiles = profiles.filter((profile) => (
    runProject && selectedRunTarget
      ? providerSupportsProject(profile, runProject) && providerSupportsTarget(profile, selectedRunTarget)
      : false
  ))
  useEffect(() => {
    if (runProfileId && !runProfiles.some((profile) => profile.id === runProfileId)) {
      setRunProfileId('')
    }
  }, [runProfileId, runProfiles])

  const resetForm = () => {
    setEditing(null); setCreating(false); setName(''); setInstructions('')
    setCapabilityText(''); setDefaultTarget(''); setConfigText('{}'); setRelease(true)
  }
  const edit = (task: TaskDefinition) => {
    setEditing(task); setCreating(true); setProjectId(task.project_id); setName(task.name)
    setInstructions(task.instructions); setCapabilityText(task.required_capabilities.join(', '))
    setDefaultTarget(task.default_target ? remoteTargetKey(task.default_target) : '')
    setConfigText(JSON.stringify(task.config, null, 2)); setRelease(task.released)
  }
  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const config = JSON.parse(configText) as unknown
      const target = formChoices.find((choice) => choice.key === defaultTarget)?.target ?? null
      const fields = {
        name: name.trim(), instructions: instructions.trim(),
        required_capabilities: required, default_target: target, config, release,
      }
      if (editing) {
        await client.createTaskVersion(editing.id, { base_revision: editing.revision, ...fields })
      } else {
        await client.createTask({ project_id: projectId, ...fields })
      }
      resetForm(); await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const publish = async (task: TaskDefinition) => {
    setBusy(true); setError(null)
    try { await client.releaseTaskVersion(task.id, task.revision); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const remove = async (task: TaskDefinition) => {
    setBusy(true); setError(null)
    try { await client.deleteTask(task.id, task.revision); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const prepareRun = (task: TaskDefinition) => {
    const compatible = choices.filter((choice) => remoteTargetSupports(
      choice,
      task.required_capabilities,
    ))
    const preferred = task.default_target ? remoteTargetKey(task.default_target) : ''
    setRunTask(task)
    setRunTarget(compatible.some((choice) => choice.key === preferred) ? preferred : compatible[0]?.key ?? '')
    setRunProfileId(''); setRunInput('')
  }
  const startRun = async (event: FormEvent) => {
    event.preventDefault()
    const target = runChoices.find((choice) => choice.key === runTarget)?.target
    if (!runTask || !runProject || !target || !runTask.released) return
    setBusy(true); setError(null)
    try {
      const thread = await client.createThread(runProject.id, `Task · ${runTask.name}`)
      const text = runInput.trim() || `Run task: ${runTask.name}`
      const { run } = await client.createThreadMessage(thread.id, {
        content: { text },
        run: {
          thread_id: thread.id,
          project_id: runProject.id,
          project_revision: runProject.revision,
          project_privacy: runProject.privacy,
          task: { id: runTask.id, revision: runTask.revision },
          executor_target: target,
          required_capabilities: runTask.required_capabilities,
          input: runInput.trim() ? { prompt: runInput.trim() } : {},
          model_profile_id: runProfileId || null,
          snapshot_id: null,
          idempotency_key: crypto.randomUUID(),
        },
      })
      setRunTask(null); setRunInput(''); await onRunCreated(run)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><ClipboardList size={14} /> Tasks</button>
  return (
    <section className={`remote-management-panel remote-task-manager${compact ? ' compact' : ''}`}>
      <header><div><ClipboardList size={16} /><strong>Reusable tasks</strong></div><button type="button" aria-label="Close tasks" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <div className="remote-management-list">
        {tasks.length === 0 ? <p>No reusable tasks.</p> : tasks.map((task) => <article key={`${task.id}:${task.revision}`}><span><strong>{task.name}</strong><small>{projects.find((project) => project.id === task.project_id)?.name ?? task.project_id} · revision {task.revision} · {task.released ? 'released' : 'draft'}</small><small>{task.required_capabilities.length > 0 ? task.required_capabilities.join(', ') : 'No special capabilities'}</small></span><div><button type="button" aria-label={`Edit ${task.name}`} disabled={busy} onClick={() => edit(task)}><Pencil size={14} /></button>{task.released ? <button type="button" aria-label={`Run ${task.name}`} disabled={busy} onClick={() => prepareRun(task)}><Play size={14} /></button> : <button type="button" aria-label={`Release ${task.name}`} disabled={busy} onClick={() => { void publish(task) }}><Upload size={14} /></button>}<button type="button" aria-label={`Delete ${task.name}`} disabled={busy} onClick={() => { void remove(task) }}><Trash2 size={14} /></button></div></article>)}
      </div>
      {runTask ? <form onSubmit={startRun}>
        <strong>Run · {runTask.name}</strong>
        <label>Run on<select value={runTarget} onChange={(event) => setRunTarget(event.target.value)} required><option value="" disabled>No compatible executor</option>{runChoices.map((choice) => <option key={choice.key} value={choice.key}>{choice.label}</option>)}</select></label>
        <label>Model profile<select value={runProfileId} onChange={(event) => setRunProfileId(event.target.value)}><option value="">Server/device default</option>{runProfiles.map((profile) => <option key={profile.id} value={profile.id}>{providerModelLabel(profile)}</option>)}</select></label>
        <label>Optional run input<textarea value={runInput} onChange={(event) => setRunInput(event.target.value)} rows={3} placeholder="Additional input for this run" /></label>
        <div className="remote-management-actions"><button type="submit" disabled={busy || !runTarget}><Play size={14} /> Start task</button><button type="button" onClick={() => setRunTask(null)}>Cancel</button></div>
      </form> : null}
      {creating ? <form onSubmit={save}>
        <label>Project<select value={projectId} onChange={(event) => setProjectId(event.target.value)} disabled={Boolean(editing)} required>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label>
        <label>Name<input value={name} onChange={(event) => setName(event.target.value)} maxLength={200} required /></label>
        <label>Instructions<textarea value={instructions} onChange={(event) => setInstructions(event.target.value)} rows={5} required /></label>
        <label>Required capabilities<input value={capabilityText} onChange={(event) => setCapabilityText(event.target.value)} placeholder="browser.headless, office.microsoft" /></label>
        <label>Preferred executor<select value={defaultTarget} onChange={(event) => setDefaultTarget(event.target.value)}><option value="">Choose at run time</option>{formChoices.map((choice) => <option key={choice.key} value={choice.key}>{choice.label}</option>)}</select></label>
        <label>Versioned JSON configuration<textarea value={configText} onChange={(event) => setConfigText(event.target.value)} rows={4} spellCheck={false} required /></label>
        <label className="remote-management-check"><input type="checkbox" checked={release} onChange={(event) => setRelease(event.target.checked)} /> Release this version immediately</label>
        <div className="remote-management-actions"><button type="submit" disabled={busy || !projectId || !name.trim() || !instructions.trim()}><Save size={14} /> {editing ? 'Create version' : 'Create task'}</button><button type="button" onClick={resetForm}>Cancel</button></div>
      </form> : <button className="remote-management-add" type="button" onClick={() => setCreating(true)}><Plus size={14} /> Add task</button>}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
