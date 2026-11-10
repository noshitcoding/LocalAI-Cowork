import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ManagedInstance, ManagementClient } from '../runtime/managementClient'
import i18n from '../i18n'
import EnterpriseMemberAccess from './EnterpriseMemberAccess'
import EnterpriseMemberResources from './EnterpriseMemberResources'

const orgId = '10000000-0000-4000-8000-000000000001'
const userId = '10000000-0000-4000-8000-000000000002'
const resourceId = '10000000-0000-4000-8000-000000000003'
const firstId = '10000000-0000-4000-8000-000000000004'
const secondId = '10000000-0000-4000-8000-000000000005'
const environmentId = '10000000-0000-4000-8000-000000000006'
const member = { user_id: userId, display_name: 'Review member', email: 'member@example.test' }
const instance = (id = firstId, can_manage = true) => ({ id, organisation_id: orgId, name: id === firstId ? 'Research server' : 'Operations server', can_manage, policy: { minimum_isolation: 'container' } }) as ManagedInstance
const resource = { id: resourceId, kind: 'model', name: 'Research model', enabled: true, available: true, deleted_at: null }
const current = { organisation_id: orgId, user_id: userId, revision: 9, assignments: [{ resource_id: resourceId, instance_id: null, environment_id: null, available: true }] }

describe('member management boundaries', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })

  it('keeps organisation assignments distinct from instance access and saves the original revision', async () => {
    const request = vi.fn(async (path, _schema, body, method) => method === 'PUT' ? { ...current, revision: 10, assignments: body.assignments } : path.endsWith('/resources') ? { items: [resource] } : current)
    render(<EnterpriseMemberResources client={{ request } as unknown as ManagementClient} organisationId={orgId} member={member} instances={[instance()]} onClose={vi.fn()} />)
    expect(await screen.findByText('Entire organisation')).toBeInTheDocument()
    expect(screen.getByText(/Instance, runtime and project access must be granted separately/)).toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: 'Runtime scope' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: 'Remove assignment for Research model' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save assignments' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/resource-assignments`, expect.anything(), { expected_revision: 9, assignments: [] }, 'PUT'))
    expect(await screen.findByRole('status')).toHaveTextContent('Assignments saved centrally. The supported model subset is available to updated instances.')
  })

  it('discards an old instance environment response after the scope changes', async () => {
    let finishFirst!: (value: unknown) => void
    const request = vi.fn(async path => {
      if (path === `/organisations/${orgId}/instances/${firstId}/environments`) return new Promise(resolve => { finishFirst = resolve })
      if (path === `/organisations/${orgId}/instances/${secondId}/environments`) return [{ id: environmentId, state: 'running', spec: { name: 'Current runtime' } }]
      return path.endsWith('/resources') ? { items: [resource] } : current
    })
    render(<EnterpriseMemberResources client={{ request } as unknown as ManagementClient} organisationId={orgId} member={member} instances={[instance(), instance(secondId)]} onClose={vi.fn()} />)
    await screen.findByText('Entire organisation')
    fireEvent.change(screen.getByRole('combobox', { name: 'Instance scope' }), { target: { value: firstId } })
    await waitFor(() => expect(finishFirst).toBeTypeOf('function'))
    fireEvent.change(screen.getByRole('combobox', { name: 'Instance scope' }), { target: { value: secondId } })
    await screen.findByRole('option', { name: 'Current runtime' })
    await act(async () => finishFirst([{ id: resourceId, state: 'running', spec: { name: 'Old runtime' } }]))
    expect(screen.queryByRole('option', { name: 'Old runtime' })).not.toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'Current runtime' })).toBeInTheDocument()
  })

  it('preserves the selected runtime scope in an explicit assignment write', async () => {
    const request = vi.fn(async (path, _schema, body, method) => method === 'PUT' ? { ...current, revision: 10, assignments: body.assignments } : path.endsWith('/environments') ? [{ id: environmentId, state: 'running', spec: { name: 'Research sandbox' } }] : path.endsWith('/resources') ? { items: [resource] } : { ...current, assignments: [] })
    render(<EnterpriseMemberResources client={{ request } as unknown as ManagementClient} organisationId={orgId} member={member} instances={[instance()]} onClose={vi.fn()} />)
    await screen.findByRole('option', { name: 'Research model · model' })
    fireEvent.change(screen.getByRole('combobox', { name: 'Resource' }), { target: { value: resourceId } })
    fireEvent.change(screen.getByRole('combobox', { name: 'Instance scope' }), { target: { value: firstId } })
    await screen.findByRole('option', { name: 'Research sandbox' })
    fireEvent.change(screen.getByRole('combobox', { name: 'Runtime scope' }), { target: { value: environmentId } })
    fireEvent.click(screen.getByRole('button', { name: 'Add assignment' }))
    expect(request.mock.calls.some(call => call[3] === 'PUT')).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Save assignments' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/resource-assignments`, expect.anything(), { expected_revision: 9, assignments: [{ resource_id: resourceId, instance_id: firstId, environment_id: environmentId }] }, 'PUT'))
  })

  it('never offers to modify an instance owner or an instance the operator cannot manage', async () => {
    const client = { instanceMembers: vi.fn(async (value: ManagedInstance) => [{ user_id: userId, role: value.id === firstId ? 'owner' : 'viewer' }]), setInstanceMember: vi.fn(), removeInstanceMember: vi.fn() }
    render(<EnterpriseMemberAccess client={client as unknown as ManagementClient} member={member} instances={[instance(), instance(secondId, false)]} onClose={vi.fn()} />)
    await screen.findByRole('option', { name: 'Instance owner' })
    expect(screen.getAllByRole('combobox', { name: 'Access for this member' }).every(element => element.hasAttribute('disabled'))).toBe(true)
    expect(screen.getAllByRole('button', { name: 'Save instance access' }).every(element => element.hasAttribute('disabled'))).toBe(true)
    expect(client.setInstanceMember).not.toHaveBeenCalled()
    expect(client.removeInstanceMember).not.toHaveBeenCalled()
  })

  it('writes an instance role only for the chosen member and server', async () => {
    const target = instance()
    const client = { instanceMembers: vi.fn(async () => [{ user_id: userId, role: 'viewer' }]), setInstanceMember: vi.fn().mockResolvedValue(undefined), removeInstanceMember: vi.fn() }
    render(<EnterpriseMemberAccess client={client as unknown as ManagementClient} member={member} instances={[target]} onClose={vi.fn()} />)
    const select = await screen.findByRole('combobox', { name: 'Access for this member' })
    fireEvent.change(select, { target: { value: 'member' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save instance access' }))
    await waitFor(() => expect(client.setInstanceMember).toHaveBeenCalledWith(target, userId, 'member'))
    expect(client.removeInstanceMember).not.toHaveBeenCalled()
  })
})
