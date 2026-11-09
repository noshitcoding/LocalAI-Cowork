// Actual packaged extension and real tab DOM, with a deterministic native-peer
// fixture. Native stdio/OS transport is tested separately by the Rust suites.
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),http=require('node:http'),assert=require('node:assert/strict');
async function main(){
 const root=path.resolve(__dirname,'../..'),playwright=require(process.argv[2]),executable=process.argv[3];
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'cowork-extension-chrome-')),extension=path.join(temporary,'extension');fs.cpSync(path.join(root,'dist/browser-extensions/chrome'),extension,{recursive:true});
 const manifest=JSON.parse(fs.readFileSync(path.join(extension,'manifest.json')));manifest.host_permissions=['<all_urls>'];fs.writeFileSync(path.join(extension,'manifest.json'),JSON.stringify(manifest));
 const server=http.createServer((request,response)=>{response.writeHead(200,{'content-type':'text/html'});response.end('<title>Shared fixture</title><h1 id="result">Ready</h1><input id="entry"><input type="password" value="never-publish"><button id="save" onclick="document.querySelector(\'#result\').textContent=document.querySelector(\'#entry\').value;document.cookie=\'kept=yes;path=/\'">Save</button>');});
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));let context;
 try{
  context=await playwright.chromium.launchPersistentContext(path.join(temporary,'profile'),{executablePath:executable,headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`]});
  const worker=context.serviceWorkers()[0]??await context.waitForEvent('serviceworker');const id=new URL(worker.url()).hostname;
  await worker.evaluate(()=>{globalThis.__messages=[];const port={postMessage:value=>globalThis.__messages.push(value),disconnect:()=>{},onDisconnect:{addListener:()=>{}},onMessage:{addListener:fn=>globalThis.__emit=fn}};chrome.runtime.connectNative=()=>port;});
  const page=await context.newPage();await page.goto(`http://127.0.0.1:${server.address().port}/shared`);
  const hidden=await context.newPage();await hidden.goto(`http://127.0.0.1:${server.address().port}/unshared`);
  const popup=await context.newPage();await popup.goto(`chrome-extension://${id}/popup.html`);
  await popup.evaluate(async()=>{const tabs=await chrome.tabs.query({});const selected=tabs.find(t=>t.url?.endsWith('/shared'));await chrome.runtime.sendMessage({type:'connect',instance_id:'fixture',tabs:[selected.id]});});
  const connected=await worker.evaluate(()=>globalThis.__messages.find(v=>v.type==='connect'));assert.equal(connected.tabs.length,1);assert.ok(connected.tabs[0].url.endsWith('/shared'));
  await worker.evaluate(()=>globalThis.__emit({type:'connected',version:1,connection_id:'fixture'}));
  async function call(command){const request_id=crypto.randomUUID();await worker.evaluate(({command,request_id})=>globalThis.__emit({type:'command',version:1,request_id,epoch:'fixture-epoch',command}),{command,request_id});
   for(let attempt=0;attempt<200;attempt++){const result=await worker.evaluate(id=>globalThis.__messages.find(v=>v.type==='result'&&v.request_id===id),request_id);if(result)return result;await new Promise(resolve=>setTimeout(resolve,25));}throw Error('Extension action timeout');}
  const tab=connected.tabs[0];const scope={tab_id:tab.id,document_id:tab.document_id};
  const model=process.env.COWORK_BROWSER_TEST_MODEL_ACTIONS?JSON.parse(fs.readFileSync(process.env.COWORK_BROWSER_TEST_MODEL_ACTIONS,'utf8')):null;
  let result;for(const action of model?.actions??[{operation:'fill',selector:'#entry',text:'Extension works'},{operation:'click',selector:'#save'}]){result=await call({...scope,...action});assert.equal(result.ok,true);}
  assert.equal(await page.locator('#result').innerText(),model?.expected??'Extension works');assert.ok((await context.cookies()).some(c=>c.name==='kept'));
  result=await call({...scope,operation:'dom'});assert.equal(result.ok,true);assert.ok(!result.result.dom.includes('never-publish'));
  const privateTab=await popup.evaluate(async()=>{const tabs=await chrome.tabs.query({});return tabs.find(t=>t.url?.endsWith('/unshared')).id;});
  assert.equal((await call({...scope,tab_id:String(privateTab),operation:'dom'})).ok,false);
  assert.equal((await call({...scope,document_id:'stale',operation:'click',selector:'#save'})).ok,false);
  await popup.evaluate(()=>chrome.runtime.sendMessage({type:'disconnect'}));const state=await popup.evaluate(()=>chrome.runtime.sendMessage({type:'status'}));assert.equal(state.connected,false);assert.equal(state.grants.length,0);
  console.log(JSON.stringify({browser:'Chrome for Testing',native_peer:'fixture',model:model?.model??null,checks:['real_extension_loaded','existing_cookie_context','selected_tab_fill_click_dom','password_redaction','unshared_tab_rejected','stale_document_rejected','revoke_clears_grants']},null,2));
 }finally{if(context)await context.close();await new Promise(resolve=>server.close(resolve));if(path.dirname(path.resolve(temporary))!==path.resolve(os.tmpdir())||!path.basename(temporary).startsWith('cowork-extension-chrome-'))throw Error('Unexpected cleanup path');fs.rmSync(temporary,{recursive:true,force:true});}
}
main().catch(error=>{console.error(error);process.exitCode=1;});
