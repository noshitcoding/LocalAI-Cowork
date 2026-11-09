import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import i18n from '../i18n'

const mocks = vi.hoisted(() => ({ organisations: vi.fn(), instances: vi.fn(), request: vi.fn() }))
vi.mock('../stores/remoteRuntimeStore', () => ({ useRemoteRuntimeStore: () => ({ status: 'authenticated', serverUrl: 'https://management.example', userId: 'review-user', email: 'review@example.test', logout: vi.fn() }) }))
vi.mock('../runtime/managementClient', () => ({ ManagementClient: class { organisations = mocks.organisations; instances = mocks.instances; request = mocks.request } }))
vi.mock('./RemoteServerView', () => ({ default: ({ managementContent }: { managementContent?: import('react').ReactNode }) => managementContent }))
import EnterpriseManagement from './EnterpriseManagement'

const policy = { revision: 1, extensions: 'catalog', content_access: 'support_grant', minimum_isolation: 'container', allow_laptop_full_access: false, allow_offline_continuation: true, allowed_network_origins: [], limits: { cpu_millis: 2000, memory_mib: 1024, storage_gib: 10, instances: 2, concurrent_runs: 1 }, audit_retention_days: 365, content_logging: false }
const org = { id: '10000000-0000-4000-8000-000000000001', name: 'Review organisation', role: 'admin', policy }
const capabilities = { schema_version: 1, organisation_settings: true, organisation_archive: true, resource_catalog: true, environments: true, environment_access: true, directory: true, organisation_mfa: true }

describe('management redesign compatibility', () => {
  beforeEach(async () => {
    vi.resetAllMocks(); await i18n.changeLanguage('en')
    mocks.organisations.mockResolvedValue({ platform_admin: true, items: [org] })
    mocks.instances.mockResolvedValue([])
    mocks.request.mockImplementation(async path => path === '/capabilities' ? capabilities : [])
  })

  it('starts with the overview and does not expose an instance creation form there', async () => {
    render(<MemoryRouter><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Server inventory' })
    expect(screen.getByRole('heading', { name: 'Server overview' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Create instance' })).not.toBeInTheDocument()
  })

  it('keeps the unsupported catalog read-only on a legacy control server', async () => {
    mocks.request.mockRejectedValue(new Error('Management request failed (404)'))
    render(<MemoryRouter><EnterpriseManagement /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Resource catalog' }))
    expect(await screen.findByRole('heading', { name: 'Central catalog requires a server update' })).toBeInTheDocument()
    expect(mocks.request.mock.calls.some(call => call[0] === '/resource-catalog')).toBe(false)
  })

  it('rechecks capabilities after a transient failure when the operator refreshes', async () => {
    let attempts = 0
    mocks.request.mockImplementation(async path => {
      if (path !== '/capabilities') return []
      attempts++
      if (attempts === 1) throw new Error('Temporary connection failure')
      return capabilities
    })
    render(<MemoryRouter><EnterpriseManagement /></MemoryRouter>)
    await screen.findByRole('heading', { name: 'Server inventory' })
    await waitFor(() => expect(attempts).toBe(1))
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await waitFor(() => expect(attempts).toBe(2))
  })
})
