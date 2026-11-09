import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ManagementClient, ManagedInstance } from '../runtime/managementClient'
import type { InstanceMember } from '../runtime/enterpriseContracts'
import i18n from '../i18n'
import EnterpriseInstanceMembers from './EnterpriseInstanceMembers'

const instance = { id: 'one', organisation_id: 'org', name: 'Shared workspace', can_manage: true } as ManagedInstance
const owner = { user_id: 'owner', display_name: 'Owner', email: 'owner@example.test', role: 'owner', organisation_role: 'member' } as InstanceMember
const reader = { user_id: 'reader', display_name: 'Reader', email: 'reader@example.test', role: 'viewer', organisation_role: 'member' } as InstanceMember
const mocks = { instanceMembers: vi.fn(), instanceMemberCandidates: vi.fn(), setInstanceMember: vi.fn(), removeInstanceMember: vi.fn() }
const client = mocks as unknown as ManagementClient
describe('instance membership management', () => {
  beforeEach(async () => {
    vi.resetAllMocks(); await i18n.changeLanguage('en')
    mocks.instanceMembers.mockResolvedValue([owner, reader])
    mocks.instanceMemberCandidates.mockResolvedValue([reader])
    mocks.setInstanceMember.mockResolvedValue({ user_id: 'reader', role: 'member' })
    mocks.removeInstanceMember.mockResolvedValue({ user_id: 'reader', removed: true })
  })
  it('defaults new assignments to read-only and scopes every mutation to the displayed instance', async () => {
    const changed = vi.fn()
    render(<EnterpriseInstanceMembers client={client} instance={instance} onClose={vi.fn()} onChanged={changed} />)
    await waitFor(() => expect(screen.getByLabelText('Organisation member')).toBeEnabled())
    expect(screen.getByRole('button', { name: 'Save instance role' })).toBeDisabled()
    expect(screen.getByLabelText('Instance role')).toHaveValue('viewer')
    fireEvent.change(screen.getByLabelText('Organisation member'), { target: { value: 'reader' } })
    fireEvent.change(screen.getByLabelText('Instance role'), { target: { value: 'member' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save instance role' }))
    await waitFor(() => expect(mocks.setInstanceMember).toHaveBeenCalledWith(instance, 'reader', 'member'))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove access for Reader' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'Remove access for Reader' }))
    await waitFor(() => expect(mocks.removeInstanceMember).toHaveBeenCalledWith(instance, 'reader'))
    expect(screen.queryByRole('button', { name: 'Remove access for Owner' })).not.toBeInTheDocument()
    expect(changed).toHaveBeenCalled()
  })
  it('does not fetch the assignment directory or expose write controls to readers', async () => {
    render(<EnterpriseInstanceMembers client={client} instance={{ ...instance, can_manage: false }} onClose={vi.fn()} onChanged={vi.fn()} />)
    await screen.findByRole('cell', { name: /reader@example.test/ })
    expect(mocks.instanceMemberCandidates).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: 'Save instance role' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Remove access for Reader' })).not.toBeInTheDocument()
  })
  it('discards a directory response after switching the instance', async () => {
    let complete!: (rows: InstanceMember[]) => void
    mocks.instanceMembers.mockReturnValueOnce(new Promise(resolve => { complete = resolve })).mockResolvedValue([])
    mocks.instanceMemberCandidates.mockResolvedValue([])
    const props = { client, onClose: vi.fn(), onChanged: vi.fn() }
    const { rerender } = render(<EnterpriseInstanceMembers key="one" {...props} instance={instance} />)
    rerender(<EnterpriseInstanceMembers key="two" {...props} instance={{ ...instance, id: 'two', name: 'Other workspace' }} />)
    await act(async () => { complete([owner, reader]) })
    expect(screen.queryByRole('cell', { name: /reader@example.test/ })).not.toBeInTheDocument()
    expect(screen.getByRole('heading')).toHaveTextContent('Other workspace')
  })
  it('removes write controls after central management authority is lost', async () => {
    const props = { client, onClose: vi.fn(), onChanged: vi.fn() }
    const { rerender } = render(<EnterpriseInstanceMembers {...props} instance={instance} />)
    await waitFor(() => expect(screen.getByLabelText('Organisation member')).toBeEnabled())
    rerender(<EnterpriseInstanceMembers {...props} instance={{ ...instance, can_manage: false }} />)
    expect(screen.queryByRole('button', { name: 'Save instance role' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Remove access for Reader' })).not.toBeInTheDocument()
  })
})
