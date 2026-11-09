import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import EnterpriseSso from './EnterpriseSso'
import type { ManagementClient, OrganisationSsoSettings } from '../runtime/managementClient'

const org='07fce15a-8c7f-4aa4-bb8b-a07d821015a5'
const settings:OrganisationSsoSettings={organisation_id:org,revision:4,enabled:true,available:true,provider_id:'company',auto_provision:true,group_mappings:[{group:'staff',role:'member'}],providers:[{id:'company',label:'Company SSO',issuer:'https://identity.example',groups_claim:'groups'}]}
describe('Organisation SSO settings',()=>{
  beforeEach(async()=>{await i18n.changeLanguage('en')})
  it('requires acknowledging revocation and preserves a revision conflict for explicit reload',async()=>{
    const sso=vi.fn().mockResolvedValue(settings)
    const updateSso=vi.fn().mockRejectedValue(new Error('reload SSO settings before saving'))
    render(<EnterpriseSso client={{sso,updateSso} as unknown as ManagementClient} orgId={org} serverUrl="https://management.example" />)
    await screen.findByLabelText('Group 1')
    expect(screen.getByLabelText('Sign-in link')).toHaveValue(`https://management.example/server?organisation=${org}`)
    expect(screen.getByRole('button',{name:'Save SSO settings'})).toBeDisabled()
    fireEvent.change(screen.getByLabelText('Role 1'),{target:{value:'auditor'}})
    fireEvent.click(screen.getByLabelText(/I understand that saving/))
    fireEvent.click(screen.getByRole('button',{name:'Save SSO settings'}))
    await screen.findByRole('alert')
    expect(updateSso).toHaveBeenCalledWith(org,{expected_revision:4,enabled:true,provider_id:'company',auto_provision:true,group_mappings:[{group:'staff',role:'auditor'}]})
    expect(screen.getByLabelText('Role 1')).toHaveValue('auditor')
    expect(sso).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button',{name:'Reload SSO settings'}))
    await waitFor(()=>expect(sso).toHaveBeenCalledTimes(2))
    expect(await screen.findByLabelText('Role 1')).toHaveValue('member')
    expect(screen.getByRole('button',{name:'Save SSO settings'})).toBeDisabled()
  })
  it('does not offer an unapproved provider or hide missing operator setup',async()=>{
    const sso=vi.fn().mockResolvedValue({...settings,providers:[],available:false})
    render(<EnterpriseSso client={{sso} as unknown as ManagementClient} orgId={org} serverUrl="https://management.example" />)
    await screen.findByText(/No provider has been approved/)
    fireEvent.click(screen.getByLabelText(/I understand that saving/))
    expect(screen.getByRole('button',{name:'Save SSO settings'})).toBeDisabled()
    expect(screen.queryByRole('button',{name:'Link my account to this SSO provider'})).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    // A saved but unavailable configuration can still be switched off.
    fireEvent.click(screen.getByLabelText('Enable organisation SSO'))
    expect(screen.getByLabelText('Enable organisation SSO')).not.toBeChecked()
    expect(screen.getByLabelText('Enable organisation SSO')).toBeDisabled()
  })
  it('explains local administrator and authenticator setup before SSO and keeps an unconfigured provider disabled',async()=>{
    const sso=vi.fn().mockResolvedValue({...settings,enabled:false,available:false,provider_id:null,providers:[]})
    const updateSso=vi.fn()
    render(<EnterpriseSso client={{sso,updateSso} as unknown as ManagementClient} orgId={org} serverUrl="https://management.example" />)
    await screen.findByText(/No provider has been approved/)
    expect(screen.getByRole('heading',{name:'Set up SSO after initial setup'})).toBeInTheDocument()
    const steps=screen.getAllByRole('listitem')
    expect(steps[0]).toHaveTextContent('first local administrator')
    expect(steps[1]).toHaveTextContent('two-factor verification')
    expect(steps[1]).toHaveTextContent('recovery codes')
    expect(steps[2]).toHaveTextContent('organisation settings')
    expect(steps[2]).toHaveTextContent('SSO is off by default')
    expect(screen.getByLabelText('Enable organisation SSO')).not.toBeChecked()
    expect(screen.getByLabelText('Enable organisation SSO')).toBeDisabled()
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Organisation ID')).toHaveValue(org)
    expect(screen.queryByText(/enter its organisation ID in the desktop app/)).not.toBeInTheDocument()
    expect(updateSso).not.toHaveBeenCalled()
  })
  it('publishes the sign-in link only after the server confirms a saved active configuration',async()=>{
    let finish!: (value:OrganisationSsoSettings)=>void
    const sso=vi.fn().mockResolvedValue({...settings,enabled:false})
    const updateSso=vi.fn(()=>new Promise<OrganisationSsoSettings>(resolve=>{finish=resolve}))
    render(<EnterpriseSso client={{sso,updateSso} as unknown as ManagementClient} orgId={org} serverUrl="https://management.example/" />)
    const enable=await screen.findByLabelText('Enable organisation SSO')
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    fireEvent.click(enable)
    fireEvent.click(screen.getByLabelText(/I understand that saving/))
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    expect(screen.queryByRole('button',{name:'Link my account to this SSO provider'})).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button',{name:'Save SSO settings'}))
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    await act(async()=>finish({...settings,revision:5}))
    expect(await screen.findByLabelText('Sign-in link')).toHaveValue(`https://management.example/server?organisation=${org}`)
    expect(screen.getByRole('button',{name:'Link my account to this SSO provider'})).toBeInTheDocument()
    // An unsaved disable does not misrepresent the still-active saved setting.
    fireEvent.click(screen.getByLabelText('Enable organisation SSO'))
    expect(screen.getByLabelText('Sign-in link')).toBeInTheDocument()
  })
  it('does not publish a previous organisation after its save response arrives late',async()=>{
    let finish!: (value:OrganisationSsoSettings)=>void
    const other='ef65fe8a-c9e9-4391-adca-093808360280'
    const sso=vi.fn().mockResolvedValueOnce({...settings,enabled:false}).mockResolvedValueOnce({...settings,organisation_id:other,enabled:false})
    const updateSso=vi.fn(()=>new Promise<OrganisationSsoSettings>(resolve=>{finish=resolve}))
    const client={sso,updateSso} as unknown as ManagementClient
    const view=render(<EnterpriseSso client={client} orgId={org} serverUrl="https://management.example" />)
    fireEvent.click(await screen.findByLabelText('Enable organisation SSO'))
    fireEvent.click(screen.getByLabelText(/I understand that saving/))
    fireEvent.click(screen.getByRole('button',{name:'Save SSO settings'}))
    view.rerender(<EnterpriseSso client={client} orgId={other} serverUrl="https://management.example" />)
    await waitFor(()=>expect(sso).toHaveBeenCalledTimes(2))
    await act(async()=>finish(settings))
    expect(screen.queryByLabelText('Sign-in link')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Enable organisation SSO')).not.toBeChecked()
  })
})
