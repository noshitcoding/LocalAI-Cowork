import { useEffect, useState } from 'react'
import { useEngineStore } from '../stores/engineStore'
import { safeInvoke } from '../utils/safeInvoke'
import {
  normalizeEngineRunArtifact,
  normalizeEngineRunEvent,
  type EngineRunArtifactRow,
  type EngineRunEventRow,
} from '../utils/engineRunRecords'
import { tr } from '../i18n'

type EngineRunRow = {
  id: string
  parentRunId: string | null
  threadId: string | null
  sessionId: string | null
  title: string
  inputSummary: string | null
  source: string
  status: string
  phase: string
  cwd: string | null
  workspacePath: string | null
  model: string | null
  provider: string | null
  providerProfileId: string | null
  runtimeMode: string
  toolsetPolicyId: string | null
  channelKind: string | null
  channelRef: string | null
  retryCount: number
  resumedFromRunId: string | null
  checkpointJson: string | null
  resultSummary: string | null
  error: string | null
  updatedAt: string
  createdAt: string
}

type EngineRunCheckpointRow = {
  id: string
  runId: string
  label: string
  snapshotJson: string
  createdAt: string
}

type WorkerSandboxRow = {
  id: string
  runId: string
  backendId: string | null
  status: string
  mode: string
  sourceCwd: string
  workspaceRoot: string
  allowFileRead: boolean
  allowFileWrite: boolean
  allowShellExecution: boolean
  allowWebFetch: boolean
  allowWebSearch: boolean
  allowMcp: boolean
}

type RawRecord = Record<string, unknown>

const ISO_EPOCH = new Date(0).toISOString()

const formatDateTime = (value: string) => new Date(value).toLocaleString('de-DE')

const formatBool = (value: boolean) => tr(value ? "yes" : "no")

const asRecord = (value: unknown): RawRecord =>
  value && typeof value === 'object' ? value as RawRecord : {}

const asString = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback

const asNullableString = (value: unknown): string | null =>
  typeof value === 'string' ? value : null

