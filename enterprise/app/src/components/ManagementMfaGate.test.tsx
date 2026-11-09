import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, useNavigate } from 'react-router-dom'
import i18n from '../i18n'
import { useContext } from 'react'
import { ManagementSecurityContext } from './managementSecurityContext'

const mocks = vi.hoisted(() => ({ status:vi.fn(),setup:vi.fn(),enable:vi.fn(),verify:vi.fn(),logout:vi.fn(),passkey:vi.fn(),primary:vi.fn() }))
vi.mock('../stores/remoteRuntimeStore', () => ({
  useRemoteRuntimeStore: () => ({ serverUrl:'https://management.example',userId:'admin',email:'admin@example.test',logout:mocks.logout,loginPasskey:mocks.passkey,hasRecentPrimaryLogin:mocks.primary }),
  remoteRuntimeClient: () => ({ authenticationAssurance:mocks.status,setupTotp:mocks.setup,enableTotp:mocks.enable,verifySecondFactor:mocks.verify }),
}))
vi.mock('./RemoteSecuritySettings', () => ({ default: () => <div>Account settings</div> }))
vi.mock('qrcode', () => ({ toDataURL: vi.fn(async () => 'data:image/png;base64,bG9jYWw=') }))
import ManagementMfaGate from './ManagementMfaGate'

const restricted={ required:true,totp_enabled:false,passkey_registered:false,session_verified:false,enrollment_allowed:true,reauthentication_required:false }
const verified={ ...restricted,totp_enabled:true,session_verified:true }
const setup={ secret:'TESTAUTHENTICATORSECRET',expires_at:'2099-01-01T00:00:00Z',otpauth_uri:'otpauth://totp/test?secret=TESTAUTHENTICATORSECRET' }
const page=(key='server-a')=><MemoryRouter><ManagementMfaGate key={key}><div>Management content</div></ManagementMfaGate></MemoryRouter>
const organisationA = '10000000-0000-4000-8000-000000000001'
const organisationB = '10000000-0000-4000-8000-000000000002'
function OrganisationSwitcher() {
  const navigate = useNavigate()
  return <button type="button" onClick={() => { void navigate(`/?organisation=${organisationB}`) }}>Switch organisation</button>
}
function SecurityContextProbe() {
  const value = useContext(ManagementSecurityContext)
  return <div>{value ? 'Verified header security available' : 'Header security unavailable'}</div>
}

