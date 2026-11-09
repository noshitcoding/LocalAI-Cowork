import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import EnterpriseInstanceImports from './EnterpriseInstanceImports'
import type { ManagementClient, ManagedInstance } from '../runtime/managementClient'
import { readImportJournal } from '../runtime/instanceImportJournal'

const uuid = (n: number) => `${n.toString().padStart(8, '0')}-1111-4111-8111-111111111111`
const scope = { server: 'https://manager.test', user: uuid(1), organisation: uuid(2) }
const source = { id: uuid(3), organisation_id: scope.organisation, owner_user_id: scope.user, name: 'Source workspace', backend: 'compose', state: 'stopped', revision: 2 } as ManagedInstance
const target = { ...source, id: uuid(4), name: 'Fresh target', revision: 3, execution_required: true, current_release: 'a'.repeat(64), can_open: true }
const backup = { id: uuid(5), instance_id: source.id, organisation_id: scope.organisation, created_at: '2026-10-09', sha256: 'b'.repeat(64), size_bytes: 123, operation_id: uuid(8), manifest_version: 1 }
const prepared = { id: uuid(7), phase: 'prepared', revision: 1, operation_id: uuid(8) }
const view = { id: uuid(7), organisation_id: scope.organisation, source_instance_id: source.id, target_instance_id: target.id, backup_id: backup.id, binding_sha256: 'a'.repeat(64), phase: 'qualified', revision: 5 }
const route = { organisation_id: scope.organisation, source_instance_id: source.id, target_instance_id: source.id, revision: 1, import_id: null }
const api = { backups: vi.fn(), prepareInstanceImport: vi.fn(), instanceImport: vi.fn(), instanceRoute: vi.fn(), cutoverInstanceImport: vi.fn(), abortInstanceImport: vi.fn() }
const props = { client: api as unknown as ManagementClient, orgId: scope.organisation, serverUrl: scope.server, userId: scope.user, instances: [source, target], canManage: true, onChanged: vi.fn(), onOpen: vi.fn() }

