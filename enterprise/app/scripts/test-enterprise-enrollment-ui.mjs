// Render/client contract test only; API responses here are fixtures.
import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from '@playwright/test'
import { preview } from 'vite'

const output=new URL('../../.internal/enterprise-local/',import.meta.url)
const org='11111111-1111-4111-8111-111111111111',instance='22222222-2222-4222-8222-222222222222',operation='33333333-3333-4333-8333-333333333333'
const policy={revision:1,extensions:'catalog',content_access:'support_grant',minimum_isolation:'container',allow_laptop_full_access:false,
  allow_offline_continuation:true,allowed_network_origins:[],limits:{cpu_millis:4000,memory_mib:8192,storage_gib:50,concurrent_runs:4,instances:10},audit_retention_days:365,content_logging:false}
const server=await preview({mode:'web',preview:{host:'127.0.0.1',port:0,strictPort:true}})
const origin=`http://127.0.0.1:${server.httpServer.address().port}`
const browser=await chromium.launch({channel:'msedge',headless:true}),checks=[]
try {
  for(const [name,width,height,role] of [['desktop',1440,1000,'admin'],['mobile',390,844,'admin'],['auditor',1440,1000,'auditor']]) {
    const page=await browser.newPage({viewport:{width,height},locale:'en-US'}),errors=[],mutations=[]
    page.on('pageerror',error=>errors.push(error.message))
    await page.addInitScript(()=>localStorage.setItem('open-cowork-browser-session-v1','active'))
    await page.route('**/*',async route=>{
      const req=route.request(),url=new URL(req.url()),path=url.pathname
      if(url.origin!==origin){errors.push('external request');return route.abort()}
      const reply=(body,status=200)=>route.fulfill({status,contentType:'application/json',body:JSON.stringify(body)})
      if(path==='/api/v1/auth/browser/refresh')return reply({schema_version:2,access_token:'enrollment-ui-fixture-not-a-real-access-token',access_expires_at:new Date(Date.now()+3600000).toISOString(),refresh_expires_at:new Date(Date.now()+86400000).toISOString(),user_id:org,session_id:org})
      if(path==='/api/v1/version')return reply({error:'not_found',message:'management server',details:{}},404)
      if(path==='/api/v1/auth/assurance')return reply({required:false,totp_enabled:false,passkey_registered:false,session_verified:false,enrollment_allowed:true,reauthentication_required:false})
      if(path==='/api/v1/auth/oidc/config')return reply({schema_version:2,enabled:false})
      if(path==='/management/v1/organisations')return reply({schema_version:3,platform_admin:false,execution_admission_enabled:false,items:[{id:org,name:'Enrollment fixture',policy,role,created_at:'2026-09-27T00:00:00Z'}]})
      const base='/management/v1/organisations/'+org
      if(path===base+'/instances')return reply([{id:instance,organisation_id:org,owner_user_id:org,name:'Existing workspace',backend:'compose',state:'stopped',policy,revision:8+mutations.length,created_at:'2026-09-27T00:00:00Z',execution_required:false,can_manage:role==='admin',available_actions:mutations.length?[]:['enroll_execution']}])
      if(path===base+'/instances/'+instance+'/operations') {
        if(req.method()==='POST'){mutations.push({path,body:req.postDataJSON()});return reply({id:operation,state:'queued'})}
        return reply([{id:operation,action:'enroll_execution',state:'reconciling',created_at:'2026-09-27T00:00:00Z'}])
      }
      if(path===base+'/instances/'+instance+'/operations/'+operation+'/recover'){
        mutations.push({path,body:req.postDataJSON()});return reply({id:operation,state:'reconciling'})
      }
      if(path.startsWith('/api/')||path.startsWith('/management/v1/')){errors.push('unknown endpoint: '+path);return reply({},404)}
      return route.continue()
    })
    await page.goto(origin+'/management')
    try { await page.getByRole('heading',{name:'Existing workspace',exact:true}).waitFor() }
    catch(error) {
      await page.screenshot({path:fileURLToPath(new URL('execution-enrollment-ui-failure.png',output)),fullPage:true})
      console.log(JSON.stringify({errors,body:await page.locator('body').innerText()}));throw error
    }
    if(role==='admin') {
      await page.getByRole('button',{name:'Enroll in central limits',exact:true}).click()
      const confirm=page.getByRole('button',{name:'Confirm operation',exact:true})
      assert.equal(await confirm.isEnabled(),false)
      await page.getByLabel('I authorize this enrollment review and understand that the instance remains stopped.',{exact:true}).check()
      await page.screenshot({path:fileURLToPath(new URL(`execution-enrollment-${name}.png`,output)),fullPage:true})
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1),true)
      await confirm.click()
      await page.getByRole('button',{name:'Review interrupted operation',exact:true}).click()
      const recover=page.getByRole('button',{name:'Continue enrollment review',exact:true})
      assert.equal(await recover.isEnabled(),false)
      await page.getByLabel('I authorize this enrollment review and understand that the instance remains stopped.',{exact:true}).check()
      await recover.click()
      await page.waitForFunction(()=>!document.querySelector('form[aria-label="Recover interrupted operation"]'))
      assert.equal(mutations.length,2)
      assert.deepEqual({...mutations[0].body,idempotency_key:'uuid'},{action:'enroll_execution',expected_revision:8,idempotency_key:'uuid',backup_id:null,release:null,acknowledge_execution_enrollment:true})
      assert.deepEqual(mutations[1].body,{mode:'continue_enrollment'})
    } else {
      assert.equal(await page.getByRole('button',{name:'Enroll in central limits',exact:true}).count(),0)
      await page.getByRole('button',{name:'Operations',exact:true}).click()
      await page.getByRole('cell',{name:'reconciling',exact:true}).waitFor()
      assert.equal(await page.getByRole('button',{name:'Review interrupted operation',exact:true}).count(),0)
      assert.equal(mutations.length,0)
    }
    assert.deepEqual(errors,[]);checks.push(name+'_enrollment_acknowledgement_and_recovery');await page.close()
  }
  await writeFile(new URL('execution-enrollment-ui-results.json',output),JSON.stringify({checked_at:new Date().toISOString(),checks,api:'mocked',live_modified:false},null,2))
  console.log('enrollment_browser_desktop_mobile_auditor_contract=ok (mocked API)')
}finally{await browser.close();await new Promise((resolve,reject)=>server.httpServer.close(error=>error?reject(error):resolve()))}
