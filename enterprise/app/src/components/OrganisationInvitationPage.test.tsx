import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import { pendingInvitation, rememberInvitation } from '../runtime/organisationInvitation'

const mocks = vi.hoisted(() => ({ inspect: vi.fn(), accept: vi.fn(), register: vi.fn(), restore: vi.fn(), setConnection: vi.fn(), logout: vi.fn(), status: 'signed_out', serverUrl: 'https://management.example', email: 'invited@example.test' }))
vi.mock('../runtime/authClient', () => ({ RemoteAuthClient: class { inspectOrganisationInvitation = mocks.inspect } }))
vi.mock('../runtime/managementClient', () => ({ ManagementClient: class { acceptInvitation = mocks.accept } }))
vi.mock('../stores/remoteRuntimeStore', () => ({ useRemoteRuntimeStore: () => ({ ...mocks, registerOrganisationInvitation: mocks.register }) }))
vi.mock('./RemoteServerView', () => ({ default: () => <div>Existing sign-in options</div> }))
vi.mock('./ManagementMfaGate', () => ({ default: ({ children }: { children: import('react').ReactNode }) => children }))
import OrganisationInvitationPage from './OrganisationInvitationPage'

const token = 't'.repeat(43)
const preview = { id: 'invitation-a', organisation_id: 'org-a', organisation_name: 'Team A', email: 'invited@example.test', role: 'member', account_exists: false, expires_at: '2099-01-01T00:00:00Z' }
const page = () => <MemoryRouter><OrganisationInvitationPage /></MemoryRouter>

describe('organisation invitation acceptance', () => {
  beforeEach(async () => {
    vi.resetAllMocks(); sessionStorage.clear(); await i18n.changeLanguage('en')
    window.history.replaceState(null, '', `/join#invitation=${token}`)
    mocks.status = 'signed_out'; mocks.serverUrl = 'https://management.example'; mocks.email = preview.email
    mocks.inspect.mockResolvedValue(preview); mocks.restore.mockResolvedValue(false)
    mocks.register.mockResolvedValue(undefined); mocks.accept.mockResolvedValue({ organisation_id: preview.organisation_id })
  })
  it('registers a new account with the bound token and removes the fragment', async () => {
    render(page())
    await screen.findByRole('heading', { name: 'Team A' })
    expect(window.location.hash).toBe('')
    expect(pendingInvitation(mocks.serverUrl)).toBe(token)
    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: 'New user' } })
    fireEvent.change(screen.getByLabelText('New password'), { target: { value: 'long-test-password' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }))
    expect(await screen.findByRole('status')).toHaveTextContent('You have joined')
    expect(mocks.register).toHaveBeenCalledWith(mocks.serverUrl, preview.email, 'New user', 'long-test-password', token)
    expect(pendingInvitation(mocks.serverUrl)).toBe('')
    expect(screen.getByRole('link', { name: 'Open organisation' })).toHaveAttribute('href', '/management?organisation=org-a')
  })
  it('requires the invited identity on the same server before accepting', async () => {
    mocks.inspect.mockResolvedValue({ ...preview, account_exists: true })
    mocks.status = 'authenticated'; mocks.email = 'wrong@example.test'
    const view = render(page())
    await screen.findByRole('heading', { name: 'Team A' })
    expect(screen.queryByRole('button', { name: 'Join organisation' })).not.toBeInTheDocument()
    mocks.email = preview.email; mocks.serverUrl = 'https://other.example'
    view.rerender(page())
    expect(screen.queryByRole('button', { name: 'Join organisation' })).not.toBeInTheDocument()
    mocks.serverUrl = 'https://management.example'; view.rerender(page())
    fireEvent.click(screen.getByRole('button', { name: 'Join organisation' }))
    await screen.findByRole('status')
    expect(mocks.accept).toHaveBeenCalledWith(token)
    expect(mocks.register).not.toHaveBeenCalled()
  })
  it('resumes a server-bound invitation after a login redirect', async () => {
    window.history.replaceState(null, '', '/join')
    rememberInvitation(mocks.serverUrl, token, preview.expires_at)
    mocks.inspect.mockResolvedValue({ ...preview, account_exists: true })
    render(page())
    await screen.findByText('Existing sign-in options')
    expect(mocks.inspect).toHaveBeenCalledWith(token, expect.any(AbortSignal))
    expect(screen.queryByLabelText('New password')).not.toBeInTheDocument()
  })
  it('discards a preview in flight when the server changes', async () => {
    let finish!: (value: unknown) => void
    mocks.inspect.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    render(page())
    await waitFor(() => expect(mocks.inspect).toHaveBeenCalled())
    fireEvent.change(screen.getByLabelText('Server address'), { target: { value: 'https://other.example' } })
    await act(async () => finish(preview))
    expect(screen.queryByRole('heading', { name: 'Team A' })).not.toBeInTheDocument()
    expect(screen.getByLabelText('Invitation token')).toHaveValue('')
    expect(mocks.inspect).toHaveBeenCalledTimes(1)
  })
  it('shows an expired or consumed invitation without a signup form', async () => {
    mocks.inspect.mockRejectedValue(new Error('Invitation unavailable'))
    render(page())
    expect(await screen.findByRole('alert')).toHaveTextContent('Invitation unavailable')
    expect(screen.queryByRole('button', { name: 'Create account and join' })).not.toBeInTheDocument()
    expect(pendingInvitation(mocks.serverUrl)).toBe('')
  })

  it('replaces an old invitation when another link changes only the fragment', async () => {
    mocks.inspect.mockRejectedValueOnce(new Error('Invitation unavailable'))
    render(page())
    await screen.findByRole('alert')
    const secondToken = 's'.repeat(43)
    window.history.replaceState(null, '', `/join#invitation=${secondToken}`)
    fireEvent(window, new HashChangeEvent('hashchange'))
    await screen.findByRole('heading', { name: 'Team A' })
    expect(mocks.inspect).toHaveBeenLastCalledWith(secondToken, expect.any(AbortSignal))
    expect(window.location.hash).toBe('')
    expect(pendingInvitation(mocks.serverUrl)).toBe(secondToken)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })
})
