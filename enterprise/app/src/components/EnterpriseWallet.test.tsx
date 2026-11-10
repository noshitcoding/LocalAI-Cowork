import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { z } from 'zod'
import type { ManagementClient } from '../runtime/managementClient'
import type { MemberWallet, WalletDetail } from '../runtime/wallet'
import i18n from '../i18n'
import EnterpriseWallet from './EnterpriseWallet'

const orgId = '10000000-0000-4000-8000-000000000001'
const otherOrgId = '10000000-0000-4000-8000-000000000002'
const userId = '10000000-0000-4000-8000-000000000003'
const secondUserId = '10000000-0000-4000-8000-000000000004'
const reservationId = '10000000-0000-4000-8000-000000000005'
const wallet = (overrides: Partial<MemberWallet> = {}): MemberWallet => ({ organisation_id: orgId, user_id: userId, email: 'alex@example.test', display_name: 'Alex', currency: 'EUR', revision: 7, enabled: true, balance_micros: 10000000, reserved_micros: 1000000, available_micros: 9000000, monthly_limit_micros: 20000000, monthly_spent_micros: 3000000, monthly_reserved_micros: 1000000, period_start: '2026-10-01T00:00:00Z', enforcement: { state: 'enforced', coverage: 'server_managed_models', automatic_charging: true }, ...overrides })
const detail = (overrides: Partial<WalletDetail> = {}): WalletDetail => ({ ...wallet(), entries: [], reservations: [], has_more: false, ...overrides })
const held = { id: reservationId, state: 'held' as const, maximum_micros: 1000000, charged_micros: null, created_at: '2026-10-17T06:00:00Z' }
const list = (organisation = orgId) => ({ organisation_id: organisation, currency: 'EUR', settings_revision: 1, currency_locked: true, coverage: 'managed_model_proxy', wallets: [wallet({ organisation_id: organisation }), wallet({ organisation_id: organisation, user_id: secondUserId, display_name: 'Samira', email: 'samira@example.test' })] })
function setup(canManage = true, handler?: (path: string, body: unknown, method: string) => unknown | Promise<unknown>) {
  const request = vi.fn(async (path: string, schema: z.ZodType, body?: unknown, method = body === undefined ? 'GET' : 'POST') => schema.parse(await (handler ? handler(path, body, method) : path.endsWith('/wallets') ? list() : detail())))
  const client = { request } as unknown as ManagementClient
  const view = render(<EnterpriseWallet client={client} orgId={orgId} canManage={canManage} enabled />)
  return { request, client, view }
}

