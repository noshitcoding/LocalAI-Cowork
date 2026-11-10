import {webcrypto} from 'node:crypto'
import {fireEvent,render,screen,waitFor} from '@testing-library/react'
import {afterEach,beforeEach,describe,expect,it,vi} from 'vitest'
import i18n from '../i18n'
import {useRemoteRuntimeStore} from '../stores/remoteRuntimeStore'
import type {RemoteRuntimeClient} from '../runtime/runtimeClient'
import RemoteSnapshotUpload from './RemoteSnapshotUpload'
const project='10000000-0000-4000-8000-000000000001',manifest='10000000-0000-4000-8000-000000000002'
describe('private snapshot consent and recovery',()=>{
  beforeEach(async()=>{await i18n.changeLanguage('en');sessionStorage.clear();vi.stubGlobal('crypto',webcrypto);useRemoteRuntimeStore.setState({serverUrl:'https://fixture.example',userId:'fixture-user'})})
  afterEach(()=>{vi.unstubAllGlobals();useRemoteRuntimeStore.setState({serverUrl:'',userId:null})})
  it('requires explicit empty-file choice and consent and retains the original upload key after a lost response',async()=>{
    const begin=vi.fn().mockRejectedValueOnce(new Error('begin reply lost')).mockResolvedValue({schema_version:2,manifest_id:manifest,missing_chunks:[],max_chunk_bytes:16*1024*1024,expires_at:null,warnings:[]})
    const client={enterpriseSource:'https://fixture.example',beginSnapshotUpload:begin,commitSnapshot:vi.fn(async()=>({schema_version:2,id:manifest,project_id:project,total_bytes:0,files:[],encryption_key_id:'fixture',created_at:new Date(0).toISOString(),expires_at:null}))} as unknown as RemoteRuntimeClient
    const ready=vi.fn();render(<RemoteSnapshotUpload client={client} projectId={project} onReady={ready}/>)
    expect(screen.getByRole('button',{name:'Upload approved snapshot'})).toBeDisabled()
    fireEvent.click(screen.getByRole('checkbox',{name:/Run without project files/}))
    expect(screen.getByRole('button',{name:'Upload approved snapshot'})).toBeDisabled()
    fireEvent.click(screen.getByRole('checkbox',{name:/I approve transferring/}))
    fireEvent.click(screen.getByRole('button',{name:'Upload approved snapshot'}))
    expect(await screen.findByRole('alert')).toHaveTextContent('begin reply lost')
    expect(ready).not.toHaveBeenCalledWith(manifest)
    fireEvent.click(screen.getByRole('button',{name:'Upload approved snapshot'}))
    await waitFor(()=>expect(ready).toHaveBeenCalledWith(manifest))
    expect(begin.mock.calls[1][0].idempotency_key).toBe(begin.mock.calls[0][0].idempotency_key)
    expect(sessionStorage.getItem(sessionStorage.key(0)!)).not.toContain('Bearer')
  })
  it('uses a fresh request identifier only after an explicit restart of an expired upload',async()=>{
    const begin=vi.fn().mockRejectedValueOnce(new Error('the original snapshot upload has expired')).mockResolvedValue({schema_version:2,manifest_id:manifest,missing_chunks:[],max_chunk_bytes:16*1024*1024,expires_at:null,warnings:[]})
    const client={enterpriseSource:'https://fixture.example',beginSnapshotUpload:begin,commitSnapshot:vi.fn(async()=>({schema_version:2,id:manifest,project_id:project,total_bytes:0,files:[],encryption_key_id:'fixture',created_at:new Date(0).toISOString(),expires_at:null}))} as unknown as RemoteRuntimeClient
    const ready=vi.fn();render(<RemoteSnapshotUpload client={client} projectId={project} onReady={ready}/>)
    fireEvent.click(screen.getByRole('checkbox',{name:/Run without project files/}))
    fireEvent.click(screen.getByRole('checkbox',{name:/I approve transferring/}))
    fireEvent.click(screen.getByRole('button',{name:'Upload approved snapshot'}))
    expect(await screen.findByRole('alert')).toHaveTextContent('expired')
    expect(begin).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button',{name:'Begin a new upload after expiry'}))
    await waitFor(()=>expect(ready).toHaveBeenCalledWith(manifest))
    expect(begin.mock.calls[1][0].idempotency_key).not.toBe(begin.mock.calls[0][0].idempotency_key)
  })
})
