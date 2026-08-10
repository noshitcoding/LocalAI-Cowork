import { CalendarClock, Pause, Play, Plus, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type { CapabilityCatalog, ExecutorTarget, ProjectRecord, ScheduleRecord, TaskDefinition } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type RemoteScheduleManagerProps = { client: RemoteRuntimeClient; compact?: boolean }
type TargetChoice = { key: string; label: string; target: ExecutorTarget; capabilities: Set<string> }

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function targetKey(target: ExecutorTarget): string {
  if (target.kind === 'server_linux') return `server:${target.pool_id ?? ''}`
  if (target.kind === 'managed_windows_pool') return `windows:${target.pool_id}`
  return `device:${target.device_id}`
}
function choicesFrom(catalog: CapabilityCatalog): TargetChoice[] {
  const choices: TargetChoice[] = [{
    key: 'server:', label: 'Linux server', target: { kind: 'server_linux', pool_id: null },
    capabilities: new Set(catalog.server_linux.map((item) => item.name)),
  }]
  const pools = new Set<string>()
  for (const executor of catalog.executors) {
    if (!executor.online) continue
    const capabilities = new Set(executor.registration.capabilities.map((item) => item.name))
    if (executor.registration.kind === 'managed_windows' && executor.registration.pool_id) {
      const key = `windows:${executor.registration.pool_id}`
      if (pools.has(key)) continue
      pools.add(key)
      choices.push({ key, label: `Windows pool · ${executor.registration.display_name}`, target: { kind: 'managed_windows_pool', pool_id: executor.registration.pool_id }, capabilities })
    } else if (executor.registration.kind === 'personal_device') {
      choices.push({ key: `device:${executor.registration.executor_id}`, label: `Personal device · ${executor.registration.display_name}`, target: { kind: 'personal_device', device_id: executor.registration.executor_id }, capabilities })
    }
  }
  return choices
}

export default function RemoteScheduleManager({ client, compact = false }: RemoteScheduleManagerProps) {
  const [open, setOpen] = useState(false)
  const [creating, setCreating] = useState(false)
  const [tasks, setTasks] = useState<TaskDefinition[]>([])
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [schedules, setSchedules] = useState<ScheduleRecord[]>([])
  const [catalog, setCatalog] = useState<CapabilityCatalog | null>(null)
  const [taskId, setTaskId] = useState('')
  const [target, setTarget] = useState('server:')
  const [cron, setCron] = useState('0 9 * * *')
  const [timezone, setTimezone] = useState(() => Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC')
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextProjects, nextCatalog] = await Promise.all([
        client.listProjects(), client.capabilities(),
      ])
      const [taskGroups, scheduleGroups] = await Promise.all([
        Promise.all(nextProjects.map((project) => client.listTasks(project.id))),
        Promise.all(nextProjects.map((project) => client.listSchedules(project.id))),
      ])
      const nextTasks = taskGroups.flat()
      const nextSchedules = scheduleGroups.flat()
      const released = nextTasks.filter((task) => task.released && !task.deleted_at)
      setTasks(released)
      setProjects(nextProjects)
      setSchedules(nextSchedules.filter((schedule) => !schedule.deleted_at))
      setCatalog(nextCatalog)
      setTaskId((current) => current || released[0]?.id || '')
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])

  useEffect(() => { if (open) void load() }, [load, open])
  const task = tasks.find((item) => item.id === taskId)
  const allChoices = useMemo(() => catalog ? choicesFrom(catalog) : [], [catalog])
  const compatible = useMemo(() => allChoices.filter((choice) => (
    task?.required_capabilities.every((required) => choice.capabilities.has(required)) ?? true
  )), [allChoices, task])
  useEffect(() => {
    const preferred = task?.default_target ? targetKey(task.default_target) : null
    if (preferred && compatible.some((choice) => choice.key === preferred)) setTarget(preferred)
    else if (!compatible.some((choice) => choice.key === target)) setTarget(compatible[0]?.key ?? '')
  }, [compatible, target, task])

  const create = async (event: FormEvent) => {
    event.preventDefault()
    const choice = compatible.find((item) => item.key === target)
    if (!task || !choice) return
    setBusy(true)
    setError(null)
    try {
      const thread = await client.createThread(task.project_id, `Scheduled · ${task.name}`)
      await client.createSchedule({
        task_id: task.id,
        project_id: task.project_id,
        thread_id: thread.id,
        cron,
        timezone,
        executor_target: choice.target,
        input: input.trim() ? { prompt: input.trim() } : {},
        model_profile_id: null,
        enabled: true,
      })
      setCreating(false)
      setInput('')
      await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  const toggle = async (schedule: ScheduleRecord) => {
    setBusy(true)
    try {
      await client.updateSchedule(schedule.id, {
        expected_revision: schedule.revision,
        cron: schedule.cron,
        timezone: schedule.timezone,
        executor_target: schedule.executor_target,
        input: schedule.input,
        model_profile_id: schedule.model_profile_id,
        enabled: !schedule.enabled,
      })
      await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  const remove = async (schedule: ScheduleRecord) => {
    setBusy(true)
    try { await client.deleteSchedule(schedule.id); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><CalendarClock size={14} /> Schedules</button>
  return (
    <section className={`remote-schedule-manager${compact ? ' compact' : ''}`}>
      <header><div><CalendarClock size={15} /><strong>Schedules</strong></div><button type="button" aria-label="Close schedules" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <div className="remote-schedule-list">
        {schedules.length === 0 ? <p>No schedules yet.</p> : schedules.map((schedule) => {
          const scheduleTask = tasks.find((item) => item.id === schedule.task_id)
          const project = projects.find((item) => item.id === schedule.project_id)
          return <article key={schedule.id}><div><strong>{scheduleTask?.name ?? 'Task'}</strong><small>{project?.name ?? schedule.project_id} · {schedule.cron} · {schedule.timezone}</small>{schedule.blocked_reason ? <em>{schedule.blocked_reason}</em> : <small>Next: {schedule.next_run_at ? new Date(schedule.next_run_at).toLocaleString() : 'paused'}</small>}</div><button type="button" disabled={busy} aria-label={schedule.enabled ? 'Pause schedule' : 'Enable schedule'} onClick={() => { void toggle(schedule) }}>{schedule.enabled ? <Pause size={14} /> : <Play size={14} />}</button><button type="button" disabled={busy} aria-label="Delete schedule" onClick={() => { void remove(schedule) }}><Trash2 size={14} /></button></article>
        })}
      </div>
      {creating ? <form onSubmit={create}>
        <label>Released task<select value={taskId} onChange={(event) => setTaskId(event.target.value)} required><option value="" disabled>Select task</option>{tasks.map((item) => <option key={`${item.id}:${item.revision}`} value={item.id}>{item.name}</option>)}</select></label>
        <label>Run on<select value={target} onChange={(event) => setTarget(event.target.value)} required><option value="" disabled>No compatible executor</option>{compatible.map((choice) => <option key={choice.key} value={choice.key}>{choice.label}</option>)}</select></label>
        <label>Cron<input value={cron} onChange={(event) => setCron(event.target.value)} placeholder="0 9 * * *" required /></label>
        <label>IANA timezone<input value={timezone} onChange={(event) => setTimezone(event.target.value)} placeholder="Europe/Berlin" required /></label>
        <label>Optional run input<textarea value={input} onChange={(event) => setInput(event.target.value)} rows={3} /></label>
        <button type="submit" disabled={busy || !task || !target}><Play size={14} /> Create schedule</button>
      </form> : <button className="remote-schedule-add" type="button" disabled={tasks.length === 0} onClick={() => setCreating(true)}><Plus size={14} /> Add schedule</button>}
      {tasks.length === 0 ? <p className="remote-muted">Create and release a reusable task before scheduling it.</p> : null}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