describe('enforced member wallets', () => {
  beforeEach(async () => { await i18n.changeLanguage('en') })

  it('does not request wallets from a server without the capability', () => {
    const request = vi.fn()
    render(<EnterpriseWallet client={{ request } as unknown as ManagementClient} orgId={orgId} canManage />)
    expect(screen.getByRole('heading', { name: 'Member wallets require a server update' })).toBeInTheDocument()
    expect(request).not.toHaveBeenCalled()
  })

  it('opens the requested member only when that wallet is visible', async () => {
    const request = vi.fn(async (path: string, schema: z.ZodType) => schema.parse(path.endsWith('/wallets') ? list() : detail({ user_id: secondUserId, display_name: 'Samira' })))
    render(<EnterpriseWallet client={{ request } as unknown as ManagementClient} orgId={orgId} initialUserId={secondUserId} canManage enabled />)
    await screen.findByText('Wallet limits active')
    expect(screen.getByRole('combobox', { name: 'Member wallet' })).toHaveValue(secondUserId)
    expect(request.mock.calls.some(call => call[0] === `/organisations/${orgId}/members/${userId}/wallet`)).toBe(false)
  })

  it('displays and books a historical USD wallet in its own currency without FX', async () => {
    const usd = detail({ currency: 'USD', reservations: [held], entries: [{ id: reservationId, request_id: reservationId, kind: 'usage_charge', amount_micros: 123456, balance_after_micros: 10000000, reason: 'Historical USD usage', created_at: held.created_at, actor_user_id: null }] })
    const { request } = setup(true, path => path.endsWith('/wallets') ? list() : usd)
    expect(await screen.findByText('9.00 USD')).toBeInTheDocument()
    expect(screen.getByText('Wallet currency: USD')).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: /Monthly limit \(USD\)/ })).toHaveValue('20')
    expect(screen.getByText('−0.123456 USD')).toBeInTheDocument()
    const amount = screen.getByRole('textbox', { name: 'Amount (USD)' })
    expect(screen.queryByRole('textbox', { name: 'Amount (EUR)' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByText(/Advanced: reconcile held request/))
    expect(screen.getByRole('textbox', { name: /Verified actual cost \(USD\)/ })).toBeInTheDocument()
    fireEvent.change(amount, { target: { value: '0.123456' } })
    fireEvent.change(screen.getByRole('textbox', { name: 'Booking note' }), { target: { value: 'USD allocation without conversion' } })
    fireEvent.click(screen.getByRole('button', { name: 'Record credit allocation' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/wallet/entries`, expect.anything(), expect.objectContaining({ amount_micros: 123456 }), 'POST'))
  })

  it('shows reserved credit and coverage honestly with read-only access', async () => {
    const { request } = setup(false)
    await screen.findByText('Wallet limits active')
    expect(screen.getByText('9.00 €')).toBeInTheDocument()
    expect(screen.getAllByText('1.00 €')).toHaveLength(2)
    expect(screen.getByText(/Direct provider requests, local models and other traffic are outside this budget/)).toBeInTheDocument()
    expect(screen.getByText(/Read-only access/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Save wallet limits' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Record credit allocation' })).not.toBeInTheDocument()
    expect(request.mock.calls.every(call => call[3] === 'GET')).toBe(true)
  })

  it('saves exact decimal micro-units with the wallet revision', async () => {
    const { request } = setup(true, (path, body, method) => method === 'PUT' ? detail({ revision: 8, monthly_limit_micros: (body as { monthly_limit_micros: number }).monthly_limit_micros }) : path.endsWith('/wallets') ? list() : detail())
    const input = await screen.findByRole('textbox', { name: /Monthly limit/ })
    fireEvent.change(input, { target: { value: '0,123456' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save wallet limits' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/wallet`, expect.anything(), { expected_revision: 7, enabled: true, monthly_limit_micros: 123456 }, 'PUT'))
    expect(await screen.findByRole('status')).toHaveTextContent('Wallet limits saved.')
  })

  it('reuses a booking idempotency key after a lost response and replaces it for a changed booking', async () => {
    const { request } = setup(true, (path, _body, method) => {
      if (method === 'POST') throw new Error('Response lost after network interruption')
      return path.endsWith('/wallets') ? list() : detail()
    })
    fireEvent.change(await screen.findByRole('textbox', { name: 'Amount (EUR)' }), { target: { value: '0.123456' } })
    fireEvent.change(screen.getByRole('textbox', { name: 'Booking note' }), { target: { value: 'October allocation' } })
    fireEvent.click(screen.getByRole('button', { name: 'Record credit allocation' }))
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('button', { name: 'Record credit allocation' }))
    await waitFor(() => expect(request.mock.calls.filter(call => call[3] === 'POST')).toHaveLength(2))
    await screen.findByRole('alert')
    const writes = request.mock.calls.filter(call => call[3] === 'POST'), first = writes[0][2] as { request_id: string }
    expect(writes[1][2]).toEqual(first)
    expect(first).toEqual(expect.objectContaining({ amount_micros: 123456, expected_revision: 7, kind: 'credit', reason: 'October allocation' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Amount (EUR)' }), { target: { value: '1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Record credit allocation' }))
    await waitFor(() => expect(request.mock.calls.filter(call => call[3] === 'POST')).toHaveLength(3))
    const changed = request.mock.calls.filter(call => call[3] === 'POST')[2][2] as { request_id: string }
    expect(changed.request_id).not.toBe(first.request_id)
  })

  it('blocks unsupported precision and amounts beyond the server ceiling', async () => {
    const { request } = setup()
    const amount = await screen.findByRole('textbox', { name: 'Amount (EUR)' })
    fireEvent.change(screen.getByRole('textbox', { name: 'Booking note' }), { target: { value: 'Allocation' } })
    for (const value of ['1e3', '0.0000001', '9000001', '-1']) {
      fireEvent.change(amount, { target: { value } })
      expect(screen.getByRole('button', { name: 'Record credit allocation' })).toBeDisabled()
    }
    expect(request.mock.calls.some(call => call[3] !== 'GET')).toBe(false)
  })

  it('never offers reserved credit for withdrawal and sends a positive debit amount', async () => {
    const { request } = setup(true, (path, _body, method) => method === 'POST' ? detail({ revision: 8 }) : path.endsWith('/wallets') ? list() : detail())
    fireEvent.change(await screen.findByRole('combobox', { name: 'Booking type' }), { target: { value: 'debit' } })
    fireEvent.change(screen.getByRole('textbox', { name: 'Booking note' }), { target: { value: 'Withdraw unused allocation' } })
    fireEvent.change(screen.getByRole('textbox', { name: 'Amount (EUR)' }), { target: { value: '10' } })
    expect(screen.getByRole('button', { name: 'Record credit withdrawal' })).toBeDisabled()
    expect(screen.getByRole('alert')).toHaveTextContent('Only unreserved credit can be withdrawn. Available: 9.00 €')
    fireEvent.change(screen.getByRole('textbox', { name: 'Amount (EUR)' }), { target: { value: '9' } })
    fireEvent.click(screen.getByRole('button', { name: 'Record credit withdrawal' }))
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/wallet/entries`, expect.anything(), expect.objectContaining({ expected_revision: 7, kind: 'debit', amount_micros: 9000000 }), 'POST'))
  })

  it('does not display a previous member response after another wallet is selected', async () => {
    let finish!: (value: WalletDetail) => void
    const { view } = setup(true, path => path.endsWith('/wallets') ? list() : path.includes(secondUserId) ? detail({ user_id: secondUserId, display_name: 'Samira', available_micros: 5000000 }) : new Promise(resolve => { finish = resolve }))
    await screen.findByRole('option', { name: 'Samira · samira@example.test' })
    fireEvent.change(screen.getByRole('combobox', { name: 'Member wallet' }), { target: { value: secondUserId } })
    await screen.findByText('5.00 €')
    await act(async () => finish(detail({ available_micros: 9000000 })))
    expect(screen.queryByText('9.00 €')).not.toBeInTheDocument()
    expect(view.container).toHaveTextContent('5.00 €')
  })

  it('drops an old organisation response and never labels inactive enforcement as active', async () => {
    let finish!: (value: WalletDetail) => void
    const { client, view } = setup(true, path => path.endsWith('/wallets') ? list(path.includes(otherOrgId) ? otherOrgId : orgId) : path.includes(otherOrgId) ? detail({ organisation_id: otherOrgId, enabled: false, enforcement: { state: 'disabled', coverage: 'server_managed_models', automatic_charging: true } }) : new Promise(resolve => { finish = resolve }))
    await screen.findByRole('combobox', { name: 'Member wallet' })
    view.rerender(<EnterpriseWallet client={client} orgId={otherOrgId} canManage enabled />)
    await screen.findByText('Wallet limits inactive')
    await act(async () => finish(detail()))
    expect(screen.queryByText('Wallet limits active')).not.toBeInTheDocument()
    expect(screen.getByText(/Its credit and monthly limit do not currently block model calls/)).toBeInTheDocument()
  })

  it('does not apply an old organisation mutation response after switching organisations', async () => {
    let finish!: (value: WalletDetail) => void
    const { client, request, view } = setup(true, (path, _body, method) => method === 'PUT' ? new Promise(resolve => { finish = resolve }) : path.endsWith('/wallets') ? list(path.includes(otherOrgId) ? otherOrgId : orgId) : detail({ organisation_id: path.includes(otherOrgId) ? otherOrgId : orgId }))
    fireEvent.change(await screen.findByRole('textbox', { name: /Monthly limit/ }), { target: { value: '42' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save wallet limits' }))
    await waitFor(() => expect(finish).toBeTypeOf('function'))
    view.rerender(<EnterpriseWallet client={client} orgId={otherOrgId} canManage enabled />)
    await screen.findByRole('textbox', { name: /Monthly limit/ })
    const requestsBeforeOldReply = request.mock.calls.length
    await act(async () => finish(detail({ revision: 8, monthly_limit_micros: 42000000 })))
    expect(screen.getByRole('textbox', { name: /Monthly limit/ })).toHaveValue('20')
    expect(screen.queryByText('Wallet limits saved.')).not.toBeInTheDocument()
    expect(request).toHaveBeenCalledTimes(requestsBeforeOldReply)
  })

  it('requires a reviewed actual cost within the held reservation and an explicit acknowledgement', async () => {
    const { request } = setup(true, (path, _body, method) => method === 'POST' ? detail({ revision: 8, reservations: [{ ...held, state: 'settled', charged_micros: 123456 }] }) : path.endsWith('/wallets') ? list() : detail({ reservations: [held] }))
    fireEvent.click(await screen.findByText(/Advanced: reconcile held request/))
    const amount = screen.getByRole('textbox', { name: /Verified actual cost/ })
    const note = screen.getByRole('textbox', { name: 'Reconciliation note' })
    const acknowledge = screen.getByRole('checkbox', { name: 'I have checked the costs actually billed by the provider.' })
    const submit = screen.getByRole('button', { name: 'Record verified cost and reconcile' })
    fireEvent.change(amount, { target: { value: '1.000001' } })
    fireEvent.change(note, { target: { value: 'Checked provider request receipt' } })
    fireEvent.click(acknowledge)
    expect(submit).toBeDisabled()
    fireEvent.change(amount, { target: { value: '0.123456' } })
    expect(acknowledge).not.toBeChecked()
    expect(submit).toBeDisabled()
    expect(request.mock.calls.some(call => call[3] === 'POST')).toBe(false)
    fireEvent.click(acknowledge)
    fireEvent.click(submit)
    await waitFor(() => expect(request).toHaveBeenCalledWith(`/organisations/${orgId}/members/${userId}/wallet/reservations/${reservationId}/resolve`, expect.anything(), expect.objectContaining({ expected_revision: 7, actual_micros: 123456, reason: 'Checked provider request receipt', request_id: expect.any(String) }), 'POST'))
    expect(await screen.findByRole('status')).toHaveTextContent('Reservation reconciled with the verified provider cost.')
  })

  it('reuses the same resolution key after a lost response and hides reconciliation from read-only users', async () => {
    const { request, client, view } = setup(true, (path, _body, method) => {
      if (method === 'POST') throw new Error('Response lost')
      return path.endsWith('/wallets') ? list() : detail({ reservations: [held, { ...held, id: secondUserId, state: 'reserved' }] })
    })
    fireEvent.click(await screen.findByText(/Advanced: reconcile held request/))
    expect(screen.getAllByText(/Advanced: reconcile held request/)).toHaveLength(1)
    fireEvent.change(screen.getByRole('textbox', { name: /Verified actual cost/ }), { target: { value: '0' } })
    fireEvent.change(screen.getByRole('textbox', { name: 'Reconciliation note' }), { target: { value: 'Provider confirmed this request was not billed' } })
    fireEvent.click(screen.getByRole('checkbox', { name: 'I have checked the costs actually billed by the provider.' }))
    fireEvent.click(screen.getByRole('button', { name: 'Record verified cost and reconcile' }))
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('button', { name: 'Record verified cost and reconcile' }))
    await waitFor(() => expect(request.mock.calls.filter(call => call[3] === 'POST')).toHaveLength(2))
    const writes = request.mock.calls.filter(call => call[3] === 'POST')
    expect(writes[1][2]).toEqual(writes[0][2])
    expect(writes[0][2]).toEqual(expect.objectContaining({ actual_micros: 0 }))
    view.rerender(<EnterpriseWallet client={client} orgId={orgId} canManage={false} enabled />)
    expect(screen.queryByText(/Advanced: reconcile held request/)).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Record verified cost and reconcile' })).not.toBeInTheDocument()
  })
})
