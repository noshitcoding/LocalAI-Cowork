import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import EnterpriseConsoleLayout from './EnterpriseConsoleLayout'
import { ManagementSecurityContext } from './managementSecurityContext'

vi.mock('../runtime/oidc', () => ({ oidcEnabled: vi.fn(async () => false) }))

const client = {
  totpStatus: vi.fn(async () => ({ schema_version:2, enabled:true, unused_recovery_codes:8 })),
  listPasskeys: vi.fn(async () => []), listAuthSessions: vi.fn(async () => []),
  passkeysAvailableInContext: () => false,
} as unknown as RemoteRuntimeClient
const layout = <EnterpriseConsoleLayout area="overview" onArea={vi.fn()} organisations={[]} organisationId="" onOrganisation={vi.fn()} platformAdmin busy={false} serverUrl="https://management.example" email="admin@example.test" admissionEnabled importsEnabled={false} onRefresh={vi.fn()} onLogout={vi.fn()}><h2>Management content</h2></EnterpriseConsoleLayout>

describe('management account security placement', () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    await i18n.changeLanguage('en')
    Object.defineProperties(HTMLDialogElement.prototype, {
      showModal: { configurable:true, value:function(this:HTMLDialogElement){this.open=true} },
      close: { configurable:true, value:function(this:HTMLDialogElement){this.open=false} },
    })
  })

  it('keeps account security in the header and opens its settings in a modal', async () => {
    const { container } = render(<ManagementSecurityContext.Provider value={{ client, requiredMfa:true }}>{layout}</ManagementSecurityContext.Provider>)
    expect(container.firstElementChild).toHaveClass('enterprise-shell')
    expect(container.querySelectorAll('.enterprise-management')).toHaveLength(1)
    expect(client.totpStatus).not.toHaveBeenCalled()
    const trigger = screen.getByRole('button', { name:'Account security' })
    expect(trigger.closest('header')).toHaveClass('enterprise-topbar')
    trigger.focus()
    fireEvent.click(trigger)
    const dialog = await screen.findByRole('dialog', { name:'Account security' })
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    await screen.findByLabelText('Authenticator or recovery code')
    expect(screen.getByRole('button', { name:'Disable TOTP' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name:'Close security settings' }))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(trigger).toHaveFocus()
    fireEvent.click(trigger)
    fireEvent(await screen.findByRole('dialog', { name:'Account security' }), new Event('cancel', { cancelable:true }))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('does not expose account settings without the verified gate context', () => {
    render(layout)
    expect(screen.queryByRole('button', { name:'Account security' })).not.toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('keeps pending operations and newly returned recovery codes visible until acknowledged', async () => {
    let resolveCodes!: (value: { recovery_codes:string[] }) => void
    const response = new Promise<{ recovery_codes:string[] }>(resolve => { resolveCodes = resolve })
    const regeneration = vi.fn(() => response)
    render(<ManagementSecurityContext.Provider value={{ client:{ ...client, regenerateRecoveryCodes:regeneration } as unknown as RemoteRuntimeClient, requiredMfa:true }}>{layout}</ManagementSecurityContext.Provider>)
    fireEvent.click(screen.getByRole('button', { name:'Account security' }))
    fireEvent.change(await screen.findByLabelText('Authenticator or recovery code'), { target:{ value:'test-recovery-code' } })
    fireEvent.click(screen.getByRole('button', { name:'Replace recovery codes' }))
    const dialog = screen.getByRole('dialog', { name:'Account security' })
    expect(screen.getByRole('button', { name:'Close security settings' })).toBeDisabled()
    fireEvent(dialog, new Event('cancel', { cancelable:true }))
    expect(dialog).toBeInTheDocument()
    await act(async () => { resolveCodes({ recovery_codes:['new-once-only-code'] }) })
    expect(await screen.findByText('new-once-only-code')).toBeInTheDocument()
    expect(screen.getByRole('button', { name:'Close security settings' })).toBeDisabled()
    fireEvent(dialog, new Event('cancel', { cancelable:true }))
    expect(dialog).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name:'I saved these codes' }))
    expect(screen.getByRole('button', { name:'Close security settings' })).toBeEnabled()
    fireEvent(dialog, new Event('cancel', { cancelable:true }))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })
})
