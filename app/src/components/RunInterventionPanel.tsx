import { Check, MessageSquareText, ShieldAlert, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'

import type { ApprovalRequest, RunInputRequest } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type RunInterventionPanelProps = {
  client: RemoteRuntimeClient
  runId: string
  refreshKey?: number
  onResolved?: () => void | Promise<void>
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function describe(value: unknown): string {
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  if (value === null || value === undefined) return ''
  try { return JSON.stringify(value, null, 2) } catch { return String(value) }
}

export default function RunInterventionPanel({
  client,
  runId,
  refreshKey = 0,
  onResolved,
}: RunInterventionPanelProps) {
  const [approvals, setApprovals] = useState<ApprovalRequest[]>([])
  const [inputs, setInputs] = useState<RunInputRequest[]>([])
  const [responses, setResponses] = useState<Record<string, string>>({})
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextApprovals, nextInputs] = await Promise.all([
        client.listApprovals(runId),
        client.listInputRequests(runId),
      ])
      setApprovals(nextApprovals)
      setInputs(nextInputs)
      setError(null)
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client, runId])

  useEffect(() => { void load() }, [load, refreshKey])

  const pendingApprovals = useMemo(
    () => approvals.filter((approval) => approval.state === 'pending'),
    [approvals],
  )
  const pendingInputs = useMemo(
    () => inputs.filter((input) => input.state === 'pending'),
    [inputs],
  )

  const resolveApproval = async (approval: ApprovalRequest, decision: 'approved' | 'rejected') => {
    setBusyId(approval.id)
    setError(null)
    try {
      await client.resolveApproval(runId, approval.id, approval.revision, decision)
      await load()
      await onResolved?.()
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusyId(null)
    }
  }

  const submitInput = async (input: RunInputRequest) => {
    const response = responses[input.id]?.trim()
    if (!response) return
    setBusyId(input.id)
    setError(null)
    try {
      await client.submitInputResponse(runId, input.id, input.revision, response)
      setResponses((current) => ({ ...current, [input.id]: '' }))
      await load()
      await onResolved?.()
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusyId(null)
    }
  }

  if (pendingApprovals.length === 0 && pendingInputs.length === 0 && !error) return null
  return (
    <section className="run-intervention-panel" aria-label="Run decisions and questions">
      <header><div><ShieldAlert size={16} /><h2>Action required</h2></div><span>{pendingApprovals.length + pendingInputs.length}</span></header>
      {pendingApprovals.map((approval) => (
        <article key={approval.id} className="run-intervention-card">
          <div className="run-intervention-title"><ShieldAlert size={15} /><strong>Approval requested</strong><time>Expires {new Date(approval.expires_at).toLocaleString()}</time></div>
          <pre>{describe(approval.requested_action)}</pre>
          <div className="run-intervention-actions">
            <button className="ui-button ui-button--danger ui-button--sm" type="button" disabled={busyId === approval.id} onClick={() => { void resolveApproval(approval, 'rejected') }}><X size={14} /> Reject</button>
            <button className="ui-button ui-button--primary ui-button--sm" type="button" disabled={busyId === approval.id} onClick={() => { void resolveApproval(approval, 'approved') }}><Check size={14} /> Approve</button>
          </div>
        </article>
      ))}
      {pendingInputs.map((input) => (
        <article key={input.id} className="run-intervention-card">
          <div className="run-intervention-title"><MessageSquareText size={15} /><strong>Agent question</strong><time>Expires {new Date(input.expires_at).toLocaleString()}</time></div>
          <pre>{describe(input.prompt)}</pre>
          <label>
            Response
            <textarea value={responses[input.id] ?? ''} onChange={(event) => setResponses((current) => ({ ...current, [input.id]: event.target.value }))} rows={3} />
          </label>
          <div className="run-intervention-actions">
            <button className="ui-button ui-button--primary ui-button--sm" type="button" disabled={busyId === input.id || !(responses[input.id]?.trim())} onClick={() => { void submitInput(input) }}><Check size={14} /> Send response</button>
          </div>
        </article>
      ))}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
