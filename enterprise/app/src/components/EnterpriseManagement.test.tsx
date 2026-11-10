import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import i18n from '../i18n'

const mocks = vi.hoisted(() => ({ sso:vi.fn(), provisioningReleases:vi.fn(), createInstance:vi.fn(), invitations: vi.fn(), organisations: vi.fn(), instances: vi.fn(), request: vi.fn(), backups: vi.fn(), releases: vi.fn(), rollbacks: vi.fn(), action: vi.fn() }))
vi.mock('../stores/remoteRuntimeStore', () => ({ useRemoteRuntimeStore: () => ({ status: 'authenticated', serverUrl: 'https://management.example', userId: 'user' }) }))
vi.mock('../runtime/managementClient', () => ({ ManagementClient: class { sso=mocks.sso; provisioningReleases=mocks.provisioningReleases; createInstance=mocks.createInstance; invitations = mocks.invitations; organisations = mocks.organisations; instances = mocks.instances; request = mocks.request; backups = mocks.backups; releases = mocks.releases; rollbacks = mocks.rollbacks; action = mocks.action } }))
vi.mock('./RemoteServerView', () => ({ default: ({ managementContent }: { managementContent: import('react').ReactNode }) => managementContent ?? <div>Sign in</div> }))
import EnterpriseManagement from './EnterpriseManagement'

const policy = { revision: 1, extensions: 'catalog', content_access: 'support_grant', minimum_isolation: 'container', allow_laptop_full_access: false, allow_offline_continuation: true, allowed_network_origins: [], limits: { cpu_millis: 2000, memory_mib: 1024, storage_gib: 10, instances: 2, concurrent_runs: 1 }, audit_retention_days: 365, content_logging: false }
const a = { id: 'a', name: 'Organisation A', role: 'admin', policy }
const b = { id: 'b', name: 'Organisation B', role: 'member', policy }
const instance = { id: 'instance-a', organisation_id: 'a', owner_user_id: 'user', name: 'Private workspace A', backend: 'compose', state: 'pending', policy, revision: 1, can_manage: true, available_actions: ['provision'] }

