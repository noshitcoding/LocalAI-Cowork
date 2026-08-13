import { Download, Gauge, Headphones, Plus, RefreshCw, Save, ShieldX, X } from 'lucide-react'
import { useCallback, useEffect, useState, type FormEvent } from 'react'

import type {
  ProjectRecord,
  OperationsSnapshot,
  QuotaScopeType,
  QuotaStatus,
  SetQuotaLimitsRequest,
  SupportGrantRecord,
  ThreadRecord,
} from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type Props = { client: RemoteRuntimeClient; currentUserId: string; compact?: boolean }
function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }
function optionalNumber(value: string): number | null { return value.trim() === '' ? null : Number(value) }
function editableValue(value: number | null): string { return value === null ? '' : String(value) }
function formatBytes(value: number): string {
  if (value < 1_024) return `${value} B`
  if (value < 1_048_576) return `${(value / 1_024).toFixed(1)} KiB`
  if (value < 1_073_741_824) return `${(value / 1_048_576).toFixed(1)} MiB`
  return `${(value / 1_073_741_824).toFixed(2)} GiB`
}

export default function RemoteGovernancePanel({ client, currentUserId, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [threads, setThreads] = useState<ThreadRecord[]>([])
  const [grants, setGrants] = useState<SupportGrantRecord[]>([])
  const [projectId, setProjectId] = useState('')
  const [threadId, setThreadId] = useState('')
  const [scope, setScope] = useState<'project' | 'thread'>('project')
  const [supportUserId, setSupportUserId] = useState('')
  const [reason, setReason] = useState('')
  const [hours, setHours] = useState(1)
  const [quotaScope, setQuotaScope] = useState<QuotaScopeType>('user')
  const [quotaScopeId, setQuotaScopeId] = useState(currentUserId)
  const [quota, setQuota] = useState<QuotaStatus | null>(null)
  const [quotaDraft, setQuotaDraft] = useState<Record<'storage' | 'runs' | 'tokens' | 'cost', string>>({ storage: '', runs: '', tokens: '', cost: '' })
  const [hardCostLimit, setHardCostLimit] = useState(true)
  const [operations, setOperations] = useState<OperationsSnapshot | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [loadedProjects, loadedGrants] = await Promise.all([
        client.listProjects(), client.listSupportGrants(),
      ])
      setProjects(loadedProjects); setGrants(loadedGrants)
      setProjectId((current) => current || loadedProjects[0]?.id || '')
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])
  useEffect(() => { if (open) void load() }, [load, open])
  const loadQuota = useCallback(async () => {
    if (!quotaScopeId) return
    try {
      const loaded = await client.getQuota(quotaScope, quotaScopeId)
      setQuota(loaded)
      setQuotaDraft({
        storage: editableValue(loaded.limits.storage_bytes),
        runs: editableValue(loaded.limits.concurrent_runs),
        tokens: editableValue(loaded.limits.monthly_tokens),
        cost: editableValue(loaded.limits.monthly_cost_micros),
      })
      setHardCostLimit(loaded.limits.hard_cost_limit)
      setError(null)
    } catch (cause) { setQuota(null); setError(messageOf(cause)) }
  }, [client, quotaScope, quotaScopeId])
  useEffect(() => { if (open) void loadQuota() }, [loadQuota, open])
  useEffect(() => {
    setThreadId(''); setThreads([])
    if (!open || scope !== 'thread' || !projectId) return
    void client.listProjectThreads(projectId)
      .then((loaded) => { setThreads(loaded); setThreadId(loaded[0]?.id ?? '') })
      .catch((cause) => setError(messageOf(cause)))
  }, [client, open, projectId, scope])

  const create = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      await client.createSupportGrant({
        support_user_id: supportUserId.trim(),
        project_id: scope === 'project' ? projectId : null,
        thread_id: scope === 'thread' ? threadId : null,
        reason: reason.trim(),
        expires_at: new Date(Date.now() + hours * 60 * 60 * 1_000).toISOString(),
      })
      setSupportUserId(''); setReason(''); await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const revoke = async (grantId: string) => {
    setBusy(true); setError(null)
    try { await client.revokeSupportGrant(grantId); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const saveQuota = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const request: SetQuotaLimitsRequest = {
        storage_bytes: optionalNumber(quotaDraft.storage),
        concurrent_runs: optionalNumber(quotaDraft.runs),
        monthly_tokens: optionalNumber(quotaDraft.tokens),
        monthly_cost_micros: optionalNumber(quotaDraft.cost),
        hard_cost_limit: hardCostLimit,
      }
      const updated = await client.setQuota(quotaScope, quotaScopeId, request)
      setQuota(updated)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const loadOperations = async () => {
    setBusy(true); setError(null)
    try { setOperations(await client.operationsMetrics()) }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const downloadSupportBundle = async () => {
    setBusy(true); setError(null)
    try {
      const blob = await client.downloadSupportBundle()
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = `open-cowork-support-${new Date().toISOString().replace(/[:.]/g, '-')}.json`
      link.click()
      URL.revokeObjectURL(url)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><Gauge size={14} /> Governance</button>
  return (
    <section className={`remote-governance-panel${compact ? ' compact' : ''}`}>
      <header><div><Gauge size={16} /><strong>Governance</strong></div><button type="button" aria-label="Close governance" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <section className="remote-quota-panel">
        <div><span><Gauge size={14} /><strong>Server operations</strong></span><button type="button" aria-label="Refresh server operations" disabled={busy} onClick={() => { void loadOperations() }}><RefreshCw size={14} /></button></div>
        <p>Platform administrators can inspect aggregate local metrics and export a redacted diagnostic bundle. Project names, prompts, files, object keys, identities, and secrets are excluded.</p>
        {operations ? <dl className="remote-quota-usage"><div><dt>Build / migration</dt><dd>{operations.application.build_version} / {operations.application.database_migration_version}</dd></div><div><dt>Users / projects</dt><dd>{operations.database.users} / {operations.database.projects}</dd></div><div><dt>Recently seen executors</dt><dd>{operations.workload.executors_recently_seen} / {operations.workload.executors_registered}</dd></div><div><dt>Stored chunks</dt><dd>{formatBytes(operations.storage.ready_chunk_ciphertext_bytes)}</dd></div></dl> : null}
        <button type="button" disabled={busy} onClick={() => { void downloadSupportBundle() }}><Download size={14} /> Download redacted support bundle</button>
      </section>
      <section className="remote-quota-panel">
        <div><span><Gauge size={14} /><strong>Quotas and live usage</strong></span><button type="button" aria-label="Refresh quota" disabled={busy || !quotaScopeId} onClick={() => { void loadQuota() }}><RefreshCw size={14} /></button></div>
        <p>Users can inspect their own usage. Platform administrators manage users; team owners and admins manage teams.</p>
        <form className="remote-quota-form" onSubmit={saveQuota}>
          <label>Scope<select value={quotaScope} onChange={(event) => { const next = event.target.value as QuotaScopeType; setQuotaScope(next); setQuotaScopeId(next === 'user' ? currentUserId : '') }}><option value="user">User</option><option value="team">Team</option></select></label>
          <label>Scope ID<input value={quotaScopeId} onChange={(event) => setQuotaScopeId(event.target.value.trim())} pattern="[0-9a-fA-F-]{36}" placeholder="UUID" required /></label>
          <label>Storage bytes<input type="number" min={0} value={quotaDraft.storage} onChange={(event) => setQuotaDraft((value) => ({ ...value, storage: event.target.value }))} placeholder="Unlimited" /></label>
          <label>Concurrent runs<input type="number" min={0} value={quotaDraft.runs} onChange={(event) => setQuotaDraft((value) => ({ ...value, runs: event.target.value }))} placeholder="Unlimited" /></label>
          <label>Monthly tokens<input type="number" min={0} value={quotaDraft.tokens} onChange={(event) => setQuotaDraft((value) => ({ ...value, tokens: event.target.value }))} placeholder="Unlimited" /></label>
          <label>Monthly cost (micros)<input type="number" min={0} value={quotaDraft.cost} onChange={(event) => setQuotaDraft((value) => ({ ...value, cost: event.target.value }))} placeholder="Unlimited" /></label>
          <label className="remote-quota-check"><input type="checkbox" checked={hardCostLimit} onChange={(event) => setHardCostLimit(event.target.checked)} /> Stop at the configured cost limit</label>
          <button type="submit" disabled={busy || !quotaScopeId}><Save size={14} /> Save limits</button>
        </form>
        {quota ? <dl className="remote-quota-usage"><div><dt>Storage</dt><dd>{formatBytes(quota.usage.storage_bytes)}{quota.limits.storage_bytes === null ? '' : ` / ${formatBytes(quota.limits.storage_bytes)}`}</dd></div><div><dt>Active runs</dt><dd>{quota.usage.running_runs}{quota.limits.concurrent_runs === null ? '' : ` / ${quota.limits.concurrent_runs}`}</dd></div><div><dt>Monthly tokens</dt><dd>{quota.usage.tokens.toLocaleString()}{quota.limits.monthly_tokens === null ? '' : ` / ${quota.limits.monthly_tokens.toLocaleString()}`}</dd></div><div><dt>Monthly cost</dt><dd>{quota.usage.cost_micros.toLocaleString()} μ</dd></div></dl> : null}
      </section>
      <section className="remote-support-section">
      <div className="remote-section-title"><Headphones size={14} /><strong>Temporary support access</strong></div>
      <p>Project editors can grant a platform administrator audited viewer access for at most 24 hours.</p>
      <form className="remote-support-form" onSubmit={create}>
        <label>Support administrator user ID<input value={supportUserId} onChange={(event) => setSupportUserId(event.target.value)} pattern="[0-9a-fA-F-]{36}" placeholder="00000000-0000-0000-0000-000000000000" required /></label>
        <label>Project<select value={projectId} onChange={(event) => setProjectId(event.target.value)} required>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label>
        <label>Scope<select value={scope} onChange={(event) => setScope(event.target.value as 'project' | 'thread')}><option value="project">Entire project</option><option value="thread">One thread</option></select></label>
        {scope === 'thread' ? <label>Thread<select value={threadId} onChange={(event) => setThreadId(event.target.value)} required>{threads.map((thread) => <option key={thread.id} value={thread.id}>{thread.title}</option>)}</select></label> : null}
        <label>Duration in hours<input type="number" min={1} max={24} value={hours} onChange={(event) => setHours(Number(event.target.value))} required /></label>
        <label>Reason<textarea value={reason} onChange={(event) => setReason(event.target.value)} maxLength={1000} required /></label>
        <button type="submit" disabled={busy || !projectId || (scope === 'thread' && !threadId) || !supportUserId || !reason.trim()}><Plus size={14} /> Grant temporary access</button>
      </form>
      <div className="remote-support-grants">
        <div><strong>Visible grants</strong><button type="button" aria-label="Refresh support grants" disabled={busy} onClick={() => { void load() }}><RefreshCw size={14} /></button></div>
        {grants.length === 0 ? <p>No support grants.</p> : <ul>{grants.map((grant) => {
          const active = !grant.revoked_at && Date.parse(grant.expires_at) > Date.now()
          return <li key={grant.id}><span><strong>{grant.thread_id ? 'Thread access' : 'Project access'}</strong><small>{grant.support_user_id}</small><small>{grant.reason}</small><small>{active ? `Expires ${new Date(grant.expires_at).toLocaleString()}` : 'Inactive'}</small></span>{active ? <button type="button" aria-label="Revoke support grant" disabled={busy} onClick={() => { void revoke(grant.id) }}><ShieldX size={14} /></button> : null}</li>
        })}</ul>}
      </div>
      </section>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
