// Optional explicitly configured Node Playwright runtime. The existing supervisor
// owns this process and its browser tree; IPC never opens a debugging TCP port.
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const directory=fs.realpathSync(process.argv[2]),config=JSON.parse(fs.readFileSync(path.join(directory,'config.json'),'utf8'));
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms)),limit=2*1024*1024;
function publish(name,value){let raw=JSON.stringify(value);if(Buffer.byteLength(raw)>limit)raw=JSON.stringify({error:'Persistent tool output exceeds its limit'});fs.writeFileSync(path.join(directory,name+'.tmp'),raw,{flag:'wx'});fs.renameSync(path.join(directory,name+'.tmp'),path.join(directory,name));}
function publicUrl(value){try{const u=new URL(value);return u.origin+u.pathname;}catch{return '';}}
const sensitive=/authorization|cookie|api[-_]?key|token|secret/i;
function headers(input){return Object.fromEntries(Object.entries(input).map(([key,value])=>[key,sensitive.test(key)?'[redacted]':String(value).slice(0,4096)]));}
async function main(){
 const {chromium}=require(config.playwright_module);
 const context=await chromium.launchPersistentContext(config.profile,{headless:true,viewport:{width:1440,height:960},timeout:45000,...(config.executable?{executablePath:config.executable}:{})});
 const page=context.pages()[0]??await context.newPage(),consoleEntries=[],network=[],responses=new Map(),annotations=new Map();
 const retain=(list,value)=>{list.push(value);if(list.length>200)list.shift();};
 page.on('console',message=>retain(consoleEntries,{type:message.type(),text:message.text().slice(0,4096),url:publicUrl(message.location().url),line:message.location().lineNumber,recorded_at:Date.now()}));
 page.on('pageerror',error=>retain(consoleEntries,{type:'pageerror',text:String(error).slice(0,4096),recorded_at:Date.now()}));
 page.on('response',response=>{const id=crypto.randomUUID();retain(network,{id,method:response.request().method(),url:publicUrl(response.url()),status:response.status(),resource_type:response.request().resourceType(),recorded_at:Date.now()});responses.set(id,response);if(responses.size>200)responses.delete(responses.keys().next().value);});
 page.on('requestfailed',request=>retain(network,{id:crypto.randomUUID(),method:request.method(),url:publicUrl(request.url()),failure:request.failure()?.errorText?.slice(0,1000),recorded_at:Date.now()}));
 async function dispatch(action,seconds){
  const operation=action.operation,timeout=Math.min(seconds*1000,45000);let details=null,shot=null,text=null;
  if(operation==='search'){if(typeof action.text!=='string'||!action.text||action.text.length>8000)throw Error('Invalid search query');const u=new URL('https://html.duckduckgo.com/html/');u.searchParams.set('q',action.text);action={...action,url:u.href};}
  if(['navigate','fetch_text','search'].includes(operation)){const u=new URL(action.url);if(!['http:','https:'].includes(u.protocol)||u.username||u.password)throw Error('Expected HTTP(S) URL without credentials');const response=await page.goto(u.href,{waitUntil:'domcontentloaded',timeout});
   if(operation==='fetch_text')text=`URL: ${page.url()}\nStatus: ${response?.status()??0}\nTitle: ${await page.title()}\n\n`+(await page.locator('body').innerText({timeout})).slice(0,4000);
   else if(operation==='search'){if(!response?.ok())throw Error('Search returned an unsuccessful HTTP status');const results=await page.evaluate(()=>Array.from(document.querySelectorAll('.result')).slice(0,5).map(item=>({title:item.querySelector('.result__a')?.textContent??'',url:item.querySelector('.result__a')?.href??'',snippet:item.querySelector('.result__snippet')?.textContent??''})));if(!results.length)throw Error('Search returned no recognizable results; provider may require a challenge');text=results.map((item,index)=>{const u=new URL(item.url);return `${index+1}. ${item.title.slice(0,1000)}\n${(u.searchParams.get('uddg')??item.url).slice(0,8192)}\n${item.snippet.slice(0,1000)}`;}).join('\n\n');}
  }
  else if(page.url()==='about:blank')throw Error('Navigate this session first');
  else if(operation==='click')await page.locator(action.selector).click({timeout});
  else if(operation==='fill')await page.locator(action.selector).fill(action.text,{timeout});
  else if(operation==='annotate'){
   if(typeof action.selector!=='string'||action.selector.length>1000||typeof action.text!=='string'||!action.text||action.text.length>2000)throw Error('Invalid annotation');
   const target=page.locator(action.selector);if(await target.count()!==1)throw Error('Annotation requires one exact element');const id=crypto.randomUUID();
   await target.evaluate((element,note)=>{const box=document.createElement('span');box.textContent=note.text;box.dataset.coworkAnnotation=note.id;box.style.cssText='position:absolute;z-index:2147483647;background:#fff3a3;color:#171717;padding:4px;border:2px solid #a74a00;max-width:400px;pointer-events:none;white-space:pre-wrap';const r=element.getBoundingClientRect();box.style.left=(r.left+scrollX)+'px';box.style.top=(r.top+scrollY)+'px';document.documentElement.appendChild(box);},{id,text:action.text});
   annotations.set(id,{id,selector:action.selector,text:action.text,url:publicUrl(page.url()),recorded_at:Date.now()});
  }else if(operation==='clear_annotations'){await page.locator('[data-cowork-annotation]').evaluateAll(elements=>elements.forEach(element=>element.remove()));annotations.clear();}
  else if(operation==='network_details'){
   const response=responses.get(action.selector);if(!response)throw Error('Network entry is no longer retained');const all=await response.allHeaders();
   details={id:action.selector,url:publicUrl(response.url()),status:response.status(),request_headers:headers(await response.request().allHeaders()),response_headers:headers(all)};
   if(/text\/|json|javascript|xml/.test(all['content-type']??'')&&/^\d+$/.test(all['content-length']??'')&&Number(all['content-length'])<=1024*1024){const raw=await response.body();details.body=raw.subarray(0,65536).toString('utf8');details.body_truncated=raw.length>65536;}
  }else if(!['dom','screenshot','console','network','annotations'].includes(operation))throw Error('Unknown browser operation');
  if(operation==='screenshot'){shot='browser-'+config.session_id+'.png';await page.screenshot({path:path.join(config.workspace,shot),fullPage:false,timeout});}
  if(['fetch_text','search'].includes(operation)){const raw=Buffer.from(text??'','utf8');let result={session_id:config.session_id,url:page.url().slice(0,2048),title:(await page.title()).slice(0,500),backend:'playwright_persistent_node',context_epoch:config.epoch,text:new TextDecoder('utf-8').decode(raw.subarray(0,14000),{stream:true}),text_truncated:raw.length>14000};while(Buffer.byteLength(JSON.stringify(result))>23000){result.text=Array.from(result.text).slice(0,Math.floor(Array.from(result.text).length*.75)).join('');result.text_truncated=true;}return result;}
  return {session_id:config.session_id,url:page.url(),title:await page.title(),backend:'playwright_persistent_node',context_epoch:config.epoch,dom:['navigate','dom','click','fill'].includes(operation)?await page.evaluate(()=>document.documentElement.outerHTML.slice(0,1048576)):null,screenshot_path:shot,text,console:operation==='console'?consoleEntries:null,network:operation==='network'?network:null,network_details:details,annotations:['annotations','annotate','clear_annotations'].includes(operation)?Array.from(annotations.values()):null};
 }
 publish('ready.json',{ready:true});let last=Date.now();
 try{while(Date.now()-last<900000){const pending=fs.readdirSync(directory).filter(name=>name.endsWith('.request.json')).sort();if(!pending.length){await sleep(25);continue;}for(const name of pending){last=Date.now();const working=path.join(directory,name+'.working');fs.renameSync(path.join(directory,name),working);let reply;try{if(fs.statSync(working).size>limit)throw Error('Oversized request');const request=JSON.parse(fs.readFileSync(working,'utf8'));reply={result:await dispatch(request.body,request.timeout_seconds)};}catch(error){reply={error:String(error).slice(0,4096)};}publish(name.replace('.request.json','.response.json'),reply);fs.unlinkSync(working);}}}finally{await context.close();}
}
main().catch(error=>{process.stderr.write(String(error));process.exitCode=1;});
