import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteGovernancePanel from './RemoteGovernancePanel'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((next) => { resolve = next })
  return { promise, resolve }
}

const userId = '10000000-0000-4000-8000-000000000101'
const firstProjectId = '10000000-0000-4000-8000-000000000102'
const secondProjectId = '10000000-0000-4000-8000-000000000103'

describe('RemoteGovernancePanel', () => {
  it('uses the organisation directory and does not offer self approval', async () => {
    const client = {
      getContentAccess: vi.fn(async () => ({ managed: true, read_only: false, administrators: [{ user_id: userId, display_name: 'Self', email: 'self@example.test' }, { user_id: secondProjectId, display_name: 'Support', email: 'support@example.test' }] })),
      listProjects: vi.fn(async () => [{ id: firstProjectId, name: 'Project' }]),
      listSupportGrants: vi.fn(async () => []),
      getQuota: vi.fn(async () => { throw new Error('Not configured') }),
      createSupportGrant: vi.fn(async () => ({})),
    } as unknown as RemoteRuntimeClient
    render(<RemoteGovernancePanel client={client} currentUserId={userId} />)
    fireEvent.click(screen.getByRole('button', { name: 'Governance' }))
    await screen.findByRole('option', { name: 'Support · support@example.test' })
    expect(screen.queryByRole('option', { name: /Self/ })).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Support administrator'), { target: { value: secondProjectId } })
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'Investigate this project' } })
    fireEvent.click(screen.getByRole('button', { name: 'Grant temporary access' }))
    await vi.waitFor(() => expect(client.createSupportGrant).toHaveBeenCalledWith(expect.objectContaining({ project_id: firstProjectId, thread_id: null, support_user_id: secondProjectId })))
  })
  it('hides grant creation and operational controls in a support-only session', async () => {
    const client = {
      getContentAccess: vi.fn(async () => ({ managed: true, read_only: true, administrators: [] })),
      listProjects: vi.fn(async () => []), listSupportGrants: vi.fn(async () => []), getQuota: vi.fn(),
    } as unknown as RemoteRuntimeClient
    render(<RemoteGovernancePanel client={client} currentUserId={userId} />)
    fireEvent.click(screen.getByRole('button', { name: 'Governance' }))
    await vi.waitFor(() => expect(screen.queryByRole('button', { name: 'Grant temporary access' })).not.toBeInTheDocument())
    expect(screen.queryByRole('button', { name: 'Save limits' })).not.toBeInTheDocument()
    expect(client.getQuota).not.toHaveBeenCalled()
  })
  it('ignores a late thread list from the previously selected project', async () => {
    const firstThreads = deferred<Array<Record<string, unknown>>>()
    const secondThreads = deferred<Array<Record<string, unknown>>>()
    const client = {
      getContentAccess: vi.fn(async () => ({ managed: false, read_only: false })),
      listProjects: vi.fn(async () => [
        { id: firstProjectId, name: 'First project' },
        { id: secondProjectId, name: 'Second project' },
      ]),
      listSupportGrants: vi.fn(async () => []),
      getQuota: vi.fn(async () => ({
        limits: {
          storage_bytes: null, concurrent_runs: null, monthly_tokens: null,
          monthly_cost_micros: null, hard_cost_limit: true,
        },
        usage: { storage_bytes: 0, running_runs: 0, tokens: 0, cost_micros: 0 },
      })),
      listProjectThreads: vi.fn((id: string) => (
        id === firstProjectId ? firstThreads.promise : secondThreads.promise
      )),
    } as unknown as RemoteRuntimeClient

    render(<RemoteGovernancePanel compact client={client} currentUserId={userId} />)
    fireEvent.click(screen.getByRole('button', { name: 'Governance' }))
    await screen.findByRole('option', { name: 'First project' })

    fireEvent.change(screen.getAllByLabelText('Scope')[1], { target: { value: 'thread' } })
    await vi.waitFor(() => expect(client.listProjectThreads).toHaveBeenCalledWith(firstProjectId))
    fireEvent.change(screen.getByLabelText('Project'), { target: { value: secondProjectId } })
    await vi.waitFor(() => expect(client.listProjectThreads).toHaveBeenCalledWith(secondProjectId))

    secondThreads.resolve([{
      schema_version: 2, id: '10000000-0000-4000-8000-000000000104', revision: 1,
      etag: 'W/"thread:1"', project_id: secondProjectId, forked_from_thread_id: null,
      forked_from_message_id: null, title: 'Second thread', deleted_at: null,
    }])
    expect(await screen.findByRole('option', { name: 'Second thread' })).toBeInTheDocument()

    firstThreads.resolve([{
      schema_version: 2, id: '10000000-0000-4000-8000-000000000105', revision: 1,
      etag: 'W/"thread:1"', project_id: firstProjectId, forked_from_thread_id: null,
      forked_from_message_id: null, title: 'Stale first thread', deleted_at: null,
    }])
    await Promise.resolve()
    await Promise.resolve()

    expect(screen.queryByRole('option', { name: 'Stale first thread' })).not.toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'Second thread' })).toBeInTheDocument()
  })
})
