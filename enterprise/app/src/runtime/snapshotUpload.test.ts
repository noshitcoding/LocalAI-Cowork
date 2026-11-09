import {File as NodeFile} from 'node:buffer'
import {webcrypto} from 'node:crypto'
import {afterEach,describe,expect,it,vi} from 'vitest'
import {prepareSnapshot,sendSnapshot,snapshotPath,snapshotRequired,snapshotSensitivePath,type SnapshotUploadClient,type SnapshotUploadRequest} from './snapshotUpload'
describe('explicit browser snapshots',()=>{
  afterEach(()=>vi.unstubAllGlobals())
  it('requires snapshots only for remote execution of private files and identifies sensitive selections',()=>{
    expect(snapshotRequired('private_local',{kind:'server_linux'})).toBe(true)
    expect(snapshotRequired('private_local',{kind:'managed_windows_pool'})).toBe(true)
    expect(snapshotRequired('private_local',{kind:'personal_device'})).toBe(false)
    expect(snapshotRequired('team_managed',{kind:'server_linux'})).toBe(false)
    expect(snapshotSensitivePath('project/.env')).toBe(true)
    expect(snapshotSensitivePath('project/.git/config')).toBe(true)
    expect(()=>snapshotPath({name:'../outside',webkitRelativePath:''})).toThrow()
  })
  it('hashes bounded chunks, deduplicates transfer and validates the committed original',async()=>{
    vi.stubGlobal('crypto',webcrypto)
    const file=new NodeFile([new Uint8Array(8*1024*1024)],'fixture.bin',{lastModified:0}) as unknown as File
    const prepared=await prepareSnapshot('project',[file])
    expect(prepared.chunks.size).toBe(1)
    expect(prepared.declaration.files[0].chunks).toHaveLength(2)
    const begin=vi.fn(async(_request:SnapshotUploadRequest)=>({schema_version:2,manifest_id:'manifest',missing_chunks:[...prepared.chunks.keys()],max_chunk_bytes:16*1024*1024,expires_at:null,warnings:[]}))
    const upload=vi.fn(async()=>({warnings:[]}))
    const commit=vi.fn(async()=>({schema_version:2,id:'manifest',project_id:'project',total_bytes:file.size,encryption_key_id:'fixture',created_at:new Date(0).toISOString(),expires_at:null,files:prepared.declaration.files.map(f=>({...f,modified_at:'1970-01-01T00:00:00+00:00',chunks:f.chunks.map(c=>({...c,ciphertext_size:c.plaintext_size+16}))}))}))
    const client:SnapshotUploadClient={beginSnapshotUpload:begin,uploadSnapshotChunk:upload,commitSnapshot:commit}
    expect((await sendSnapshot(client,prepared,'original-key')).id).toBe('manifest')
    expect(begin.mock.calls[0]?.[0]).toMatchObject({idempotency_key:'original-key'})
    expect(upload).toHaveBeenCalledTimes(1)
    commit.mockResolvedValueOnce({...await commit(),project_id:'other-project'})
    await expect(sendSnapshot(client,prepared,'original-key')).rejects.toThrow(/identity differs/)
  })
})