describe('management administrator assurance', () => {
  beforeEach(async () => {
    vi.resetAllMocks()
    Object.defineProperties(HTMLDialogElement.prototype, {
      showModal: { configurable:true, value:function(this:HTMLDialogElement){this.open=true} },
      close: { configurable:true, value:function(this:HTMLDialogElement){this.open=false} },
    })
    await i18n.changeLanguage('en'); mocks.primary.mockReturnValue(true); mocks.logout.mockResolvedValue(undefined); mocks.status.mockResolvedValue(restricted); mocks.setup.mockResolvedValue(setup); mocks.enable.mockResolvedValue({ recovery_codes:['test-recovery-one','test-recovery-two'] }); mocks.verify.mockResolvedValue(verified)
  })
  it('returns restored unverified sessions to password sign-in without exposing MFA inputs', async () => {
    mocks.primary.mockReturnValue(false)
    mocks.status.mockResolvedValue({ ...restricted, totp_enabled:true })
    render(page())
    await waitFor(() => expect(mocks.logout).toHaveBeenCalledTimes(1))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Authenticator or recovery code')).not.toBeInTheDocument()
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    expect(mocks.setup).not.toHaveBeenCalled()
    expect(mocks.verify).not.toHaveBeenCalled()
  })
  it('keeps a server-verified restored session usable without another primary prompt', async () => {
    mocks.primary.mockReturnValue(false)
    mocks.status.mockResolvedValue(verified)
    render(page())
    await screen.findByText('Management content')
    expect(mocks.logout).not.toHaveBeenCalled()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })
  it('provides header security only after verification without adding a second main container', async () => {
    const { container } = render(<MemoryRouter><ManagementMfaGate securityInHeader><SecurityContextProbe /></ManagementMfaGate></MemoryRouter>)
    await screen.findByRole('dialog', { name:'Confirm your sign-in' })
    expect(screen.queryByText('Verified header security available')).not.toBeInTheDocument()
    mocks.status.mockResolvedValue(verified)
    fireEvent(window, new Event('focus'))
    expect(await screen.findByText('Verified header security available')).toBeInTheDocument()
    expect(container.querySelector('.enterprise-management')).not.toBeInTheDocument()
    expect(screen.queryByText('Account settings')).not.toBeInTheDocument()
  })
  it('shows enrollment only in a modal after a fresh primary login and cancels by signing out', async () => {
    render(page())
    const dialog=await screen.findByRole('dialog', { name:'Confirm your sign-in' })
    expect(dialog).toHaveAttribute('aria-modal','true')
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    fireEvent(dialog,new Event('cancel',{cancelable:true}))
    await waitFor(()=>expect(mocks.logout).toHaveBeenCalledTimes(1))
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
  })
  it('holds management until enrollment and recovery-code acknowledgement', async () => {
    render(page())
    fireEvent.click(await screen.findByRole('button', { name:'Set up authenticator' }))
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    expect(await screen.findByRole('img', { name: 'QR code for your authenticator app' })).toHaveAttribute('src', 'data:image/png;base64,bG9jYWw=')
    expect(screen.getByText(/In Microsoft Authenticator/)).toHaveTextContent('Other account')
    expect(await screen.findByLabelText('Authenticator secret')).toHaveValue(setup.secret)
    fireEvent.change(screen.getByLabelText('Six-digit code'), { target:{ value:'123456' } })
    fireEvent.click(screen.getByRole('button', { name:'Verify and enable' }))
    await screen.findByRole('heading', { name:'Save your recovery codes' })
    expect(mocks.enable).toHaveBeenCalledWith('123456')
    mocks.status.mockResolvedValue(verified)
    fireEvent(window,new Event('focus'))
    await act(async () => {})
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name:'I saved the codes — continue' }))
    await screen.findByText('Management content')
    expect(screen.queryByLabelText('Recovery codes')).not.toBeInTheDocument()
  })
  it('verifies an enrolled factor and leaves rejected codes restricted', async () => {
    mocks.status.mockResolvedValue({ ...restricted,totp_enabled:true,enrollment_allowed:false })
    mocks.verify.mockRejectedValueOnce(new Error('Code already used'))
    render(page())
    fireEvent.change(await screen.findByLabelText('Authenticator or recovery code'), { target:{ value:'already-used' } })
    fireEvent.click(screen.getByRole('button', { name:'Verify this session' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Code already used')
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Authenticator or recovery code'), { target:{ value:'new-recovery' } })
    fireEvent.click(screen.getByRole('button', { name:'Verify this session' }))
    await screen.findByText('Management content')
    expect(mocks.verify).toHaveBeenLastCalledWith('new-recovery', undefined)
  })
  it('requires a fresh login before enrolling from an old session', async () => {
    mocks.status.mockResolvedValue({ ...restricted,enrollment_allowed:false,reauthentication_required:true })
    render(page())
    await screen.findByText(/Sign out and sign in again before setting up/)
    expect(screen.queryByRole('button', { name:'Set up authenticator' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name:'Sign out' }))
    expect(mocks.logout).toHaveBeenCalled()
  })
  it('does not mistake a configured passkey for proof in this session', async () => {
    mocks.status.mockResolvedValue({ ...restricted,passkey_registered:true,enrollment_allowed:false })
    render(page())
    fireEvent.click(await screen.findByRole('button', { name:'Sign in with a verified passkey' }))
    expect(mocks.passkey).toHaveBeenCalledWith('https://management.example','admin@example.test')
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
  })
  it('closes management when the next status check cannot prove access', async () => {
    mocks.status.mockResolvedValueOnce(verified).mockRejectedValueOnce(new Error('Server unavailable'))
    render(page())
    await screen.findByText('Management content')
    fireEvent(window,new Event('focus'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Server unavailable')
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
  })
  it('allows ordinary members without changing their account security settings', async () => {
    mocks.status.mockResolvedValue({ ...restricted,required:false })
    render(page())
    await screen.findByText('Management content')
    expect(mocks.setup).not.toHaveBeenCalled()
    expect(screen.queryByText('This access requires an additional sign-in check.')).not.toBeInTheDocument()
  })
  it('discards an enrollment response from a disconnected server', async () => {
    let finish!: (value:unknown)=>void
    mocks.setup.mockImplementation(()=>new Promise(resolve=>{finish=resolve}))
    const view=render(page())
    fireEvent.click(await screen.findByRole('button', { name:'Set up authenticator' }))
    await waitFor(()=>expect(mocks.setup).toHaveBeenCalled())
    view.rerender(page('server-b'))
    await act(async()=>finish(setup))
    expect(screen.queryByLabelText('Authenticator secret')).not.toBeInTheDocument()
  })
  it('shows loading and a retry without opening access when status is unavailable', async () => {
    mocks.status.mockRejectedValueOnce(new Error('Server unavailable'))
    render(page())
    expect(screen.getByRole('status')).toHaveTextContent('Checking sign-in verification')
    expect(await screen.findByRole('alert')).toHaveTextContent('Server unavailable')
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await screen.findByRole('button', { name: 'Set up authenticator' })
    expect(screen.getByText('This access requires an additional sign-in check.')).toBeInTheDocument()
  })
  it('does not let polling bypass recovery acknowledgement while enable is still pending', async () => {
    let finishEnable!: (value: unknown) => void
    let finishOldStatus!: (value: unknown) => void
    mocks.enable.mockImplementationOnce(() => new Promise(resolve => { finishEnable = resolve }))
    mocks.status.mockResolvedValueOnce(restricted).mockImplementationOnce(() => new Promise(resolve => { finishOldStatus = resolve }))
    render(page())
    fireEvent.click(await screen.findByRole('button', { name: 'Set up authenticator' }))
    await screen.findByLabelText('Six-digit code')
    fireEvent(window, new Event('focus'))
    expect(mocks.status).toHaveBeenCalledTimes(2)
    fireEvent.change(screen.getByLabelText('Six-digit code'), { target: { value: '123456' } })
    fireEvent.click(screen.getByRole('button', { name: 'Verify and enable' }))
    mocks.status.mockResolvedValue(verified)
    fireEvent(window, new Event('focus'))
    expect(mocks.status).toHaveBeenCalledTimes(2)
    await act(async () => finishOldStatus(verified))
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    await act(async () => finishEnable({ recovery_codes: ['test-recovery-one'] }))
    await screen.findByRole('heading', { name: 'Save your recovery codes' })
    fireEvent(window, new Event('focus'))
    expect(mocks.status).toHaveBeenCalledTimes(2)
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'I saved the codes — continue' }))
    await screen.findByText('Management content')
    expect(mocks.status).toHaveBeenCalledTimes(3)
  })
  it('closes the old organisation immediately and discards its in-flight assurance response', async () => {
    let finishOldStatus!: (value: unknown) => void
    let finishNewStatus!: (value: unknown) => void
    mocks.status.mockResolvedValueOnce(verified)
      .mockImplementationOnce(() => new Promise(resolve => { finishOldStatus = resolve }))
      .mockImplementationOnce(() => new Promise(resolve => { finishNewStatus = resolve }))
    render(<MemoryRouter initialEntries={[`/?organisation=${organisationA}`]}><OrganisationSwitcher /><ManagementMfaGate><div>Management content</div></ManagementMfaGate></MemoryRouter>)
    await screen.findByText('Management content')
    fireEvent(window, new Event('focus'))
    expect(mocks.status).toHaveBeenCalledTimes(2)
    fireEvent.click(screen.getByRole('button', { name: 'Switch organisation' }))
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    expect(mocks.status).toHaveBeenLastCalledWith(organisationB)
    await act(async () => finishOldStatus(verified))
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
    await act(async () => finishNewStatus(restricted))
    await screen.findByRole('button', { name: 'Set up authenticator' })
    expect(screen.queryByText('Management content')).not.toBeInTheDocument()
  })
})
