import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { z } from 'zod'
import type { ManagedInstance, ManagementClient } from '../runtime/managementClient'
import i18n from '../i18n'
import EnterpriseEnvironments from './EnterpriseEnvironments'

vi.mock('./EnterpriseVmSnapshots', () => ({ default: () => null }))
const orgId = '10000000-0000-4000-8000-000000000001'
const instanceId = '10000000-0000-4000-8000-000000000002'
const templateId = '10000000-0000-4000-8000-000000000003'
const environmentId = '10000000-0000-4000-8000-000000000004'
const instance = { id: instanceId, organisation_id: orgId, name: 'Team workspace', state: 'ready', can_manage: true, backend: 'compose', revision: 7, policy: { minimum_isolation: 'virtual_machine', limits: { cpu_millis: 2000, memory_mib: 2048, storage_gib: 10 } } } as ManagedInstance
const template = { id: templateId, revision: 1, name: 'Python tools', backend: 'docker', isolation: 'virtual_machine', image_reference: `example/tools@sha256:${'a'.repeat(64)}`, enabled: true }
const environment = { id: environmentId, revision: 3, state: 'running', spec: { name: 'Research runtime', backend: 'docker', persistent: true, shared_workspace: false, limits: { cpu_millis: 1000, memory_mib: 512, storage_gib: 2 } }, runtime: { container_id: 'container-test' } }
let environments: unknown[] = []
let controllers: unknown[] = []
let templates: unknown[] = []
const request = vi.fn(async (path: string, schema: z.ZodType, body?: unknown) => {
  if (body) {
    if (path.endsWith('/actions')) return schema.parse({ id: environmentId, state: 'completed', plan_sha256: 'b'.repeat(64) })
    return schema.parse({ id: environmentId })
  }
  return schema.parse(path.endsWith('/templates') ? templates : path.endsWith('/environments') ? environments : path.endsWith('/environment-controllers') ? controllers : [])
})
const client = { request } as unknown as ManagementClient

describe('enterprise runtime environments', () => {
  beforeEach(async () => { vi.clearAllMocks(); environments = []; controllers = []; templates = [template]; await i18n.changeLanguage('en') })

  it('keeps template registration and environment names separate', async () => {
    render(<EnterpriseEnvironments client={client} instance={instance} admin onClose={vi.fn()} />)
    await screen.findByRole('option', { name: 'Python tools · Enabled' })
    fireEvent.change(screen.getByLabelText('Template'), { target: { value: `${templateId}:1` } })
    fireEvent.change(screen.getByLabelText('Template name'), { target: { value: 'New base image' } })
    fireEvent.change(screen.getByLabelText('Environment name'), { target: { value: 'Independent runtime' } })
    expect(screen.getByLabelText('Template name')).toHaveValue('New base image')
    expect(screen.getByLabelText('Environment name')).toHaveValue('Independent runtime')
    fireEvent.click(screen.getByRole('button', { name: 'Create environment' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(expect.stringContaining('/environments'), expect.anything(), expect.objectContaining({ name: 'Independent runtime', template_id: templateId, template_revision: 1 })))
  })

  it('shows OpenShell only when enrolled and keeps VM power controls separate', async () => {
    controllers = [{ id: environmentId, backend: 'open_shell', revision: 2, enabled: true, specification: { vm_id: instanceId, limits: { cpu_millis: 1000, memory_mib: 1024, storage_gib: 2 } } }]
    render(<EnterpriseEnvironments client={client} instance={instance} admin onClose={vi.fn()} />)
    await screen.findByRole('option', { name: 'NVIDIA OpenShell' })
    expect(screen.getByText(/OpenShell runs sandboxes inside the enrolled instance VM/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Start original VM' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Stop original VM' })).not.toBeInTheDocument()
    expect(screen.getByText('1 CPU · 1024 MiB RAM · 2 GiB')).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Runtime backend'), { target: { value: 'open_shell' } })
    expect(screen.queryByRole('option', { name: 'Python tools · Enabled' })).not.toBeInTheDocument()
  })

  it('does not advertise managed runtimes for an unsupported ordinary Compose instance', async () => {
    render(<EnterpriseEnvironments client={client} instance={{ ...instance, policy: { ...instance.policy, minimum_isolation: 'container' } }} admin onClose={vi.fn()} />)
    await screen.findByText('No environments yet')
    expect(screen.getByText(/This deployment does not support managed environments/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Create environment' })).not.toBeInTheDocument()
    expect(screen.queryByRole('option', { name: 'NVIDIA OpenShell' })).not.toBeInTheDocument()
  })

  it('keeps environment deletion bound to its exact name and revision', async () => {
    environments = [environment]
    render(<EnterpriseEnvironments client={client} instance={instance} admin onClose={vi.fn()} />)
    await screen.findByText('Research runtime')
    fireEvent.click(screen.getByText('Delete environment'))
    const remove = screen.getByRole('button', { name: 'Delete workspace and environment' })
    expect(remove).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Name to confirm workspace deletion'), { target: { value: 'Research runtime' } })
    fireEvent.click(remove)
    await waitFor(() => expect(request).toHaveBeenCalledWith(expect.stringContaining(`/environments/${environmentId}/actions`), expect.anything(), expect.objectContaining({ action: 'delete', expected_instance_revision: 7, expected_environment_revision: 3, confirmation_name: 'Research runtime' })))
  })

  it('shows allocated resources and the actual runtime identity without invented access links', async () => {
    environments = [environment]
    render(<EnterpriseEnvironments client={client} instance={{ ...instance, can_manage: false }} admin={false} onClose={vi.fn()} />)
    await screen.findByText('Research runtime')
    expect(screen.getByText('512 MiB')).toBeInTheDocument()
    expect(screen.getByText('container-test')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Start runtime' })).not.toBeInTheDocument()
    expect(screen.queryByRole('link')).not.toBeInTheDocument()
  })
})
