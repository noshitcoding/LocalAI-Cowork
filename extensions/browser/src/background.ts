import {HOST,VERSION,CHUNK,MAX_MESSAGE,allowedUrl,grantedTab,validateCommand} from "./policy.js";
const api=(globalThis as any).browser??(globalThis as any).chrome;
type Tab={id:number,url:string,title?:string,active:boolean,incognito?:boolean,windowId:number};
let native:any=null,connected=false,connectionId="",instanceList:any[]=[],lastError="";
const grants=new Set<number>(),documents=new Map<number,string>();
let chunks:{id:string,total:number,next:number,raw:number[],updated:number}|undefined;
let busy=false;
const engine=api.runtime.getManifest().browser_specific_settings?"firefox":"chrome";

function encode(value:unknown){const raw=new TextEncoder().encode(JSON.stringify(value));if(raw.length>MAX_MESSAGE)throw Error("browser_message_limit");return raw;}
function send(value:unknown){
  if(!native)throw Error("browser_not_connected");const raw=encode(value);
  if(raw.length<=CHUNK){native.postMessage(value);return;}
  const id=crypto.randomUUID(),total=Math.ceil(raw.length/CHUNK);
  for(let index=0;index<total;index++){let binary="";for(const byte of raw.subarray(index*CHUNK,(index+1)*CHUNK))binary+=String.fromCharCode(byte);native.postMessage({type:"chunk",id,index,total,data:btoa(binary)});}
}
function receive(value:any){
  if(value?.type!=="chunk")return value;
  if(!chunks||Date.now()-chunks.updated>10000){if(value.index!==0)throw Error("browser_chunk_order");chunks={id:value.id,total:value.total,next:0,raw:[],updated:Date.now()};}
  if(value.id!==chunks.id||value.index!==chunks.next||value.total!==chunks.total||!Number.isInteger(value.total)||value.total<1||value.total>32||typeof value.data!=="string"||value.data.length>CHUNK*2)throw Error("browser_chunk_order");
  for(const char of atob(value.data))chunks.raw.push(char.charCodeAt(0));
  if(chunks.raw.length>MAX_MESSAGE)throw Error("browser_message_limit");chunks.next++;chunks.updated=Date.now();
  if(chunks.next!==chunks.total)return null;
  const raw=new Uint8Array(chunks.raw);chunks=undefined;return JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(raw));
}
async function sharedTabs(){
  const all:Tab[]=await api.tabs.query({});const output=[];
  for(const tab of all){if(grantedTab(tab,grants)){if(!documents.has(tab.id))documents.set(tab.id,crypto.randomUUID());output.push({id:String(tab.id),document_id:documents.get(tab.id),url:tab.url,title:(tab.title??"").slice(0,1000),active:tab.active});}}
  return output;
}
async function refreshGrants(){if(connected)send({type:"grants",version:VERSION,tabs:await sharedTabs()});await updateBadge();}
async function updateBadge(){await api.action.setBadgeText({text:connected?String(grants.size):""});await api.action.setBadgeBackgroundColor({color:"#226b45"});}
function disconnect(){const port=native;native=null;connected=false;connectionId="";grants.clear();documents.clear();chunks=undefined;busy=false;if(port)port.disconnect();void updateBadge();}
function ensureNative(){
  if(native)return;
  lastError="";native=api.runtime.connectNative(HOST);const port=native;
  native.onDisconnect.addListener(()=>{if(native!==port)return;lastError="browser_host_unavailable_or_disconnected";native=null;connected=false;connectionId="";grants.clear();documents.clear();chunks=undefined;void updateBadge();});
  native.onMessage.addListener((raw:any)=>{void (async()=>{
    if(native!==port)return;
    const value=receive(raw);if(!value)return;
    if(value.version!==VERSION)throw Error("browser_protocol_mismatch");
    if(value.type==="instances"){instanceList=value.instances;return;}
    if(value.type==="connected"){connected=true;connectionId=value.connection_id;await updateBadge();return;}
    if(value.type!=="command"||!connected)throw Error("browser_invalid_message");
    if(busy)throw Error("browser_busy");busy=true;
    try{
      validateCommand(value.command);const result=await execute(value.command);
      send({type:"result",version:VERSION,request_id:value.request_id,epoch:value.epoch,ok:true,result:{...result,context_epoch:value.epoch}});
    }catch{
      // Never include browser exception strings, URLs or field values in logs.
      send({type:"result",version:VERSION,request_id:value.request_id,epoch:value.epoch,ok:false,error:"browser_action_failed_or_uncertain"});
    }finally{busy=false;}
  })().catch(()=>{lastError="browser_protocol_error";disconnect();});});
}
async function checked(id:number,documentId?:string){
  const tab:Tab=await api.tabs.get(id);
  if(!connected||!grantedTab(tab,grants))throw Error("browser_tab_not_granted");
  if(documentId&&documents.get(id)!==documentId)throw Error("browser_document_changed");return tab;
}
async function execute(c:Record<string,unknown>):Promise<Record<string,unknown>>{
  if(c.operation==="tabs")return {tabs:await sharedTabs()};
  if(c.operation==="new_tab"){
    const url=c.url??"https://example.com/";if(!allowedUrl(url))throw Error("browser_url");
    const tab:Tab=await api.tabs.create({url,active:true});grants.add(tab.id);documents.set(tab.id,crypto.randomUUID());await refreshGrants();return {tab_id:String(tab.id),tabs:await sharedTabs()};
  }
  const id=Number(c.tab_id);if(!Number.isSafeInteger(id))throw Error("browser_tab_id");
  const tab=await checked(id,String(c.document_id));
  switch(c.operation){
    case "navigate":if(!allowedUrl(c.url))throw Error("browser_url");await api.tabs.update(id,{url:c.url});break;
    case "search":await api.tabs.update(id,{url:"https://html.duckduckgo.com/html/?q="+encodeURIComponent(String(c.text??"").slice(0,8000))});break;
    case "back":await api.tabs.goBack(id);break;
    case "forward":await api.tabs.goForward(id);break;
    case "reload":await api.tabs.reload(id);break;
    case "close_tab":grants.delete(id);documents.delete(id);await api.tabs.remove(id);break;
    case "activate_tab":await api.tabs.update(id,{active:true});break;
    case "screenshot":{
      if(!tab.active)throw Error("browser_activate_selected_tab_first");
      const before:Tab[] = await api.tabs.query({active:true,windowId:tab.windowId});
      if(before[0]?.id!==id)throw Error("browser_active_tab_changed");
      const image:string=await api.tabs.captureVisibleTab(tab.windowId,{format:"jpeg",quality:65});
      const after:Tab[]=await api.tabs.query({active:true,windowId:tab.windowId});await checked(id,String(c.document_id));
      if(after[0]?.id!==id)throw Error("browser_active_tab_changed");
      if(image.length>MAX_MESSAGE/2)throw Error("browser_image_limit");return {image,tabs:await sharedTabs()};
    }
    default:{
      const result=await api.scripting.executeScript({target:{tabId:id,frameIds:[0]},world:"ISOLATED",func:pageAction,args:[c]});
      await checked(id,String(c.document_id));
      if(result.length!==1||!result[0].result?.ok)throw Error("browser_page_action_failed");
      return {...result[0].result,tabs:await sharedTabs(),tab_id:String(id),document_id:documents.get(id)};
    }
  }
  await refreshGrants();return {tabs:await sharedTabs()};
}
// This self-contained function is serialized by scripting.executeScript. It
// exposes only fixed DOM operations, never eval or a page-to-native message API.
function pageAction(c:Record<string,unknown>){
  const operation=c.operation;
  if(!["http:","https:"].includes(location.protocol))return {ok:false};
  if(operation==="dom"){
    const root=document.documentElement.cloneNode(true) as HTMLElement;
    root.querySelectorAll("script").forEach(e=>e.remove());
    root.querySelectorAll("input").forEach(e=>{if(e.type==="password"||e.type==="hidden"||/token|secret|credential|api.?key/i.test(e.name)){e.removeAttribute("value");e.value="";}});
    return {ok:true,dom:root.outerHTML.slice(0,500000),url:location.href,title:document.title};
  }
  if(operation==="fetch_text")return {ok:true,text:document.body.innerText.slice(0,14000),url:location.href,title:document.title};
  if(operation==="scroll"){window.scrollBy(Number(c.x??0),Number(c.y??0));return {ok:true};}
  const items=document.querySelectorAll(String(c.selector??""));if(items.length!==1)return {ok:false};
  const target=items[0];
  if(operation==="click"&&target instanceof HTMLElement){target.click();return {ok:true};}
  if(operation==="select"&&target instanceof HTMLSelectElement){target.value=String(c.text??"");target.dispatchEvent(new Event("input",{bubbles:true}));target.dispatchEvent(new Event("change",{bubbles:true}));return {ok:true};}
  if(operation==="fill"&&(target instanceof HTMLInputElement||target instanceof HTMLTextAreaElement)){
    if(target instanceof HTMLInputElement&&["password","file","hidden"].includes(target.type))return {ok:false};
    const prototype=target instanceof HTMLInputElement?HTMLInputElement.prototype:HTMLTextAreaElement.prototype;
    Object.getOwnPropertyDescriptor(prototype,"value")!.set!.call(target,String(c.text??""));
    target.dispatchEvent(new Event("input",{bubbles:true}));target.dispatchEvent(new Event("change",{bubbles:true}));return {ok:true};
  }return {ok:false};
}
api.tabs.onUpdated.addListener((id:number,change:any)=>{if(grants.has(id)){if(change.status==="loading"||change.url)documents.set(id,crypto.randomUUID());void refreshGrants();}});
api.tabs.onRemoved.addListener((id:number)=>{grants.delete(id);documents.delete(id);void refreshGrants();});
api.tabs.onActivated.addListener(()=>{void refreshGrants();});
api.permissions.onRemoved.addListener(()=>{disconnect();});
api.runtime.onMessage.addListener((message:any,sender:any)=>{
  if(sender.id!==api.runtime.id||(sender.tab&&!(typeof sender.url==="string"&&sender.url===api.runtime.getURL?.("popup.html"))))return Promise.reject(Error("browser_popup_only"));
  return (async()=>{
    if(message.type==="discover"){ensureNative();send({type:"instances",version:VERSION});}
    else if(message.type==="connect"){
      if(connected)throw Error("browser_disconnect_first");ensureNative();
      if(!Array.isArray(message.tabs)||message.tabs.length>64)throw Error("browser_tab_limit");
      grants.clear();documents.clear();
      for(const id of message.tabs){const tab:Tab=await api.tabs.get(id);if(tab.incognito||!allowedUrl(tab.url))throw Error("browser_tab_not_allowed");grants.add(id);documents.set(id,crypto.randomUUID());}
      send({type:"connect",version:VERSION,instance_id:message.instance_id,engine,label:engine==="firefox"?"Firefox":"Chrome",tabs:await sharedTabs()});
    }else if(message.type==="disconnect"){if(connected)send({type:"revoke",version:VERSION});disconnect();}
    else if(message.type!=="status")throw Error("browser_popup_action");
    return {connected,connection_id:connectionId,instances:instanceList,error:lastError,grants:[...grants]};
  })();
});