const asNumber = (value: unknown, fallback = 0): number => {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

const asBoolean = (value: unknown, fallback = false): boolean =>
  typeof value === 'boolean' ? value : fallback

const asTimestampString = (primary: unknown, secondary?: unknown): string => {
  const value = asString(primary) || asString(secondary)
  return value || ISO_EPOCH
}

const normalizeRun = (value: unknown): EngineRunRow | null => {
  const row = asRecord(value)
  const id = asString(row.id)
  if (!id) return null

  return {
    id,
    parentRunId: asNullableString(row.parentRunId ?? row.parent_run_id),
    threadId: asNullableString(row.threadId ?? row.thread_id),
    sessionId: asNullableString(row.sessionId ?? row.session_id),
    title: asString(row.title, 'Untitleder Run'),
    inputSummary: asNullableString(row.inputSummary ?? row.input_summary),
    source: asString(row.source, 'desktop'),
    status: asString(row.status, 'unknown'),
    phase: asString(row.phase, 'unknown'),
    cwd: asNullableString(row.cwd),
    workspacePath: asNullableString(row.workspacePath ?? row.workspace_path),
    model: asNullableString(row.model),
    provider: asNullableString(row.provider),
    providerProfileId: asNullableString(row.providerProfileId ?? row.provider_profile_id),
    runtimeMode: asString(row.runtimeMode ?? row.runtime_mode, 'host'),
    toolsetPolicyId: asNullableString(row.toolsetPolicyId ?? row.toolset_policy_id),
    channelKind: asNullableString(row.channelKind ?? row.channel_kind),
    channelRef: asNullableString(row.channelRef ?? row.channel_ref),
    retryCount: asNumber(row.retryCount ?? row.retry_count),
    resumedFromRunId: asNullableString(row.resumedFromRunId ?? row.resumed_from_run_id),
    checkpointJson: asNullableString(row.checkpointJson ?? row.checkpoint_json),
    resultSummary: asNullableString(row.resultSummary ?? row.result_summary),
    error: asNullableString(row.error),
    updatedAt: asTimestampString(row.updatedAt ?? row.updated_at, row.createdAt ?? row.created_at),
    createdAt: asTimestampString(row.createdAt ?? row.created_at, row.updatedAt ?? row.updated_at),
  }
}

const normalizeCheckpoint = (value: unknown): EngineRunCheckpointRow | null => {
  const checkpoint = asRecord(value)
  const id = asString(checkpoint.id)
  if (!id) return null

  return {
    id,
    runId: asString(checkpoint.runId ?? checkpoint.run_id),
    label: asString(checkpoint.label, 'Checkpoint'),
    snapshotJson: asString(checkpoint.snapshotJson ?? checkpoint.snapshot_json),
    createdAt: asTimestampString(checkpoint.createdAt ?? checkpoint.created_at),
  }
}

const normalizeSandbox = (value: unknown): WorkerSandboxRow | null => {
  const sandbox = asRecord(value)
  const id = asString(sandbox.id)
  if (!id) return null

  return {
    id,
    runId: asString(sandbox.runId ?? sandbox.run_id),
    backendId: asNullableString(sandbox.backendId ?? sandbox.backend_id),
    status: asString(sandbox.status, 'unknown'),
    mode: asString(sandbox.mode, 'unknown'),
    sourceCwd: asString(sandbox.sourceCwd ?? sandbox.source_cwd),
    workspaceRoot: asString(sandbox.workspaceRoot ?? sandbox.workspace_root),
    allowFileRead: asBoolean(sandbox.allowFileRead ?? sandbox.allow_file_read),
    allowFileWrite: asBoolean(sandbox.allowFileWrite ?? sandbox.allow_file_write),
    allowShellExecution: asBoolean(sandbox.allowShellExecution ?? sandbox.allow_shell_execution),
    allowWebFetch: asBoolean(sandbox.allowWebFetch ?? sandbox.allow_web_fetch),
    allowWebSearch: asBoolean(sandbox.allowWebSearch ?? sandbox.allow_web_search),
    allowMcp: asBoolean(sandbox.allowMcp ?? sandbox.allow_mcp),
  }
}

export default function RunPanel() {
  const currentRunId = useEngineStore((state) => state.currentRunId)
  const [runs, setRuns] = useState<EngineRunRow[]>([])
  const [selectedRun, setSelectedRun] = useState<string | null>(null)
  const [events, setEvents] = useState<EngineRunEventRow[]>([])
  const [artifacts, setArtifacts] = useState<EngineRunArtifactRow[]>([])
  const [checkpoints, setCheckpoints] = useState<EngineRunCheckpointRow[]>([])
  const [sandbox, setSandbox] = useState<WorkerSandboxRow | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busyRunId, setBusyRunId] = useState<string | null>(null)

  const refreshRuns = async () => {
    setLoading(true)
    setError(null)
    try {
      const rows = await safeInvoke<unknown[] | null>('engine_run_list', { limit: 100 }, [])
      const safeRows = Array.isArray(rows)
        ? rows.map(normalizeRun).filter((row): row is EngineRunRow => row !== null)
        : []
      setRuns(safeRows)
      const nextSelected = selectedRun && safeRows.some((row) => row.id === selectedRun)
        ? selectedRun
        : safeRows[0]?.id ?? null
      setSelectedRun(nextSelected)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  const refreshCheckpoints = async (runId: string) => {
    try {
      const rows = await safeInvoke<unknown[] | null>('engine_run_checkpoint_list', {
        runId,
        limit: 20,
      }, [])
      setCheckpoints(
        Array.isArray(rows)
          ? rows.map(normalizeCheckpoint).filter((checkpoint): checkpoint is EngineRunCheckpointRow => checkpoint !== null)
          : [],
      )
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const refreshEvents = async (runId: string) => {
    try {
      const rows = await safeInvoke<unknown[] | null>('engine_run_event_list', {
        runId,
        limit: 200,
      }, [])
      setEvents(
        Array.isArray(rows)
          ? rows.map(normalizeEngineRunEvent).filter((event): event is EngineRunEventRow => event !== null)
          : [],
      )
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const refreshArtifacts = async (runId: string) => {
    try {
      const rows = await safeInvoke<unknown[] | null>('engine_run_artifact_list', {
        runId,
        limit: 100,
      }, [])
      setArtifacts(
        Array.isArray(rows)
          ? rows.map(normalizeEngineRunArtifact).filter((artifact): artifact is EngineRunArtifactRow => artifact !== null)
          : [],
      )
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const refreshSandbox = async (runId: string) => {
    try {
      const row = await safeInvoke<unknown>('worker_sandbox_get_for_run', { runId }, null)
      setSandbox(normalizeSandbox(row))
    } catch {
      setSandbox(null)
    }
  }

  useEffect(() => {
    void refreshRuns()
    // Refresh once on mount; manual refresh and actions keep the list current.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!selectedRun) {
      setEvents([])
      setArtifacts([])
      setCheckpoints([])
      setSandbox(null)
      return
    }
    void refreshEvents(selectedRun)
    void refreshArtifacts(selectedRun)
    void refreshCheckpoints(selectedRun)
    void refreshSandbox(selectedRun)
  }, [selectedRun])

  const handleAction = async (runId: string, action: 'cancel' | 'resume' | 'retry') => {
    setBusyRunId(runId)
    setError(null)
    try {
      let nextSelectedRun = runId
      if (action === 'cancel') {
        await safeInvoke('engine_run_cancel', { id: runId }, null)
      } else if (action === 'resume') {
        await safeInvoke('engine_run_resume', { id: runId }, null)
      } else {
        nextSelectedRun = await safeInvoke<string>('engine_run_retry', { id: runId })
      }
      await refreshRuns()
      setSelectedRun(nextSelectedRun)
      await Promise.all([
        refreshEvents(nextSelectedRun),
        refreshArtifacts(nextSelectedRun),
        refreshCheckpoints(nextSelectedRun),
        refreshSandbox(nextSelectedRun),
      ])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyRunId(null)
    }
  }

  return (
    <div className="panel">
      <div className="panel-heading-row">
        <h2>{tr("Runs")}</h2>
        <button type="button" className="btn-sm" onClick={() => void refreshRuns()}>{tr("Refresh")}</button>
      </div>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      <div style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 12 }}>{tr("Active run:")}{currentRunId ?? 'none'}
      </div>

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : runs.length === 0 ? (
        <p className="panel-empty">{tr("No runs saved yet.")}</p>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: '1.3fr 1fr', gap: 12 }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxHeight: 420, overflowY: 'auto' }}>
            {runs.map((run) => (
              <button
                key={run.id}
                type="button"
                className="card"
                onClick={() => setSelectedRun(run.id)}
                style={{
                  textAlign: 'left',
                  border: selectedRun === run.id ? '1px solid var(--accent-text)' : '1px solid var(--border-color)',
                  background: 'var(--bg-secondary)',
                }}
              >
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                  <strong style={{ fontSize: 13 }}>{run.title}</strong>
                  <span style={{ fontSize: 11, color: run.status === 'completed' ? 'var(--success)' : run.status === 'failed' ? 'var(--danger)' : 'var(--text-muted)' }}>
                    {run.status}
                  </span>
                </div>
                <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4 }}>
                  {formatDateTime(run.updatedAt)} - {run.source} - {run.phase}
                </div>
                <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4 }}>
                  {run.runtimeMode} {run.provider ? `- ${run.provider}` : ''} {run.model ? `- ${run.model}` : ''}
                </div>
                {run.inputSummary && (
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
                    {run.inputSummary.slice(0, 180)}
                  </div>
                )}
              </button>
            ))}
          </div>

          <div className="card" style={{ minHeight: 320 }}>
            {selectedRun ? (
              (() => {
                const run = runs.find((item) => item.id === selectedRun)
                if (!run) return <p className="panel-empty">{tr("Run not found.")}</p>
                return (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                    <div>
                      <strong>{run.title}</strong>
                      <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                        {run.id}
                      </div>
                    </div>
                    <div style={{ fontSize: 12 }}>
                      <div>{tr("Status:")} {run.status}</div>
                      <div>{tr("Phase:")} {run.phase}</div>
                      <div>{tr("Source:")} {run.source}</div>
                      <div>{tr("Runtime:")} {run.runtimeMode}</div>
                      <div>{tr("Provider:")} {run.provider ?? 'n/a'}</div>
                      <div>{tr("Profile:")} {run.providerProfileId ?? 'n/a'}</div>
                      <div>{tr("Model:")} {run.model ?? 'n/a'}</div>
                      <div>{tr("Tool Policy:")} {run.toolsetPolicyId ?? 'n/a'}</div>
                      <div>{tr("Workspace:")} {run.workspacePath ?? run.cwd ?? 'n/a'}</div>
                      {run.channelKind && <div>{tr("Channel:")} {run.channelKind}{run.channelRef ? ` / ${run.channelRef}` : ''}</div>}
                      {run.threadId && <div>{tr("Thread:")} {run.threadId}</div>}
                      <div>{tr("Retry Count:")} {run.retryCount}</div>
                      {run.parentRunId && <div>{tr("Parent:")} {run.parentRunId}</div>}
                    </div>
                    {sandbox && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>{tr("Sandbox")}</div>
                        <div style={{ fontSize: 11, color: 'var(--text-secondary)' }}>
                          <div>{tr("Status:")} {sandbox.status}</div>
                          <div>{tr("Mode:")} {sandbox.mode}</div>
                          <div>{tr("Backend:")} {sandbox.backendId ?? 'local'}</div>
                          <div>{tr("Workspace:")} {sandbox.workspaceRoot}</div>
                          <div>{tr("Source:")} {sandbox.sourceCwd}</div>
                          <div>
                            {tr("Permissions:")} {tr("read")} {formatBool(sandbox.allowFileRead)} / {tr("write")} {formatBool(sandbox.allowFileWrite)} / {tr("shell")} {formatBool(sandbox.allowShellExecution)} / {tr("web")} {formatBool(sandbox.allowWebFetch || sandbox.allowWebSearch)} / {tr("mcp")} {formatBool(sandbox.allowMcp)}
                          </div>
                        </div>
                      </div>
                    )}
                    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id || run.status === 'completed'} onClick={() => void handleAction(run.id, 'resume')}>{tr("Resume")}</button>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id} onClick={() => void handleAction(run.id, 'retry')}>{tr("Retry")}</button>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id || run.status === 'completed' || run.status === 'failed' || run.status === 'canceled'} onClick={() => void handleAction(run.id, 'cancel')}>{tr("Cancel")}</button>
                    </div>
                    {run.resultSummary && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>{tr("Result")}</div>
                        <pre style={{ fontSize: 11, whiteSpace: 'pre-wrap', background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', maxHeight: 140, overflowY: 'auto' }}>
                          {run.resultSummary}
                        </pre>
                      </div>
                    )}
                    {run.error && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4, color: 'var(--danger)' }}>{tr("Error")}</div>
                        <pre style={{ fontSize: 11, whiteSpace: 'pre-wrap', background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', maxHeight: 120, overflowY: 'auto', color: 'var(--danger)' }}>
                          {run.error}
                        </pre>
                      </div>
                    )}
                    <div>
                      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>{tr("Events")}</div>
                      {events.length === 0 ? (
                        <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{tr("No events")}</div>
                      ) : (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxHeight: 220, overflowY: 'auto' }}>
                          {events.map((event) => (
                            <div key={event.id} style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)' }}>
                              <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                                <strong>#{event.sequence} {event.eventType}</strong>
                                <span style={{ color: 'var(--text-muted)' }}>{formatDateTime(event.createdAt)}</span>
                              </div>
                              <div style={{ color: 'var(--text-secondary)', marginTop: 4 }}>{event.summary}</div>
                              {event.payloadJson && event.redactionLevel !== 'secret' && (
                                <pre style={{ whiteSpace: 'pre-wrap', margin: '6px 0 0', maxHeight: 120, overflowY: 'auto' }}>
                                  {event.payloadJson.slice(0, 800)}
                                </pre>
                              )}
                              {event.redactionLevel === 'secret' && (
                                <div style={{ color: 'var(--text-muted)', marginTop: 4 }}>{tr("Payload hidden")}</div>
                              )}
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                    <div>
                      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>{tr("Artifacts")}</div>
                      {artifacts.length === 0 ? (
                        <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{tr("No artifacts")}</div>
                      ) : (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxHeight: 160, overflowY: 'auto' }}>
                          {artifacts.map((artifact) => (
                            <div key={artifact.id} style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)' }}>
                              <div style={{ fontWeight: 600 }}>{artifact.title ?? artifact.kind}</div>
                              <div style={{ color: 'var(--text-muted)', marginTop: 3 }}>{artifact.kind} - {formatDateTime(artifact.createdAt)}</div>
                              <div style={{ color: 'var(--text-secondary)', marginTop: 4, overflowWrap: 'anywhere' }}>{artifact.path}</div>
                              {artifact.summary && <div style={{ color: 'var(--text-secondary)', marginTop: 4 }}>{artifact.summary}</div>}
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                    <div>
                      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>{tr("Checkpoints")}</div>
                      {checkpoints.length === 0 ? (
                        <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{tr("No Checkpoints")}</div>
                      ) : (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxHeight: 140, overflowY: 'auto' }}>
                          {checkpoints.map((checkpoint) => (
                            <div key={checkpoint.id} style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)' }}>
                              <div style={{ fontWeight: 600 }}>{checkpoint.label}</div>
                              <div style={{ color: 'var(--text-muted)', margin: '3px 0 6px' }}>{formatDateTime(checkpoint.createdAt)}</div>
                              <pre style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{checkpoint.snapshotJson.slice(0, 400)}</pre>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                )
              })()
            ) : (
              <p className="panel-empty">{tr("Run choose.")}</p>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
