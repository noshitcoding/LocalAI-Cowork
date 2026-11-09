import {fireEvent,render,screen,waitFor} from '@testing-library/react'
import {beforeEach,describe,expect,it,vi} from 'vitest'
import '../i18n'
import i18n from 'i18next'
import EnterpriseCapacity from './EnterpriseCapacity'
import type {ExecutionCapacity,ManagedInstance,ManagementClient} from '../runtime/managementClient'

const org='07fce15a-8c7f-4aa4-bb8b-a07d821015a5'
const instance={id:'ede4cdd2-6e2f-4c31-97ab-9be15b86f97c',organisation_id:org,name:'Fixture',revision:8,state:'ready',backend:'compose',policy:{minimum_isolation:'container'},can_manage:true,available_actions:['stop']} as ManagedInstance
const id='11111111-1111-4111-8111-111111111111'
const capacity:ExecutionCapacity={organisation_id:org,budget:{revision:2,organisation_concurrent_runs:4,user_concurrent_runs:2},held_runs:2,reservations:[
  {id,organisation_id:org,instance_id:instance.id,execution_epoch:id,run_id:id,user_id:id,target:{kind:'server_linux',pool_id:null},created_at:'2026-09-27T12:00:00Z'},
  {id:'22222222-2222-4222-8222-222222222222',organisation_id:org,instance_id:instance.id,execution_epoch:id,run_id:id,user_id:id,target:{kind:'personal_device',device_id:id},created_at:'2026-09-27T12:00:00Z'},
]}
function mock(){return {executionCapacity:vi.fn().mockResolvedValue(capacity),instances:vi.fn().mockResolvedValue([instance]),recoverExecutions:vi.fn().mockResolvedValue({id,state:'queued'}),setExecutionBudget:vi.fn().mockRejectedValue(new Error('budget revision changed'))}}
describe('Execution capacity review',()=>{
  beforeEach(async()=>{await i18n.changeLanguage('en')})
  it('requires explicit stop acknowledgement and submits only the scoped server reservation',async()=>{
    const api=mock()
    render(<EnterpriseCapacity client={api as unknown as ManagementClient} orgId={org} canManage/>)
    fireEvent.click(await screen.findByRole('button',{name:'Review server slots: Fixture (1)'}))
    expect(screen.getByRole('button',{name:'Stop and verify'})).toBeDisabled()
    expect(screen.getByText(/This stops the whole instance/)).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('I acknowledge stopping the entire instance.'))
    fireEvent.click(screen.getByRole('button',{name:'Stop and verify'}))
    await waitFor(()=>expect(api.recoverExecutions).toHaveBeenCalledWith(instance,[id]))
    expect(api.recoverExecutions).toHaveBeenCalledTimes(1)
    expect(await screen.findByRole('status')).toHaveTextContent(id)
  })
  it('keeps auditor access read-only and does not present recovery for device-only reservations',async()=>{
    const api=mock()
    const view=render(<EnterpriseCapacity client={api as unknown as ManagementClient} orgId={org} canManage={false}/>)
    await screen.findByText('Reserved task slots:')
    expect(screen.getByLabelText('Organisation task limit')).toBeDisabled()
    expect(screen.queryByRole('button',{name:/Review server slots/})).not.toBeInTheDocument()
    expect(screen.queryByRole('button',{name:'Save task limits'})).not.toBeInTheDocument()
    const devices=mock(); devices.executionCapacity.mockResolvedValue({...capacity,reservations:[capacity.reservations[1]]})
    view.rerender(<EnterpriseCapacity client={devices as unknown as ManagementClient} orgId={org} canManage/>)
    await waitFor(()=>expect(devices.executionCapacity).toHaveBeenCalled())
    expect(screen.queryByRole('button',{name:/Review server slots/})).not.toBeInTheDocument()
  })
  it('disables confirmation when an instance revision changes while the review is open',async()=>{
    const api=mock()
    const view=render(<EnterpriseCapacity client={api as unknown as ManagementClient} orgId={org} canManage/>)
    fireEvent.click(await screen.findByRole('button',{name:'Review server slots: Fixture (1)'}))
    fireEvent.click(screen.getByLabelText('I acknowledge stopping the entire instance.'))
    const next=mock();next.instances.mockResolvedValue([{...instance,revision:9}])
    view.rerender(<EnterpriseCapacity client={next as unknown as ManagementClient} orgId={org} canManage/>)
    expect(await screen.findByRole('alert')).toHaveTextContent('changed')
    expect(screen.getByRole('button',{name:'Stop and verify'})).toBeDisabled()
    expect(next.recoverExecutions).not.toHaveBeenCalled()
  })
  it('retains conflicting budget edits for review and does not retry the mutation',async()=>{
    const api=mock()
    render(<EnterpriseCapacity client={api as unknown as ManagementClient} orgId={org} canManage/>)
    fireEvent.change(await screen.findByLabelText('Task limit per member'),{target:{value:'1'}})
    fireEvent.click(screen.getByRole('button',{name:'Save task limits'}))
    expect(await screen.findByRole('alert')).toHaveTextContent('budget revision changed')
    expect(screen.getByLabelText('Task limit per member')).toHaveValue(1)
    expect(api.setExecutionBudget).toHaveBeenCalledWith(org,{expected_revision:2,organisation_concurrent_runs:4,user_concurrent_runs:1})
    expect(api.setExecutionBudget).toHaveBeenCalledTimes(1)
  })
})
