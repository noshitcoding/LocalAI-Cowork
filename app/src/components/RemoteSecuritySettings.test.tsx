import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteSecuritySettings from './RemoteSecuritySettings'

vi.mock('../runtime/oidc', () => ({ oidcEnabled: vi.fn(async () => false) }))

describe('RemoteSecuritySettings sessions', () => {
  it('lists account sessions and revokes another device', async () => {
    const current = {
      schema_version: 2,
      id: '10000000-0000-4000-8000-000000000001',
      device_id: '20000000-0000-4000-8000-000000000001',
      current: true,
      active: true,
      created_at: '2026-08-10T10:00:00Z',
      last_used_at: '2026-08-10T12:00:00Z',
      expires_at: '2026-09-09T10:00:00Z',
      revoked_at: null,
      revoke_reason: null,
    }
    const other = {
      ...current,
      id: '10000000-0000-4000-8000-000000000002',
      device_id: '20000000-0000-4000-8000-000000000002',
      current: false,
    }
    const listAuthSessions = vi.fn()
      .mockResolvedValueOnce([current, other])
      .mockResolvedValueOnce([current, {
        ...other,
        active: false,
        revoked_at: '2026-08-10T12:01:00Z',
        revoke_reason: 'user_device_revoked',
      }])
    const client = {
      totpStatus: vi.fn(async () => ({ schema_version: 2, enabled: false, unused_recovery_codes: 0 })),
      listPasskeys: vi.fn(async () => []),
      listAuthSessions,
      revokeAuthSession: vi.fn(async () => undefined),
      passkeysAvailableInContext: vi.fn(() => false),
    } as unknown as RemoteRuntimeClient

    render(<RemoteSecuritySettings client={client} />)
    fireEvent.click(screen.getByRole('button', { name: /security/i }))

    await screen.findByText('Signed-in devices')
    expect(screen.getByText('This session')).toBeInTheDocument()
    const revoke = screen.getByRole('button', { name: 'Revoke device 20000000' })
    fireEvent.click(revoke)

    await waitFor(() => expect(client.revokeAuthSession).toHaveBeenCalledWith(other.id))
    await waitFor(() => expect(listAuthSessions).toHaveBeenCalledTimes(2))
    expect(screen.queryByRole('button', { name: 'Revoke device 20000000' })).not.toBeInTheDocument()
  })
})
