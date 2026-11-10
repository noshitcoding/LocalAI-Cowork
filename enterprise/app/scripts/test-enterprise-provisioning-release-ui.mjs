// Browser rendering and wire checks with mocked API responses, never live fleet mutations.
import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from '@playwright/test'
import { preview } from 'vite'

const output=new URL('../../.internal/enterprise-local/',import.meta.url)
const org='11111111-1111-4111-8111-111111111111',instance='22222222-2222-4222-8222-222222222222',operation='33333333-3333-4333-8333-333333333333'
const release={id:'a'.repeat(64),name:'Enterprise: approved initial version for a shared team workspace'}
const policy={revision:1,extensions:'catalog',content_access:'support_grant',minimum_isolation:'container',allow_laptop_full_access:false,
  allow_offline_continuation:true,allowed_network_origins:[],limits:{cpu_millis:4000,memory_mib:8192,storage_gib:50,concurrent_runs:4,instances:10},audit_retention_days:365,content_logging:false}
const server=await preview({mode:'web',preview:{host:'127.0.0.1',port:0,strictPort:true}})
const origin=`http://127.0.0.1:${server.httpServer.address().port}`
const browser=await chromium.launch({channel:'msedge',headless:true}),checks=[]
try {
  for(const [name,width,height,role,catalogError,enabled] of [
    ['desktop',1440,1000,'admin',false,true],['mobile',390,844,'admin',false,true],
    ['unavailable',1440,1000,'admin',true,true],['auditor',1440,1000,'auditor',false,true],['legacy',1440,1000,'admin',false,false]]) {
    const page=await browser.newPage({viewport:{width,height},locale:'en-US'}),errors=[],mutations=[]
    let created=false
    page.on('pageerror',error=>errors.push(error.message))
    await page.addInitScript(()=>localStorage.setItem('open-cowork-browser-session-v1','active'))
    await page.route('**/*',async route=>{
      const req=route.request(),url=new URL(req.url()),path=url.pathname
      if(url.origin!==origin){errors.push('external request');return route.abort()}
      const reply=(body,status=200)=>route.fulfill({status,contentType:'application/json',body:JSON.stringify(body)})
      if(path==='/api/v1/auth/browser/refresh')return reply({schema_version:2,access_token:'release-ui-fixture-not-a-real-token',access_expires_at:new Date(Date.now()+3600000).toISOString(),refresh_expires_at:new Date(Date.now()+86400000).toISOString(),user_id:org,session_id:org})
      if(path==='/api/v1/version')return reply({error:'not_found',message:'management server',details:{}},404)
      if(path==='/api/v1/auth/assurance')return reply({required:false,totp_enabled:false,passkey_registered:false,session_verified:false,enrollment_allowed:true,reauthentication_required:false})
      if(path==='/api/v1/auth/oidc/config')return reply({schema_version:2,enabled:false})
      if(path==='/management/v1/organisations')return reply({schema_version:3,platform_admin:false,release_selection_enabled:enabled,items:[{id:org,name:'Initial release fixture',policy,role,created_at:'2026-09-28T00:00:00Z'}]})
      const base='/management/v1/organisations/'+org
      if(path===base+'/provisioning-releases')return catalogError?reply({message:'Catalog temporarily unavailable'},503):reply([release])
      if(path===base+'/instances') {
        if(req.method()==='POST'){mutations.push(req.postDataJSON());created=true;return reply({id:instance,state:'pending'})}
        return reply(created?[{id:instance,organisation_id:org,owner_user_id:org,name:'New workspace',backend:'compose',state:'pending',policy,revision:1,created_at:'2026-09-28T00:00:00Z',provisioning_release:release.id,can_manage:true,available_actions:['provision']}]:[])
      }
      if(path===base+'/instances/'+instance+'/operations' && req.method()==='POST') {mutations.push(req.postDataJSON());return reply({id:operation,state:'queued'})}
      if(path.startsWith('/api/')||path.startsWith('/management/v1/')){errors.push('unknown endpoint: '+path);return reply({},404)}
      return route.continue()
    })
    await page.goto(origin+'/management')
    await page.getByRole('heading',{name:'Cowork instances',exact:true}).waitFor()
    if(role==='auditor') assert.equal(await page.getByRole('button',{name:'Create instance',exact:true}).count(),0)
    else if(!enabled) assert.equal(await page.getByLabel('Initial version',{exact:true}).count(),0)
    else {
      const create=page.getByRole('button',{name:'Create instance',exact:true})
      await page.getByLabel('Instance name',{exact:true}).fill('New workspace')
      assert.equal(await create.isEnabled(),false)
      if(catalogError) await page.getByText('Error: Catalog temporarily unavailable',{exact:true}).waitFor()
      else {
        const select=page.getByLabel('Initial version',{exact:true})
        await select.selectOption(release.id)
        assert.equal(await create.isEnabled(),true)
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),true,'page overflow')
        await page.screenshot({path:fileURLToPath(new URL(`provisioning-release-${name}.png`,output)),fullPage:true})
        const provisioned=page.waitForResponse(response=>response.url().endsWith(`/instances/${instance}/operations`) && response.request().method()==='POST')
        await create.click()
        await provisioned
        await page.getByRole('heading',{name:'New workspace',exact:true}).waitFor()
        assert.deepEqual(mutations[0],{name:'New workspace',backend:'compose',release:release.id})
        assert.equal(mutations[1]?.action,'provision')
      }
    }
    assert.deepEqual(errors,[]);checks.push(name);await page.close()
  }
  await writeFile(new URL('provisioning-release-ui-results.json',output),JSON.stringify({checked_at:new Date().toISOString(),checks,api:'mocked',live_modified:false},null,2))
  console.log('initial_release_browser_contract=passed (mocked API)')
}finally{await browser.close();await new Promise((resolve,reject)=>server.httpServer.close(error=>error?reject(error):resolve()))}
