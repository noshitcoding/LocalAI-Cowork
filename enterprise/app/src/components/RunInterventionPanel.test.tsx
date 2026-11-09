import { act, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApprovalRequest } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RunInterventionPanel from './RunInterventionPanel'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

function approval(runId: string, id: string, action: string): ApprovalRequest {
  return {
    schema_version: 2, id, run_id: runId, revision: 1, etag: `W/"${id}:1"`,
    requested_action: action, state: 'pending', requested_at: '2026-08-30T12:00:00Z',
    expires_at: '2026-08-30T13:00:00Z', resolved_by_user_id: null, resolved_at: null,
  }
}

describe('RunInterventionPanel', () => {
  it('does not show a stale approval after the run changes', async () => {
    const runA = '10000000-0000-4000-8000-000000000401'
    const runB = '10000000-0000-4000-8000-000000000402'
    const approvalsA = deferred<ApprovalRequest[]>()
    const approvalsB = deferred<ApprovalRequest[]>()
    const client = {
      listApprovals: vi.fn((id: string) => id === runA ? approvalsA.promise : approvalsB.promise),
      listInputRequests: vi.fn(async () => []),
    } as unknown as RemoteRuntimeClient
    const view = render(<RunInterventionPanel client={client} runId={runA} />)
    view.rerender(<RunInterventionPanel client={client} runId={runB} />)

    await act(async () => approvalsB.resolve([
      approval(runB, '10000000-0000-4000-8000-000000000403', 'Approve B'),
    ]))
    expect(await screen.findByText('Approve B')).toBeInTheDocument()
    await act(async () => approvalsA.resolve([
      approval(runA, '10000000-0000-4000-8000-000000000404', 'Approve A'),
    ]))
    expect(screen.queryByText('Approve A')).not.toBeInTheDocument()
    expect(screen.getByText('Approve B')).toBeInTheDocument()
  })
})
