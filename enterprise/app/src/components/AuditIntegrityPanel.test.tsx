import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import AuditIntegrityPanel, { type AuditIntegrityReport } from './AuditIntegrityPanel'
import i18n from '../i18n'

const invoke = vi.fn()
vi.mock('../utils/safeInvoke', () => ({ safeInvoke: (...args: unknown[]) => invoke(...args) }))

const damaged: AuditIntegrityReport = {
  version: 2, status: 'legacy_damaged', trustLevel: 'broken', signedEvents: 0,
  legacyEvents: 948, malformedEvents: 1, firstProblemLine: 896,
  errorCode: 'audit_malformed_legacy_record', assessmentId: 'snapshot-1', recoveryAllowed: true, recovery: null,
}

describe('AuditIntegrityPanel', () => {
  beforeEach(async () => { invoke.mockReset(); await i18n.changeLanguage('en'); invoke.mockResolvedValue(damaged) })
  it('only inspects on mount, shows counts without exposing raw log content', async () => {
    render(<AuditIntegrityPanel />)
    expect(await screen.findByText('Damaged legacy history')).toBeInTheDocument()
    expect(screen.getByText('948')).toBeInTheDocument()
    expect(screen.getByText('896')).toBeInTheDocument()
    expect(invoke).toHaveBeenCalledTimes(1)
    expect(invoke).toHaveBeenCalledWith('audit_integrity_inspect', undefined, null)
  })
  it('requires explicit confirmation and binds recovery to the inspected assessment', async () => {
    render(<AuditIntegrityPanel />)
    fireEvent.click(await screen.findByRole('button', { name: 'Review audit recovery' }))
    expect(invoke).toHaveBeenCalledTimes(1)
    expect(screen.getByText(/Earlier events remain unverified/)).toBeInTheDocument()
    invoke.mockResolvedValueOnce({ ...damaged, status: 'ok', recoveryAllowed: false })
    fireEvent.click(screen.getByRole('button', { name: 'Preserve history and restart audit' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('audit_recovery_start', { assessmentId: 'snapshot-1' }))
    expect(await screen.findByText('Verified audit chain')).toBeInTheDocument()
  })
  it('does not offer destructive recovery for a transient unavailable state', async () => {
    invoke.mockResolvedValue({ ...damaged, status: 'unavailable', recoveryAllowed: false, errorCode: 'audit_lock_busy' })
    render(<AuditIntegrityPanel />)
    await screen.findByText('Audit integrity unavailable')
    expect(screen.queryByRole('button', { name: 'Review audit recovery' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Inspect audit integrity' })).toBeEnabled()
  })
})
