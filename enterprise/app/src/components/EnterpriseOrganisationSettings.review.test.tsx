import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ManagementClient, Organisation } from '../runtime/managementClient'
import i18n from '../i18n'
import EnterpriseOrganisationSettings from './EnterpriseOrganisationSettings'

const first = { id: '10000000-0000-4000-8000-000000000001', name: 'First organisation' } as Organisation
const second = { id: '10000000-0000-4000-8000-000000000002', name: 'Second organisation' } as Organisation
const settings = (org: Organisation) => ({ id: org.id, name: org.name, revision: 3, archived_at: null, archived_by: null })

describe('organisation settings review regressions', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })

  it('keeps archive confirmation tied to the exact current name and revision', async () => {
    const request = vi.fn(async (_path, _schema, _body, method) => method === 'POST' ? { ...settings(first), revision: 4, archived_at: '2026-10-17T06:00:00Z' } : settings(first))
    render(<EnterpriseOrganisationSettings client={{ request } as unknown as ManagementClient} org={first} enabled canArchive canManage onChanged={vi.fn()} />)
    await screen.findByDisplayValue(first.name)
    fireEvent.click(screen.getByText('Archive organisation'))
    const archive = screen.getByRole('button', { name: 'Archive this organisation' })
    expect(archive).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Type the organisation name to confirm'), { target: { value: first.name } })
    fireEvent.click(archive)
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${first.id}/archive`, expect.anything(), { confirmation_name: first.name, expected_revision: 3 }, 'POST'))
  })

  it('does not clear a different selected organisation after an old archive finishes', async () => {
    let finish!: (value: unknown) => void
    const request = vi.fn(async (path, _schema, _body, method) => method === 'POST' ? new Promise(resolve => { finish = resolve }) : settings(path.includes(second.id) ? second : first))
    const client = { request } as unknown as ManagementClient
    const changed = vi.fn()
    const view = render(<EnterpriseOrganisationSettings key={first.id} client={client} org={first} enabled canArchive canManage onChanged={changed} />)
    await screen.findByDisplayValue(first.name)
    fireEvent.click(screen.getByText('Archive organisation'))
    fireEvent.change(screen.getByLabelText('Type the organisation name to confirm'), { target: { value: first.name } })
    fireEvent.click(screen.getByRole('button', { name: 'Archive this organisation' }))
    await waitFor(() => expect(request.mock.calls.some(call => call[3] === 'POST')).toBe(true))
    view.rerender(<EnterpriseOrganisationSettings key={second.id} client={client} org={second} enabled canArchive canManage onChanged={changed} />)
    await screen.findByDisplayValue(second.name)
    await act(async () => finish({ ...settings(first), revision: 4, archived_at: '2026-10-17T06:00:00Z' }))
    expect(changed).not.toHaveBeenCalled()
  })
})