describe('enterprise management', () => {
  beforeEach(async () => {
    vi.resetAllMocks()
    await i18n.changeLanguage('en')
    mocks.organisations.mockResolvedValue({ platform_admin: false, items: [a, b] })
    mocks.instances.mockResolvedValue([instance])
    mocks.invitations.mockResolvedValue([])
    mocks.request.mockResolvedValue([])
    mocks.backups.mockResolvedValue([])
    mocks.releases.mockResolvedValue([])
    mocks.rollbacks.mockResolvedValue([])
    mocks.action.mockResolvedValue({ id: 'operation-a' })
    mocks.sso.mockResolvedValue({ revision: 1, enabled: false, available: false, providers: [], group_mappings: [], provider_id: null, auto_provision: false })
  })

  it('requires choosing an initial release and sends its immutable id', async () => {
    const release={id:'a'.repeat(64),name:'Approved release'}
    mocks.organisations.mockResolvedValue({platform_admin:false,release_selection_enabled:true,items:[a,b]})
    mocks.provisioningReleases.mockResolvedValue([release])
    mocks.createInstance.mockResolvedValue({id:instance.id,state:'pending'})
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Private workspace A' })
    fireEvent.click(screen.getByText('Create a new instance'))
    fireEvent.change(await screen.findByLabelText('Instance name'),{target:{value:'New workspace'}})
    expect(screen.getByRole('button',{name:'Create instance'})).toBeDisabled()
    await screen.findByRole('heading', { name: 'Private workspace A' })
    fireEvent.click(screen.getByText('Create a new instance'))
    await screen.findByRole('option',{name:/Approved release/})
    fireEvent.change(screen.getByLabelText('Initial version'),{target:{value:release.id}})
    fireEvent.click(screen.getByRole('button',{name:'Create instance'}))
    await waitFor(()=>expect(mocks.createInstance).toHaveBeenCalledWith('a','New workspace','compose',release.id))
    await waitFor(()=>expect(mocks.action).toHaveBeenCalledWith(instance,'provision'))
  })

  it('discards initial-release choices when the organisation or catalog changes', async () => {
    const release={id:'a'.repeat(64),name:'Approved release'}
    mocks.organisations.mockResolvedValue({platform_admin:false,release_selection_enabled:true,items:[a,b]})
    mocks.provisioningReleases.mockImplementation((org:string)=>org==='a'?Promise.resolve([release]):Promise.reject(new Error('Catalog unavailable')))
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('option',{name:/Approved release/})
    fireEvent.change(screen.getByLabelText('Initial version'),{target:{value:release.id}})
    fireEvent.change(screen.getByLabelText('Organisation'),{target:{value:'b'}})
    await screen.findByText(/Catalog unavailable/)
    expect(screen.getByLabelText('Initial version')).toHaveValue('')
    expect(screen.getByRole('button',{name:'Create instance'})).toBeDisabled()
    expect(mocks.createInstance).not.toHaveBeenCalled()
  })

  it('keeps older servers compatible without exposing a nonfunctional selector', async () => {
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Private workspace A' })
    fireEvent.click(screen.getByText('Create a new instance'))
    await screen.findByLabelText('Instance name')
    expect(screen.queryByLabelText('Initial version')).not.toBeInTheDocument()
    expect(mocks.provisioningReleases).not.toHaveBeenCalled()
  })

  it('requires explicit acknowledgement of stopped enrollment and sends the matching action', async () => {
    mocks.instances.mockResolvedValue([{...instance,state:'stopped',available_actions:['enroll_execution']}])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button',{name:'Enroll in central limits'}))
    expect(screen.getByRole('button',{name:'Confirm operation'})).toBeDisabled()
    expect(screen.getByText(/A fresh encrypted backup/)).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText(/I authorize this enrollment review/))
    fireEvent.click(screen.getByRole('button',{name:'Confirm operation'}))
    await waitFor(()=>expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({id:instance.id}),'enroll_execution',{acknowledge_execution_enrollment:true}))
  })

  it('keeps enrollment and its interrupted recovery restricted to administrators', async () => {
    mocks.organisations.mockResolvedValue({platform_admin:false,items:[{...a,role:'member'}]})
    mocks.instances.mockResolvedValue([{...instance,state:'stopped',available_actions:['enroll_execution']}])
    mocks.request.mockResolvedValue([{id:'op',action:'enroll_execution',state:'reconciling'}])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading',{name:instance.name})
    expect(screen.queryByRole('button',{name:'Enroll in central limits'})).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button',{name:'Operations'}))
    await screen.findByRole('cell',{name:'reconciling'})
    expect(screen.queryByRole('button',{name:'Review interrupted operation'})).not.toBeInTheDocument()
  })

  it('uses one explicit enrollment recovery mode rather than ordinary stopping recovery', async () => {
    mocks.instances.mockResolvedValue([{...instance,state:'stopped',available_actions:[]}])
    mocks.request.mockResolvedValue([{id:'op',action:'enroll_execution',state:'reconciling'}])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button',{name:'Operations'}))
    fireEvent.click(await screen.findByRole('button',{name:'Review interrupted operation'}))
    expect(screen.getByRole('button',{name:'Continue enrollment review'})).toBeDisabled()
    fireEvent.click(screen.getByLabelText(/I authorize this enrollment review/))
    fireEvent.click(screen.getByRole('button',{name:'Continue enrollment review'}))
    await waitFor(()=>expect(mocks.request).toHaveBeenCalledWith('/organisations/a/instances/instance-a/operations/op/recover',expect.anything(),{mode:'continue_enrollment'}))
  })

  it('shows that an enrolled instance awaits platform activation', async () => {
    mocks.instances.mockResolvedValue([{...instance,state:'stopped',execution_required:true,available_actions:[]}])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    expect(await screen.findByText(/Enrolled; waiting for platform activation/)).toBeInTheDocument()
    expect(screen.queryByRole('button',{name:'Start'})).not.toBeInTheDocument()
  })

  it('shows only available backend operations and retains the operation history', async () => {
    mocks.request.mockResolvedValue([{ id: 'operation-a', action: 'provision', state: 'reconciling', created_at: '2026-01-01T00:00:00Z' }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Private workspace A' })
    expect(screen.getByRole('button', { name: 'Provision' })).toBeEnabled()
    expect(screen.queryByRole('button', { name: 'Back up' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Operations' }))
    await screen.findByRole('cell', { name: 'reconciling' })
    expect(mocks.instances).toHaveBeenCalledTimes(1)
  })

  it('requires explicitly assigning content-reader permission to an administrator', async () => {
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Private workspace A' })
    fireEvent.click(screen.getByRole('button', { name: 'Members' }))
    fireEvent.change(screen.getByLabelText('E-Mail'), { target: { value: 'support@example.test' } })
    fireEvent.change(screen.getByLabelText('Role'), { target: { value: 'admin' } })
    const permission = screen.getByLabelText(/Permit administrative reading/)
    expect(permission).not.toBeChecked()
    fireEvent.click(permission)
    fireEvent.click(screen.getByRole('button', { name: 'Assign membership' }))
    await waitFor(() => expect(mocks.request).toHaveBeenCalledWith('/organisations/a/members', expect.anything(), { email: 'support@example.test', role: 'admin', content_admin: true }))
    fireEvent.change(screen.getByLabelText('Role'), { target: { value: 'member' } })
    expect(screen.queryByLabelText(/Permit administrative reading/)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Assign membership' }))
    await waitFor(() => expect(mocks.request).toHaveBeenCalledWith('/organisations/a/members', expect.anything(), { email: 'support@example.test', role: 'member', content_admin: false }))
  })

  it('discards an old organisation response after the organisation changes', async () => {
    let complete!: (value: unknown) => void
    mocks.instances.mockImplementation((org: string) => org === 'a' ? new Promise(resolve => { complete = resolve }) : Promise.resolve([{ ...instance, id: 'instance-b', name: 'Workspace B', organisation_id: 'b' }]))
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await waitFor(() => expect(mocks.instances).toHaveBeenCalledWith('a', expect.any(AbortSignal)))
    fireEvent.change(screen.getByLabelText('Organisation'), { target: { value: 'b' } })
    await screen.findByRole('heading', { name: 'Workspace B' })
    await act(async () => { complete([instance]) })
    expect(screen.queryByRole('heading', { name: 'Private workspace A' })).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Workspace B' })).toBeInTheDocument()
  })

  it('keeps auditor access read-only', async () => {
    mocks.organisations.mockResolvedValue({ platform_admin: false, items: [{ ...a, role: 'auditor' }] })
    mocks.instances.mockResolvedValue([{ ...instance, can_manage: false }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Private workspace A' })
    expect(screen.getByRole('button', { name: 'Provision' })).toBeDisabled()
    expect(screen.queryByRole('button', { name: 'Create instance' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Policies' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Audit' })).toBeEnabled()
  })

  it('selects the organisation named by a completed invitation', async () => {
    render(<MemoryRouter initialEntries={['/management?organisation=b']}><EnterpriseManagement /></MemoryRouter>)
    await waitFor(() => expect(mocks.instances).toHaveBeenCalledWith('b', expect.any(AbortSignal)))
    expect(screen.getByLabelText('Organisation')).toHaveValue('b')
  })

  it('requires selecting a verified backup belonging to this instance', async () => {
    mocks.instances.mockResolvedValue([{ ...instance, state: 'stopped', available_actions: ['restore'] }])
    mocks.backups.mockResolvedValue([
      { id: 'backup-a', instance_id: 'instance-a', organisation_id: 'a', created_at: '2026-01-01T00:00:00Z', size_bytes: 1024 },
      { id: 'backup-b', instance_id: 'instance-b', organisation_id: 'b', created_at: '2026-01-01T00:00:00Z', size_bytes: 1024 },
    ])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Restore' }))
    const select = await screen.findByLabelText('Backup')
    await waitFor(() => expect(select).toBeEnabled())
    expect(screen.getByRole('button', { name: 'Confirm operation' })).toBeDisabled()
    expect(screen.queryByRole('option', { name: /backup-b/ })).not.toBeInTheDocument()
    fireEvent.change(select, { target: { value: 'backup-a' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm operation' }))
    await waitFor(() => expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({ id: 'instance-a' }), 'restore', { backup_id: 'backup-a' }))
  })

  it('does not display a stale backup response after an organisation switch', async () => {
    let complete!: (value: unknown) => void
    mocks.backups.mockImplementation(() => new Promise(resolve => { complete = resolve }))
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Backups' }))
    await waitFor(() => expect(mocks.backups).toHaveBeenCalled())
    fireEvent.change(screen.getByLabelText('Organisation'), { target: { value: 'b' } })
    await act(async () => { complete([{ id: 'private-backup-a', created_at: '2026-01-01', size_bytes: 1 }]) })
    expect(screen.queryByText('private-backup-a')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Instance backups')).not.toBeInTheDocument()
  })

  it('requires a catalog release and defaults to testing without allowing unqualified installation', async () => {
    const source = 'a'.repeat(64), target = 'b'.repeat(64)
    mocks.instances.mockResolvedValue([{ ...instance, state: 'stopped', current_release: source, available_actions: ['upgrade'] }])
    mocks.releases.mockResolvedValue([{ id: target, source_release: source, name: 'Candidate', qualified: false }, { id: 'c'.repeat(64), source_release: 'd'.repeat(64), name: 'Foreign transition', qualified: true }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Upgrade' }))
    expect(screen.getByRole('button', { name: 'Confirm operation' })).toBeDisabled()
    await screen.findByRole('option', { name: /Candidate/ })
    expect(screen.queryByRole('option', { name: /Foreign transition/ })).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Approved release'), { target: { value: target } })
    expect(screen.getByLabelText('Update mode')).toHaveValue('test')
    expect(screen.getByRole('option', { name: 'Install tested version' })).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Update mode'), { target: { value: 'deploy' } })
    expect(screen.getByRole('button', { name: 'Confirm operation' })).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Update mode'), { target: { value: 'test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm operation' }))
    await waitFor(() => expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({ id: instance.id }), 'upgrade', { release: target, qualification_only: true }))
  })

  it('installs a qualified transition only after explicitly choosing installation', async () => {
    const source = 'a'.repeat(64), target = 'b'.repeat(64)
    mocks.instances.mockResolvedValue([{ ...instance, state: 'stopped', current_release: source, available_actions: ['upgrade'] }])
    mocks.releases.mockResolvedValue([{ id: target, source_release: source, name: 'Verified', qualified: true }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Upgrade' }))
    await screen.findByRole('option', { name: /Verified/ })
    fireEvent.change(screen.getByLabelText('Approved release'), { target: { value: target } })
    fireEvent.change(screen.getByLabelText('Update mode'), { target: { value: 'deploy' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm operation' }))
    await waitFor(() => expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({ id: instance.id }), 'upgrade', { release: target, qualification_only: false }))
  })

  it('clears a temporary connection failure after the next successful refresh', async () => {
    mocks.instances.mockRejectedValueOnce(new Error('Temporary connection failure')).mockResolvedValue([instance])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByRole('heading', { name: 'Private workspace A' })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('opens SSO without loading instance or provisioning catalogs', async () => {
    mocks.organisations.mockResolvedValue({ platform_admin: false, release_selection_enabled: true, items: [a, b] })
    mocks.instances.mockRejectedValue(new Error('Unrelated instance database failure'))
    render(<MemoryRouter initialEntries={['/management?organisation=a&view=sso']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('button', { name: 'Save SSO settings' })
    expect(mocks.sso).toHaveBeenCalledWith('a', expect.any(AbortSignal))
    expect(mocks.instances).not.toHaveBeenCalled()
    expect(mocks.provisioningReleases).not.toHaveBeenCalled()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('clears an instance connection error when opening SSO and reloads instances on return', async () => {
    mocks.instances.mockRejectedValueOnce(new Error('Instance database failure')).mockResolvedValue([instance])
    render(<MemoryRouter initialEntries={['/management?organisation=a&view=instances']}><EnterpriseManagement /></MemoryRouter>)
    expect(await screen.findByRole('alert')).toHaveTextContent('Connection interrupted. Retrying automatically.')
    fireEvent.click(screen.getByRole('button', { name: 'SSO' }))
    await screen.findByRole('button', { name: 'Save SSO settings' })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(mocks.instances).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Instances' }))
    await screen.findByRole('heading', { name: instance.name })
    expect(mocks.instances).toHaveBeenCalledTimes(2)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('ignores a delayed instance failure after switching to SSO', async () => {
    let rejectInstances!: (reason: Error) => void
    mocks.instances.mockImplementation(() => new Promise((_, reject) => { rejectInstances = reject }))
    render(<MemoryRouter initialEntries={['/management?organisation=a&view=instances']}><EnterpriseManagement /></MemoryRouter>)
    await waitFor(() => expect(mocks.instances).toHaveBeenCalledWith('a', expect.any(AbortSignal)))
    fireEvent.click(screen.getByRole('button', { name: 'SSO' }))
    await screen.findByRole('button', { name: 'Save SSO settings' })
    await act(async () => { rejectInstances(new Error('Old instance request failed')) })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(mocks.instances).toHaveBeenCalledTimes(1)
  })

  it('clears the previous organisation connection error while loading the next organisation', async () => {
    let completeInstances!: (value: unknown) => void
    mocks.instances.mockImplementation((id: string) => id === 'a' ? Promise.reject(new Error('Organisation A database failure')) : new Promise(resolve => { completeInstances = resolve }))
    render(<MemoryRouter initialEntries={['/management?organisation=a&view=instances']}><EnterpriseManagement /></MemoryRouter>)
    expect(await screen.findByRole('alert')).toHaveTextContent('Organisation A database failure')
    fireEvent.change(screen.getByLabelText('Organisation'), { target: { value: 'b' } })
    await waitFor(() => expect(mocks.instances).toHaveBeenCalledWith('b', expect.any(AbortSignal)))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    await act(async () => { completeInstances([{ ...instance, organisation_id: 'b', name: 'Organisation B workspace' }]) })
    await screen.findByRole('heading', { name: 'Organisation B workspace' })
  })

  it('keeps the member directory available when its required instance list fails', async () => {
    mocks.instances.mockRejectedValue(new Error('Instance database failure'))
    mocks.request.mockImplementation(async (path: string) => path === '/organisations/a/members' ? [{ user_id: 'member', display_name: 'Alice', email: 'alice@example.test', role: 'member' }] : [])
    render(<MemoryRouter initialEntries={['/management?organisation=a&view=members']}><EnterpriseManagement /></MemoryRouter>)
    await screen.findByText('Alice')
    expect(await screen.findByRole('alert')).toHaveTextContent('Instance list could not be loaded. Refresh to retry.')
    expect(screen.queryByText(/Retrying automatically/)).not.toBeInTheDocument()
    expect(mocks.instances).toHaveBeenCalledWith('a', expect.any(AbortSignal))
  })
  it('requires an explicit scoped recovery confirmation and hides it from viewers', async () => {
    mocks.request.mockResolvedValue([{ id: 'operation-a', action: 'start', state: 'reconciling' }])
    const rendered = render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Operations' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Review interrupted operation' }))
    expect(screen.getByText(/all running work in this instance is stopped/)).toBeInTheDocument()
    expect(mocks.request.mock.calls.some(call => call[0].endsWith('/recover'))).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Reconcile; stop if unproven' }))
    await waitFor(() => expect(mocks.request).toHaveBeenCalledWith('/organisations/a/instances/instance-a/operations/operation-a/recover', expect.anything(), { mode: 'stop_if_unproven' }))
    rendered.unmount()
    mocks.instances.mockResolvedValue([{ ...instance, can_manage: false }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Operations' }))
    await screen.findByRole('cell', { name: 'reconciling' })
    expect(screen.queryByRole('button', { name: 'Review interrupted operation' })).not.toBeInTheDocument()
  })

  it('requires the exact instance name and acknowledgement of backup deletion', async () => {
    mocks.instances.mockResolvedValue([{ ...instance, state: 'ready', available_actions: ['delete'] }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Delete' }))
    const confirm = screen.getByRole('button', { name: 'Confirm operation' })
    expect(confirm).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Type the instance name'), { target: { value: instance.name } })
    expect(confirm).toBeDisabled()
    fireEvent.click(screen.getByLabelText('Permanently delete all managed backups as well.'))
    fireEvent.change(screen.getByLabelText('Type the instance name'), { target: { value: instance.name + ' ' } })
    expect(confirm).toBeDisabled()
    fireEvent.submit(confirm.closest('form')!)
    expect(mocks.action).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText('Type the instance name'), { target: { value: instance.name } })
    fireEvent.click(confirm)
    await waitFor(() => expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({ id: instance.id }), 'delete', { confirmation_name: instance.name, purge_backups: true }))
  })

  it('requires fresh deletion consent to recover and sends the destructive recovery mode', async () => {
    mocks.instances.mockResolvedValue([{ ...instance, state: 'deleting', available_actions: [] }])
    mocks.request.mockResolvedValue([{ id: 'operation-a', action: 'delete', state: 'reconciling' }])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Operations' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Review interrupted operation' }))
    const confirm = screen.getByRole('button', { name: 'Continue permanent deletion' })
    expect(confirm).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Type the instance name'), { target: { value: instance.name } })
    fireEvent.click(screen.getByLabelText('Permanently delete all managed backups as well.'))
    fireEvent.click(confirm)
    await waitFor(() => expect(mocks.request).toHaveBeenCalledWith('/organisations/a/instances/instance-a/operations/operation-a/recover', expect.anything(), { mode: 'continue_deletion', confirmation_name: instance.name, purge_backups: true }))
  })

  it('closes history when the instance disappears during deletion polling without showing an access error', async () => {
    mocks.instances.mockResolvedValueOnce([instance]).mockResolvedValue([])
    mocks.request.mockRejectedValue(new Error('organisation or resource is not accessible'))
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Operations' }))
    await waitFor(() => expect(mocks.instances).toHaveBeenCalledTimes(2))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('requires a matching rollback point and explicit data-loss acknowledgement', async () => {
    mocks.instances.mockResolvedValue([{ ...instance, state: 'stopped', current_release: 'b'.repeat(64), available_actions: ['rollback'] }])
    mocks.rollbacks.mockResolvedValue([
      { operation_id: 'promotion-a', backup_id: 'promotion-a', source_release: 'b'.repeat(64), release: 'a'.repeat(64), created_at: '2026-01-01T00:00:00Z' },
      { operation_id: 'stale', backup_id: 'stale', source_release: 'c'.repeat(64), release: 'd'.repeat(64), created_at: '2026-01-01T00:00:00Z' },
    ])
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Roll back update' }))
    const select = screen.getByLabelText('Update to undo')
    await waitFor(() => expect(select).toBeEnabled())
    expect(select.querySelector('option[value="stale"]')).toBeNull()
    fireEvent.change(select, { target: { value: 'promotion-a' } })
    expect(screen.getByRole('button', { name: 'Confirm operation' })).toBeDisabled()
    fireEvent.click(screen.getByLabelText('I understand that changes after the pre-update backup will no longer be active.'))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm operation' }))
    await waitFor(() => expect(mocks.action).toHaveBeenCalledWith(expect.objectContaining({ id: 'instance-a' }), 'rollback', { rollback_operation_id: 'promotion-a' }))
  })

  it('does not revive a rollback point after an organisation switch', async () => {
    let complete!: (value: unknown) => void
    mocks.instances.mockResolvedValue([{ ...instance, state: 'stopped', current_release: 'b'.repeat(64), available_actions: ['rollback'] }])
    mocks.rollbacks.mockImplementation(() => new Promise(resolve => { complete = resolve }))
    render(<MemoryRouter initialEntries={['/management?view=instances']}><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Roll back update' }))
    await waitFor(() => expect(mocks.rollbacks).toHaveBeenCalled())
    fireEvent.change(screen.getByLabelText('Organisation'), { target: { value: 'b' } })
    await act(async () => { complete([{ operation_id: 'private', source_release: 'b'.repeat(64), release: 'a'.repeat(64), created_at: '2026-01-01' }]) })
    expect(screen.queryByLabelText('Update to undo')).not.toBeInTheDocument()
    expect(mocks.action).not.toHaveBeenCalled()
  })

})
