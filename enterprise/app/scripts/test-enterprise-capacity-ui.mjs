// Browser rendering and client request contract only: all API responses are fixtures.
import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from '@playwright/test'
import { preview } from 'vite'

const output = new URL('../../.internal/enterprise-local/', import.meta.url)
const org = '11111111-1111-4111-8111-111111111111'
const instanceId = '22222222-2222-4222-8222-222222222222'
const reservationId = '33333333-3333-4333-8333-333333333333'
const remoteId = '44444444-4444-4444-8444-444444444444'
const operationId = '55555555-5555-4555-8555-555555555555'
const policy = {revision:1,extensions:'catalog',content_access:'support_grant',minimum_isolation:'container',
  allow_laptop_full_access:false,allow_offline_continuation:false,allowed_network_origins:[],
  limits:{cpu_millis:4000,memory_mib:8192,storage_gib:50,concurrent_runs:4,instances:10},audit_retention_days:365,content_logging:false}
const instance = {id:instanceId,organisation_id:org,owner_user_id:org,name:'Server capacity fixture',backend:'compose',state:'ready',
  policy,revision:8,created_at:'2026-09-27T12:00:00Z',can_manage:true,available_actions:['stop']}
const row = {id:reservationId,organisation_id:org,instance_id:instanceId,execution_epoch:org,run_id:reservationId,user_id:org,
  target:{kind:'server_linux',pool_id:null},created_at:'2026-09-27T12:00:00Z'}
const server = await preview({mode:'web',preview:{host:'127.0.0.1',port:0,strictPort:true}})
const origin = `http://127.0.0.1:${server.httpServer.address().port}`
const browser = await chromium.launch({channel:'msedge',headless:true})
const checks=[]
try {
  for (const [name,width,height,role] of [['desktop',1440,1000,'admin'],['mobile',390,844,'admin'],['auditor',1440,1000,'auditor']]) {
    const page = await browser.newPage({viewport:{width,height},locale:'en-US'})
    const errors=[], mutations=[]
    let budget={revision:2,organisation_concurrent_runs:4,user_concurrent_runs:2}
    const reservations=[row,{...row,id:remoteId,run_id:remoteId,target:{kind:'personal_device',device_id:remoteId}}]
    page.on('pageerror',error=>errors.push(error.message))
    await page.addInitScript(()=>localStorage.setItem('open-cowork-browser-session-v1','active'))
    await page.route('**/*',async route=>{
      const request=route.request(), url=new URL(request.url()), path=url.pathname
      if(url.origin!==origin) {errors.push('Unexpected external request');await route.abort();return}
      const reply=(body,status=200)=>route.fulfill({status,contentType:'application/json',body:JSON.stringify(body)})
      if(path==='/api/v1/auth/browser/refresh')return reply({schema_version:2,access_token:'capacity-ui-fixture-not-a-real-token',
        access_expires_at:new Date(Date.now()+3600000).toISOString(),refresh_expires_at:new Date(Date.now()+86400000).toISOString(),user_id:org,session_id:org})
      if(path==='/api/v1/version')return reply({error:'not_found',message:'management server',details:{}},404)
      if(path==='/api/v1/auth/assurance')return reply({required:false,totp_enabled:false,passkey_registered:false,session_verified:false,enrollment_allowed:true,reauthentication_required:false})
      if(path==='/api/v1/auth/oidc/config')return reply({schema_version:2,enabled:false})
      if(path==='/management/v1/organisations')return reply({schema_version:3,platform_admin:false,execution_admission_enabled:true,items:[{id:org,name:'Capacity UI fixture',policy,role,created_at:row.created_at}]})
      const base='/management/v1/organisations/'+org
      if(path===base+'/instances')return reply([{...instance,can_manage:role==='admin'}])
      if(path===base+'/execution-capacity') {
        if(request.method()==='PUT') {
          const value=request.postDataJSON();mutations.push({path,body:value})
          assert.equal(value.expected_revision,budget.revision)
          budget={revision:budget.revision+1,organisation_concurrent_runs:value.organisation_concurrent_runs,user_concurrent_runs:value.user_concurrent_runs}
          return reply(budget)
        }
        return reply({organisation_id:org,budget,held_runs:reservations.length,reservations})
      }
      if(path===base+'/instances/'+instanceId+'/operations' && request.method()==='POST') {
        mutations.push({path,body:request.postDataJSON()});return reply({id:operationId,state:'queued'})
      }
      if(path.startsWith('/api/') || path.startsWith('/management/v1/')) {errors.push('Unexpected fixture endpoint: '+path);return reply({message:'unconfigured fixture'},404)}
      return route.continue()
    })
    await page.goto(origin+'/management')
    await page.getByRole('button',{name:'Task capacity',exact:true}).click()
    await page.getByText('Reserved task slots:').waitFor()
    const review=page.getByRole('button',{name:'Review server slots: Server capacity fixture (1)',exact:true})
    if(role==='admin') {
      await page.getByLabel('Task limit per member',{exact:true}).fill('1')
      await page.getByRole('button',{name:'Save task limits',exact:true}).click()
      await page.getByText('Task limits saved.',{exact:true}).waitFor()
      assert.deepEqual(mutations[0].body,{expected_revision:2,organisation_concurrent_runs:4,user_concurrent_runs:1})
      await review.click()
      const stop=page.getByRole('button',{name:'Stop and verify',exact:true})
      assert.equal(await stop.isEnabled(),false)
      await page.getByLabel('I acknowledge stopping the entire instance.',{exact:true}).check()
      assert.equal(await stop.isEnabled(),true)
      await page.screenshot({path:fileURLToPath(new URL(`execution-capacity-${name}.png`,output)),fullPage:true})
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1),true,'horizontal viewport overflow')
      await stop.click()
      await page.getByText('Stop verification requested. Operation: '+operationId,{exact:true}).waitFor()
      assert.equal(mutations.length,2)
      assert.deepEqual({...mutations[1].body,idempotency_key:'uuid'}, {action:'stop',expected_revision:8,idempotency_key:'uuid',reconcile_reservation_ids:[reservationId],acknowledge_execution_stop:true})
      assert.match(mutations[1].body.idempotency_key,/^[a-f0-9-]{36}$/)
    } else {
      assert.equal(await page.getByLabel('Organisation task limit',{exact:true}).isEnabled(),false)
      assert.equal(await review.count(),0)
      assert.equal(mutations.length,0)
    }
    assert.deepEqual(errors,[])
    checks.push(name+'_capacity_review_client_contract')
    await page.close()
  }
  await writeFile(new URL('execution-capacity-ui-results.json',output),JSON.stringify({checked_at:new Date().toISOString(),checks,api:'mocked',live_modified:false},null,2))
  console.log('capacity_browser_desktop_mobile_auditor_contract=ok (mocked API)')
} finally {
  await browser.close()
  await new Promise((resolve,reject)=>server.httpServer.close(error=>error?reject(error):resolve()))
}
