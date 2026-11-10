import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ManagementClient } from '../runtime/managementClient'
import type { CatalogResource } from '../runtime/resourceCatalog'
import i18n from '../i18n'
import EnterpriseResourceCatalog from './EnterpriseResourceCatalog'

const endpointId = '10000000-0000-4000-8000-000000000001'
const modelId = '10000000-0000-4000-8000-000000000002'
const roleId = '10000000-0000-4000-8000-000000000003'
const missingId = '10000000-0000-4000-8000-000000000004'
const resource = (overrides: Partial<CatalogResource> = {}): CatalogResource => ({ id: modelId, kind: 'model', name: 'Project model', description: '', configuration: { model_id: 'example-model', endpoint_resource_id: endpointId }, secret_reference: null, enabled: true, revision: 7, created_by: roleId, updated_by: roleId, created_at: '2026-10-17T00:00:00Z', updated_at: '2026-10-17T00:00:00Z', deleted_at: null, available: true, ...overrides })
const endpoint = resource({ id: endpointId, kind: 'endpoint', name: 'Retired endpoint', enabled: false, available: false, configuration: { base_url: 'https://models.example/v1', protocol: 'openai_compatible' } })

function setup(items: CatalogResource[]) {
  const request = vi.fn(async (_path, _schema, body, method) => method === 'PUT' ? { ...items.find(item => item.id === modelId), ...body } : method === 'DELETE' ? { id: modelId, deleted: true, revoked_assignments: 0 } : { items })
  render(<EnterpriseResourceCatalog client={{ request } as unknown as ManagementClient} enabled />)
  return request
}
async function edit(name: string) {
  const title = await screen.findByRole('heading', { name })
  fireEvent.click(within(title.closest('article')!).getByRole('button', { name: 'Edit' }))
  return screen.getByRole('region', { name: 'Resource editor' })
}

describe('resource catalog reference regressions', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })

  it('shows a disabled linked endpoint and allows removing it without losing the revision', async () => {
    const request = setup([endpoint, resource()])
    const editor = await edit('Project model')
    const select = within(editor).getByRole('combobox', { name: 'Endpoint' })
    expect(select).toHaveValue(endpointId)
    expect(within(select).getByRole('option', { name: 'Retired endpoint · unavailable' })).toBeInTheDocument()
    fireEvent.change(select, { target: { value: '' } })
    fireEvent.click(within(editor).getByRole('button', { name: 'Save changes' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/resource-catalog/${modelId}`, expect.anything(), expect.objectContaining({ expected_revision: 7, configuration: { model_id: 'example-model', endpoint_resource_id: null, pricing: null } }), 'PUT'))
  })

  it('represents a missing endpoint explicitly instead of displaying an empty selection', async () => {
    setup([resource({ configuration: { model_id: 'example-model', endpoint_resource_id: missingId } })])
    const editor = await edit('Project model')
    const select = within(editor).getByRole('combobox', { name: 'Endpoint' })
    expect(select).toHaveValue(missingId)
    expect(within(select).getByRole('option', { name: /Linked endpoint unavailable/ })).toBeInTheDocument()
  })

  it('lets administrators remove disabled and missing agent-role dependencies', async () => {
    const request = setup([endpoint, resource({ id: roleId, kind: 'agent_role', name: 'Research role', configuration: { system_prompt: 'Research carefully', resource_ids: [endpointId, missingId] } })])
    const editor = await edit('Research role')
    const disabled = within(editor).getByRole('checkbox', { name: 'Retired endpoint · Endpoint · unavailable' })
    const missing = within(editor).getByRole('checkbox', { name: `Unavailable resource · ${missingId}` })
    expect(disabled).toBeChecked(); expect(missing).toBeChecked()
    fireEvent.click(disabled); fireEvent.click(missing)
    fireEvent.click(within(editor).getByRole('button', { name: 'Save changes' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/resource-catalog/${roleId}`, expect.anything(), expect.objectContaining({ expected_revision: 7, configuration: { system_prompt: 'Research carefully', resource_ids: [] } }), 'PUT'))
  })

  it('requires the exact resource name before retiring its current revision', async () => {
    const request = setup([resource()])
    fireEvent.click(await screen.findByRole('button', { name: 'Retire' }))
    const retire = screen.getByRole('button', { name: 'Retire this resource' })
    expect(retire).toBeDisabled()
    fireEvent.change(screen.getByRole('textbox', { name: 'Type the resource name' }), { target: { value: 'project model' } })
    expect(retire).toBeDisabled()
    fireEvent.change(screen.getByRole('textbox', { name: 'Type the resource name' }), { target: { value: 'Project model' } })
    fireEvent.click(retire)
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/resource-catalog/${modelId}`, expect.anything(), { expected_revision: 7 }, 'DELETE'))
  })

  it('writes exact six-place tariffs with their currency and token bounds, and explicitly clears disabled pricing', async () => {
    let stored = resource({ configuration: { model_id: 'example-model', endpoint_resource_id: endpointId, context_window: 32768, max_output_tokens: 4096, pricing: { currency: 'EUR', input_micros_per_million: 1000000, output_micros_per_million: 2000000 } } })
    const request = vi.fn(async (_path, _schema, body, method) => {
      if (method === 'PUT') { stored = { ...stored, ...body, revision: stored.revision + 1 }; return stored }
      return { items: [endpoint, stored] }
    })
    render(<EnterpriseResourceCatalog client={{ request } as unknown as ManagementClient} enabled />)
    const editor = await edit('Project model')
    expect(within(editor).getByRole('checkbox', { name: 'Set a tariff for this model' })).toBeChecked()
    fireEvent.change(within(editor).getByRole('combobox', { name: 'Currency' }), { target: { value: 'USD' } })
    fireEvent.change(within(editor).getByRole('textbox', { name: 'Input price per million tokens' }), { target: { value: '0.123456' } })
    fireEvent.change(within(editor).getByRole('textbox', { name: 'Output price per million tokens' }), { target: { value: '12,000001' } })
    fireEvent.change(within(editor).getByRole('spinbutton', { name: 'Context window (tokens)' }), { target: { value: '65536' } })
    fireEvent.change(within(editor).getByRole('spinbutton', { name: 'Maximum output tokens' }), { target: { value: '8192' } })
    fireEvent.click(within(editor).getByRole('button', { name: 'Save changes' }))
    const configured = { model_id: 'example-model', endpoint_resource_id: endpointId, context_window: 65536, max_output_tokens: 8192, pricing: { currency: 'USD', input_micros_per_million: 123456, output_micros_per_million: 12000001 } }
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/resource-catalog/${modelId}`, expect.anything(), expect.objectContaining({ expected_revision: 7, configuration: configured }), 'PUT'))
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Resource editor' })).not.toBeInTheDocument())
    const reopened = await edit('Project model')
    expect(within(reopened).getByRole('textbox', { name: 'Input price per million tokens' })).toHaveValue('0.123456')
    expect(within(reopened).getByRole('textbox', { name: 'Output price per million tokens' })).toHaveValue('12.000001')
    expect(within(reopened).getByRole('combobox', { name: 'Currency' })).toHaveValue('USD')
    fireEvent.click(within(reopened).getByRole('checkbox', { name: 'Set a tariff for this model' }))
    fireEvent.click(within(reopened).getByRole('button', { name: 'Save changes' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/resource-catalog/${modelId}`, expect.anything(), expect.objectContaining({ expected_revision: 8, configuration: { ...configured, pricing: null } }), 'PUT'))
  })
})
