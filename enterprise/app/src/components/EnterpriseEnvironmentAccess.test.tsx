import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { z } from 'zod'
import type { ManagedInstance, ManagementClient } from '../runtime/managementClient'
import i18n from '../i18n'
import EnterpriseEnvironmentAccess from './EnterpriseEnvironmentAccess'

const orgId = '10000000-0000-4000-8000-000000000001'
const instanceId = '10000000-0000-4000-8000-000000000002'
const environmentId = '10000000-0000-4000-8000-000000000003'
const otherEnvironmentId = '10000000-0000-4000-8000-000000000004'
const userId = '10000000-0000-4000-8000-000000000005'
const access = { organisation_id: orgId, instance_id: instanceId, environment_id: environmentId, revision: 4, mode: 'inherit_instance', members: [] }
const person = { user_id: userId, display_name: 'Samira Klein', email: 'samira@example.test', role: 'member', organisation_role: 'member' }
const instance = { id: instanceId, organisation_id: orgId, name: 'Team server', can_manage: true } as ManagedInstance
const response = (body: unknown) => { const value = body as { mode: string; members: unknown[] }; return { ...access, mode: value.mode, members: value.members, revision: 5 } }
const request = vi.fn(async (_path: string, schema: z.ZodType, body?: unknown, method?: string) => schema.parse(method === 'PUT' ? response(body) : access))
const instanceMembers = vi.fn(async () => [person])
const client = { request, instanceMembers } as unknown as ManagementClient
const page = (canManage = true, id = environmentId, blocked = false) => <EnterpriseEnvironmentAccess client={client} instance={instance} environmentId={id} environmentName="Research" canManage={canManage} blocked={blocked} />
const open = async () => { fireEvent.click(screen.getByText('Runtime access')); await screen.findByRole('radio', { name: /All authorised instance members/ }) }

describe('runtime access editor', () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    request.mockImplementation(async (_path, schema, body, method) => schema.parse(method === 'PUT' ? response(body) : access))
    instanceMembers.mockResolvedValue([person])
    await i18n.changeLanguage('en')
  })

  it('loads lazily and preserves inherited instance access until explicitly saved', async () => {
    render(page())
    expect(request).not.toHaveBeenCalled()
    await open()
    expect(screen.getByRole('radio', { name: /All authorised instance members/ })).toBeChecked()
    expect(screen.getByText(/Instance membership remains required/)).toBeInTheDocument()
    expect(instanceMembers).toHaveBeenCalledWith(instance, expect.any(AbortSignal))
    expect(request.mock.calls.some(call => call[3] === 'PUT')).toBe(false)
  })

  it('saves only selected instance members and their exact runtime roles with the current revision', async () => {
    render(page())
    await open()
    fireEvent.click(screen.getByRole('radio', { name: /Selected members only/ }))
    expect(screen.getByText(/No selected members means nobody/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('checkbox', { name: /Samira Klein/ }))
    fireEvent.change(screen.getByLabelText('Runtime role for Samira Klein'), { target: { value: 'runner' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save runtime access' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/instances/${instanceId}/environments/${environmentId}/access`, expect.anything(), { expected_revision: 4, mode: 'restricted', members: [{ user_id: userId, role: 'runner' }] }, 'PUT'))
    expect(await screen.findByRole('status')).toHaveTextContent('Runtime access saved.')
  })

  it('keeps rejected changes visible and does not claim a successful save', async () => {
    request.mockImplementation(async (_path, schema, _body, method) => { if (method === 'PUT') throw new Error('Current tasks must finish first'); return schema.parse(access) })
    render(page())
    await open()
    fireEvent.click(screen.getByRole('radio', { name: /Selected members only/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Save runtime access' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Current tasks must finish first')
    expect(screen.getByRole('radio', { name: /Selected members only/ })).toBeChecked()
    expect(screen.queryByText('Runtime access saved.')).not.toBeInTheDocument()
  })

  it('cannot save without administrator authority or during an unsettled operation', async () => {
    const view = render(page(false))
    await open()
    expect(screen.getByRole('radio', { name: /Selected members only/ })).toBeDisabled()
    expect(screen.queryByRole('button', { name: 'Save runtime access' })).not.toBeInTheDocument()
    view.rerender(page(true, environmentId, true))
    const save = screen.getByRole('button', { name: 'Save runtime access' })
    expect(save).toBeDisabled()
    fireEvent.submit(save.closest('form')!)
    expect(request.mock.calls.some(call => call[3] === 'PUT')).toBe(false)
  })

  it('discards an in-flight response after switching environments', async () => {
    let finish!: (value: unknown) => void
    request.mockImplementation(async (path, schema) => path.includes(otherEnvironmentId) ? schema.parse({ ...access, environment_id: otherEnvironmentId }) : new Promise(resolve => { finish = resolve }))
    const view = render(page())
    fireEvent.click(screen.getByText('Runtime access'))
    await waitFor(() => expect(request).toHaveBeenCalled())
    view.rerender(page(true, otherEnvironmentId))
    await screen.findByRole('radio', { name: /All authorised instance members/ })
    await act(async () => finish({ ...access, mode: 'restricted', members: [{ user_id: userId, role: 'runner' }] }))
    expect(screen.getByRole('radio', { name: /All authorised instance members/ })).toBeChecked()
    expect(screen.queryByRole('checkbox', { name: /Samira/ })).not.toBeInTheDocument()
  })

  it('requires explicit removal of grants for people who no longer hold instance membership', async () => {
    request.mockImplementation(async (_path, schema) => schema.parse({ ...access, mode: 'restricted', members: [{ user_id: otherEnvironmentId, role: 'viewer' }] }))
    render(page())
    await open()
    expect(screen.getByText(/No longer an eligible instance member/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Save runtime access' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: 'Remove unavailable assignment' }))
    expect(screen.getByRole('button', { name: 'Save runtime access' })).toBeEnabled()
  })
})
