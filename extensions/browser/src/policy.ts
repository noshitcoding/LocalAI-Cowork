export const VERSION=1;
export const HOST="dev.localai.cowork.browser";
export const MAX_MESSAGE=4*1024*1024;
export const CHUNK=192*1024;
export function allowedUrl(value:unknown):value is string{
  if(typeof value!=="string"||value.length>8192)return false;
  try{const u=new URL(value);return ["http:","https:"].includes(u.protocol)&&!u.username&&!u.password;}catch{return false;}
}
export function grantedTab(tab:{id?:number,url?:string,incognito?:boolean},grants:Set<number>){return typeof tab.id==="number"&&grants.has(tab.id)&&!tab.incognito&&allowedUrl(tab.url);}
export function validateCommand(value:unknown):asserts value is Record<string,unknown>{
  if(!value||typeof value!=="object")throw Error("browser_invalid_command");
  const c=value as Record<string,unknown>;
  if(!["navigate","back","forward","reload","tabs","new_tab","close_tab","activate_tab","dom","fetch_text","screenshot","click","fill","select","scroll","search"].includes(String(c.operation)))throw Error("browser_capability_unavailable_use_native_browser");
  if(c.url!==null&&c.url!==undefined&&!allowedUrl(c.url))throw Error("browser_http_url_required");
  for(const [key,limit] of [["selector",1000],["text",32000]] as const){if(c[key]!=null&&(typeof c[key]!=="string"||(c[key] as string).length>limit))throw Error("browser_input_limit");}
}
