import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import type { ManagementClient } from '../runtime/managementClient'
import EnterpriseInvitations from './EnterpriseInvitations'

const calls = { invitations: vi.fn(), createInvitation: vi.fn(), revokeInvitation: vi.fn() }
const client = calls as unknown as ManagementClient
const record = { id: 'i', organisation_id: 'a', email: 'invited@example.test', role: 'member', expires_at: '2099-01-01T00:00:00Z', state: 'pending' }
const secret = { ...record, token: 's'.repeat(43) }
const panel = (org: string) => <EnterpriseInvitations key={org} client={client} orgId={org} serverUrl="https://management.example" />

describe('organisation invitations administration', () => {
  beforeEach(async () => { vi.resetAllMocks(); await i18n.changeLanguage('en'); calls.invitations.mockResolvedValue([]); calls.createInvitation.mockResolvedValue(secret); calls.revokeInvitation.mockResolvedValue({}) })
  it('creates a link, displays the secret once and revokes it', async () => {
    render(panel('a'))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh invitations' })).toBeEnabled())
    fireEvent.change(screen.getByLabelText('Invited email'), { target: { value: record.email } })
    calls.invitations.mockResolvedValue([record])
    fireEvent.click(screen.getByRole('button', { name: 'Create invitation' }))
    const field = await screen.findByLabelText('Private invitation link')
    expect(field).toHaveValue(`https://management.example/join#invitation=${secret.token}`)
    expect(calls.createInvitation).toHaveBeenCalledWith('a', { email: record.email, role: 'member', expires_at: expect.any(String) })
    fireEvent.click(await screen.findByRole('button', { name: `Revoke invitation for ${record.email}` }))
    await waitFor(() => expect(field).not.toBeInTheDocument())
    expect(calls.revokeInvitation).toHaveBeenCalledWith('a', 'i')
  })
  it('does not reveal a stale creation response after switching organisations', async () => {
    let finish!: (value: unknown) => void
    calls.createInvitation.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const view = render(panel('a'))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh invitations' })).toBeEnabled())
    fireEvent.change(screen.getByLabelText('Invited email'), { target: { value: record.email } })
    fireEvent.click(screen.getByRole('button', { name: 'Create invitation' }))
    view.rerender(panel('b'))
    await act(async () => finish(secret))
    expect(screen.queryByLabelText('Private invitation link')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Invited email')).toHaveValue('')
  })
  it('keeps rejected creation visible without inventing a link', async () => {
    calls.createInvitation.mockRejectedValue(new Error('Organisation quota reached'))
    render(panel('a'))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh invitations' })).toBeEnabled())
    fireEvent.change(screen.getByLabelText('Invited email'), { target: { value: record.email } })
    fireEvent.click(screen.getByRole('button', { name: 'Create invitation' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Organisation quota reached')
    expect(screen.queryByLabelText('Private invitation link')).not.toBeInTheDocument()
  })
})
