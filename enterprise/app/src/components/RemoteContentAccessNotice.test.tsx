import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RuntimeHttpError, type RemoteRuntimeClient } from '../runtime/runtimeClient'
import type { InstanceContentAccess } from '../runtime/enterpriseContracts'
import i18n from '../i18n'
import RemoteContentAccessNotice from './RemoteContentAccessNotice'

const info = { managed: true, read_only: true, content_access: 'support_grant', automatic_content_access: false } as InstanceContentAccess
const make = (getContentAccess: ReturnType<typeof vi.fn>) => ({ getContentAccess }) as unknown as RemoteRuntimeClient
describe('administrative content rule', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })
  afterEach(() => vi.useRealTimers())
  it('shows the effective approval rule and identifies support-only sessions', async () => {
    const onReadOnly = vi.fn()
    render(<RemoteContentAccessNotice client={make(vi.fn().mockResolvedValue(info))} onReadOnly={onReadOnly} />)
    expect(await screen.findByText(/at most 24 hours/)).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('read only')
    expect(onReadOnly).toHaveBeenCalledWith(true)
  })
  it('does not claim an approval is needed when explicit policy access is active', async () => {
    render(<RemoteContentAccessNotice client={make(vi.fn().mockResolvedValue({ ...info, content_access: 'organisation_admin', automatic_content_access: true }))} onReadOnly={vi.fn()} />)
    expect(await screen.findByText(/explicitly authorised/)).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('permission is active')
    expect(screen.queryByText(/Without an active approval/)).not.toBeInTheDocument()
  })
  it('distinguishes an ordinary reader from administrative support access', async () => {
    render(<RemoteContentAccessNotice client={make(vi.fn().mockResolvedValue({ ...info, instance_viewer: true }))} onReadOnly={vi.fn()} />)
    expect(await screen.findByRole('status')).toHaveTextContent('Project sharing still determines')
    expect(screen.queryByText(/Support access:/)).not.toBeInTheDocument()
  })
  it('drops late responses after changing servers and handles legacy 404', async () => {
    let resolve!: (value: InstanceContentAccess) => void
    const onReadOnly = vi.fn()
    const first = make(vi.fn().mockReturnValue(new Promise(done => { resolve = done })))
    const second = make(vi.fn().mockRejectedValue(new RuntimeHttpError(404, 'Route missing')))
    const { rerender } = render(<RemoteContentAccessNotice client={first} onReadOnly={onReadOnly} />)
    rerender(<RemoteContentAccessNotice client={second} onReadOnly={onReadOnly} />)
    await act(async () => { resolve(info) })
    expect(onReadOnly).not.toHaveBeenCalledWith(true)
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })
  it('reports unavailable rules and clears only that warning after recovery', async () => {
    vi.useFakeTimers()
    const load = vi.fn().mockRejectedValueOnce(new Error('Offline')).mockResolvedValue(info)
    render(<RemoteContentAccessNotice client={make(load)} onReadOnly={vi.fn()} />)
    await act(async () => { await Promise.resolve() })
    expect(screen.getByRole('status')).toHaveTextContent('could not be verified')
    await act(async () => { await vi.advanceTimersByTimeAsync(30000) })
    expect(screen.queryByText(/could not be verified/)).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('read only')
  })
})
