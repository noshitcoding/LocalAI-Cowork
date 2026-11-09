import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ManagementClient } from '../runtime/managementClient'
import EnterpriseRollouts from './EnterpriseRollouts'
import i18n from '../i18n'

const mocks = { rollouts: vi.fn(), instances: vi.fn(), releases: vi.fn(), createRollout: vi.fn(), controlRollout: vi.fn() }
const client = mocks as unknown as ManagementClient
const instance = (id: string) => ({ id, name: 'Instance ' + id, organisation_id: 'org', revision: 7, can_manage: true, rollout_id: null, state: 'ready', backend: 'compose', current_release: 'a'.repeat(64), available_actions: ['stop'], policy: { minimum_isolation: 'container' } })
const release = { id: 'b'.repeat(64), name: 'Approved runner', source_release: 'a'.repeat(64), qualified: true }
const plan = { id: 'plan', organisation_id: 'org', name: 'Team rollout', revision: 4, release: release.id, batch_size: 1, state: 'running', error_code: null, targets: [{ instance_id: 'one', instance_name: 'Instance one', ordinal: 0, state: 'active', next_action: 'stop', error_code: null }] }

describe('enterprise batch updates', () => {
  beforeEach(async () => {
    vi.resetAllMocks()
    await i18n.changeLanguage('en')
    mocks.instances.mockResolvedValue([instance('one'), instance('two')])
    mocks.rollouts.mockResolvedValue([])
    mocks.releases.mockResolvedValue([release])
    mocks.createRollout.mockResolvedValue({ id: 'new', state: 'running' })
    mocks.controlRollout.mockResolvedValue({ id: 'plan', state: 'paused' })
  })
  afterEach(() => { vi.useRealTimers() })

  it('freezes selection order and revisions and requires downtime acknowledgement', async () => {
    render(<EnterpriseRollouts client={client} orgId="org" canManage />)
    fireEvent.click(await screen.findByLabelText('Instance two · ready'))
    fireEvent.click(screen.getByLabelText('Instance one · ready'))
    await screen.findByRole('option', { name: /Approved runner/ })
    fireEvent.change(screen.getByLabelText('Rollout name'), { target: { value: 'Team rollout' } })
    fireEvent.change(screen.getByLabelText('Qualified release'), { target: { value: release.id } })
    expect(screen.getByRole('button', { name: 'Start rollout' })).toBeDisabled()
    fireEvent.click(screen.getByLabelText(/Allow maintenance downtime/))
    fireEvent.click(screen.getByRole('button', { name: 'Start rollout' }))
    await waitFor(() => expect(mocks.createRollout).toHaveBeenCalledWith('org', expect.objectContaining({ name: 'Team rollout', release: release.id, batch_size: 1, targets: [{ instance_id: 'two', expected_revision: 7 }, { instance_id: 'one', expected_revision: 7 }] })))
  })

  it('requires the chosen release to be qualified for each exact installed source', async () => {
    mocks.releases.mockImplementation((value: { id: string }) => Promise.resolve(value.id === 'one' ? [release] : [{ ...release, source_release: 'c'.repeat(64) }]))
    render(<EnterpriseRollouts client={client} orgId="org" canManage />)
    fireEvent.click(await screen.findByLabelText('Instance one · ready'))
    fireEvent.click(screen.getByLabelText('Instance two · ready'))
    await waitFor(() => expect(mocks.releases).toHaveBeenCalledTimes(3))
    expect(screen.queryByRole('option', { name: /Approved runner/ })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Start rollout' })).toBeDisabled()
  })

  it('invalidates selected revisions when another administrator changes an instance', async () => {
    render(<EnterpriseRollouts client={client} orgId="org" canManage />)
    fireEvent.click(await screen.findByLabelText('Instance one · ready'))
    await screen.findByRole('option', { name: /Approved runner/ })
    // The next poll must not silently replace the selected revision.
    mocks.instances.mockResolvedValue([{ ...instance('one'), revision: 8 }, instance('two')])
    await screen.findByText('The selected instances changed. Clear the selection and choose them again.', {}, { timeout: 4500 })
    expect(screen.getByRole('button', { name: 'Start rollout' })).toBeDisabled()
    expect(mocks.createRollout).not.toHaveBeenCalled()
  })

  it('keeps auditor rollouts read-only and ignores a late organisation response', async () => {
    let finish!: (value: unknown) => void
    mocks.rollouts.mockImplementation((org: string) => org === 'org' ? new Promise(resolve => { finish = resolve }) : Promise.resolve([]))
    const view = render(<EnterpriseRollouts key="org" client={client} orgId="org" canManage={false} />)
    await waitFor(() => expect(mocks.rollouts).toHaveBeenCalled())
    view.rerender(<EnterpriseRollouts key="other" client={client} orgId="other" canManage={false} />)
    await act(async () => { finish([plan]) })
    await screen.findByText('No rollouts yet.')
    expect(screen.queryByText('Team rollout')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Start rollout' })).not.toBeInTheDocument()
  })

  it('requires a confirmation for cancellation and explains in-flight operations', async () => {
    mocks.rollouts.mockResolvedValue([plan])
    render(<EnterpriseRollouts client={client} orgId="org" canManage />)
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel rollout' }))
    expect(screen.getByText(/Already running operations finish or require reconciliation/)).toBeVisible()
    expect(mocks.controlRollout).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rollout action' }))
    await waitFor(() => expect(mocks.controlRollout).toHaveBeenCalledWith(plan, 'cancel'))
  })
})
