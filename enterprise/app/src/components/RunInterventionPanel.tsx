import { Check, MessageSquareText, ShieldAlert, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

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
  const [interventions, setInterventions] = useState<{
    runId: string
    approvals: ApprovalRequest[]
    inputs: RunInputRequest[]
  }>({ runId, approvals: [], inputs: [] })
  const [responses, setResponses] = useState<Record<string, string>>({})
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const requestGeneration = useRef(0)
  const currentRunId = useRef(runId)
  const currentClient = useRef(client)

  useEffect(() => {
    currentRunId.current = runId
    currentClient.current = client
  }, [client, runId])

  const load = useCallback(async () => {
    const requestedRunId = runId
    const generation = ++requestGeneration.current
    try {
      const [nextApprovals, nextInputs] = await Promise.all([
        client.listApprovals(requestedRunId),
        client.listInputRequests(requestedRunId),
      ])
      if (requestGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) return
      setInterventions({ runId: requestedRunId, approvals: nextApprovals, inputs: nextInputs })
      setError(null)
    } catch (cause) {
      if (requestGeneration.current !== generation || currentRunId.current !== requestedRunId || currentClient.current !== client) return
      setError(messageOf(cause))
    }
  }, [client, runId])

  useEffect(() => {
    setInterventions({ runId, approvals: [], inputs: [] })
    setResponses({})
    setBusyId(null)
    setError(null)
    void load()
    return () => { requestGeneration.current += 1 }
  }, [load, refreshKey, runId])

  const { pendingApprovals, pendingInputs } = useMemo(() => {
    if (interventions.runId !== runId) return { pendingApprovals: [], pendingInputs: [] }
    return {
      pendingApprovals: interventions.approvals.filter((approval) => approval.state === 'pending'),
      pendingInputs: interventions.inputs.filter((input) => input.state === 'pending'),
    }
  }, [interventions, runId])

  const resolveApproval = async (approval: ApprovalRequest, decision: 'approved' | 'rejected') => {
    const targetRunId = interventions.runId
    if (targetRunId !== runId || approval.run_id !== targetRunId) return
    setBusyId(approval.id)
    setError(null)
    try {
      await client.resolveApproval(targetRunId, approval.id, approval.revision, decision)
      if (currentRunId.current !== targetRunId || currentClient.current !== client) return
      await load()
      if (currentRunId.current === targetRunId && currentClient.current === client) await onResolved?.()
    } catch (cause) {
      if (currentRunId.current !== targetRunId || currentClient.current !== client) return
      setError(messageOf(cause))
    } finally {
      if (currentRunId.current === targetRunId && currentClient.current === client) setBusyId(null)
    }
  }

  const submitInput = async (input: RunInputRequest) => {
    const targetRunId = interventions.runId
    if (targetRunId !== runId || input.run_id !== targetRunId) return
    const response = responses[input.id]?.trim()
    if (!response) return
    setBusyId(input.id)
    setError(null)
    try {
      await client.submitInputResponse(targetRunId, input.id, input.revision, response)
      if (currentRunId.current !== targetRunId || currentClient.current !== client) return
      setResponses((current) => ({ ...current, [input.id]: '' }))
      await load()
      if (currentRunId.current === targetRunId && currentClient.current === client) await onResolved?.()
    } catch (cause) {
      if (currentRunId.current !== targetRunId || currentClient.current !== client) return
      setError(messageOf(cause))
    } finally {
      if (currentRunId.current === targetRunId && currentClient.current === client) setBusyId(null)
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