async function selectPreparation() {
  fireEvent.change(screen.getByLabelText('Import source'), { target: { value: source.id } })
  await screen.findByRole('option', { name: /2026-10-09/ })
  fireEvent.change(screen.getByLabelText('New import target'), { target: { value: target.id } })
  fireEvent.change(screen.getByLabelText('Original source backup'), { target: { value: backup.id } })
  fireEvent.click(screen.getByLabelText(/I authorize preparing/))
}
describe('explicit instance import management', () => {
  beforeEach(async () => {
    vi.resetAllMocks(); localStorage.clear(); await i18n.changeLanguage('en')
    Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_: string, callback: () => unknown) => callback() } })
    api.backups.mockResolvedValue([backup, { ...backup, id: uuid(9), instance_id: target.id }, { ...backup, id: uuid(10), organisation_id: uuid(11) }])
    api.prepareInstanceImport.mockResolvedValue(prepared)
    api.instanceImport.mockResolvedValue(view); api.instanceRoute.mockResolvedValue(route)
  })
  it('requires exact source/target/backup and consent; persists the original body before the first POST', async () => {
    api.prepareInstanceImport.mockImplementation(async (_org, body) => {
      expect(readImportJournal(scope).pending).toEqual({ kind: 'prepare', body })
      return prepared
    })
    render(<EnterpriseInstanceImports {...props} />)
    expect(screen.getByRole('button', { name: 'Prepare import' })).toBeDisabled()
    await selectPreparation()
    expect(screen.queryByRole('option', { name: new RegExp(uuid(9)) })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Prepare import' }))
    await waitFor(() => expect(api.prepareInstanceImport).toHaveBeenCalledTimes(1))
    expect(api.prepareInstanceImport.mock.calls[0][1]).toMatchObject({ source_instance_id: source.id, target_instance_id: target.id, source_revision: 2, target_revision: 3, backup_id: backup.id })
    await screen.findByText('Target checks completed; awaiting switch')
    expect(api.cutoverInstanceImport).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Switch to checked target' })).toBeDisabled()
  })
  it('keeps lost acknowledgements across reload without automatic POST and retries exactly the retained key/body', async () => {
    api.prepareInstanceImport.mockRejectedValueOnce(new Error('response lost'))
    const first = render(<EnterpriseInstanceImports {...props} />)
    await selectPreparation(); fireEvent.click(screen.getByRole('button', { name: 'Prepare import' }))
    await screen.findByText('response lost')
    const original = api.prepareInstanceImport.mock.calls[0][1]
    first.unmount()
    render(<EnterpriseInstanceImports {...props} />)
    await screen.findByText('Unconfirmed import action')
    expect(api.prepareInstanceImport).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Retry retained action' }))
    await waitFor(() => expect(api.prepareInstanceImport).toHaveBeenCalledTimes(2))
    expect(api.prepareInstanceImport.mock.calls[1][1]).toEqual(original)
    await waitFor(() => expect(readImportJournal(scope).pending).toBeNull())
    expect(readImportJournal(scope).accepted).toHaveLength(1)
  })
  it('switches only after a fresh qualified status, exact route and separate explicit consent', async () => {
    api.cutoverInstanceImport.mockImplementation(async (_org, importId, body) => ({ id: importId, source_instance_id: source.id, target_instance_id: body.target_instance_id, idempotency_key: body.idempotency_key, request_digest: 'c'.repeat(64), revision: body.expected_revision + 1, route_revision: body.expected_route_revision + 1, target_state: 'stopped', schedules_paused: true }))
    render(<EnterpriseInstanceImports {...props} />)
    fireEvent.change(screen.getByLabelText('Import ID'), { target: { value: view.id } })
    fireEvent.click(screen.getByRole('button', { name: 'Check import status' }))
    await screen.findByText('Target checks completed; awaiting switch')
    fireEvent.click(screen.getByLabelText('I confirm switching to this target.'))
    fireEvent.click(screen.getByRole('button', { name: 'Switch to checked target' }))
    await waitFor(() => expect(api.cutoverInstanceImport).toHaveBeenCalledTimes(1))
    expect(api.cutoverInstanceImport.mock.calls[0]).toEqual([scope.organisation, view.id, expect.objectContaining({ expected_revision: 5, expected_route_revision: 1, target_instance_id: target.id })])
    expect(props.onOpen).not.toHaveBeenCalled()
    expect(api.abortInstanceImport).not.toHaveBeenCalled()
  })
  it('hides write controls from auditors and rejects another organisation status', async () => {
    api.instanceImport.mockResolvedValue({ ...view, organisation_id: uuid(12) })
    render(<EnterpriseInstanceImports {...props} canManage={false} />)
    expect(screen.queryByRole('button', { name: 'Prepare import' })).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Import ID'), { target: { value: view.id } })
    fireEvent.click(screen.getByRole('button', { name: 'Check import status' }))
    await screen.findByText('Import response scope changed')
    expect(api.instanceRoute).not.toHaveBeenCalled()
    expect(api.prepareInstanceImport).not.toHaveBeenCalled()
  })
  it('discards a late source backup response after the organisation changes', async () => {
    let finish!: (values: unknown[]) => void
    api.backups.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const rendered = render(<EnterpriseInstanceImports {...props} />)
    fireEvent.change(screen.getByLabelText('Import source'), { target: { value: source.id } })
    await waitFor(() => expect(api.backups).toHaveBeenCalled())
    rendered.rerender(<EnterpriseInstanceImports {...props} orgId={uuid(13)} instances={[]} />)
    await act(async () => { finish([backup]) })
    expect(screen.queryByRole('option', { name: /2026-10-09/ })).not.toBeInTheDocument()
    expect(api.prepareInstanceImport).not.toHaveBeenCalled()
  })
  it('keeps the original action when a received cutover receipt conflicts with its target or key', async () => {
    api.cutoverInstanceImport.mockResolvedValue({ id: view.id, source_instance_id: source.id, target_instance_id: uuid(15), idempotency_key: uuid(16), request_digest: 'c'.repeat(64), revision: 6, route_revision: 2, target_state: 'stopped', schedules_paused: true })
    render(<EnterpriseInstanceImports {...props} />)
    fireEvent.change(screen.getByLabelText('Import ID'), { target: { value: view.id } })
    fireEvent.click(screen.getByRole('button', { name: 'Check import status' }))
    await screen.findByText('Target checks completed; awaiting switch')
    fireEvent.click(screen.getByLabelText('I confirm switching to this target.'))
    fireEvent.click(screen.getByRole('button', { name: 'Switch to checked target' }))
    await screen.findByText('Import receipt binding changed')
    expect(readImportJournal(scope).pending?.kind).toBe('cutover')
    expect(readImportJournal(scope).accepted).toHaveLength(0)
    expect(props.onOpen).not.toHaveBeenCalled()
  })
  it('cancels only the reviewed early phase with its exact revision and a separate confirmation', async () => {
    api.instanceImport.mockResolvedValue({ ...view, phase: 'export_verified', revision: 3 })
    api.abortInstanceImport.mockImplementation(async (_org, importId, body) => ({ id: importId, phase: 'aborted', revision: body.expected_revision + 1, request_digest: 'c'.repeat(64), idempotency_key: body.idempotency_key, source_instance_id: source.id, target_instance_id: target.id, source_state: 'stopped', target_state: 'stopped', charges_retained: true }))
    render(<EnterpriseInstanceImports {...props} />)
    fireEvent.change(screen.getByLabelText('Import ID'), { target: { value: view.id } })
    fireEvent.click(screen.getByRole('button', { name: 'Check import status' }))
    await screen.findByText('Original backup verified')
    expect(screen.getByRole('button', { name: 'Cancel import before target work' })).toBeDisabled()
    expect(screen.queryByRole('button', { name: 'Switch to checked target' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('I confirm canceling before target work.'))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel import before target work' }))
    await waitFor(() => expect(api.abortInstanceImport).toHaveBeenCalledWith(scope.organisation, view.id, expect.objectContaining({ expected_revision: 3 })))
    await waitFor(() => expect(readImportJournal(scope).accepted).toHaveLength(1))
  })
  it('blocks every POST when the retained journal cannot be persisted', async () => {
    render(<EnterpriseInstanceImports {...props} />)
    await selectPreparation()
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('disk full') })
    fireEvent.click(screen.getByRole('button', { name: 'Prepare import' }))
    await screen.findByText('disk full')
    expect(api.prepareInstanceImport).not.toHaveBeenCalled()
    write.mockRestore()
  })
})
