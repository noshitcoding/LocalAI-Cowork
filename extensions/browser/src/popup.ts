import {allowedUrl} from "./policy.js";
const api=(globalThis as any).browser??(globalThis as any).chrome;
const de=api.i18n.getUILanguage().startsWith("de"),tr=(a:string,b:string)=>de?a:b;
const status=document.querySelector<HTMLElement>("#status")!,instances=document.querySelector<HTMLSelectElement>("#instances")!,list=document.querySelector<HTMLElement>("#tabs")!;
let initialized=false;
document.querySelector("h1")!.textContent="LocalAI Cowork";
document.querySelector("#hint")!.textContent=tr("Verbinde eine laufende Cowork-App und wähle Tabs aus. Der Webseitenzugriff wird nur für freigegebene Tabs verwendet. Vorhandene Anmeldungen bleiben im Browser.","Connect a running Cowork app and select tabs. Website access is used only for shared tabs. Existing sign-ins stay in your browser.");
document.querySelector("#connect")!.textContent=tr("Ausgewählte Tabs verbinden","Connect selected tabs");
document.querySelector("#disconnect")!.textContent=tr("Zugriff beenden","End access");
document.querySelector("#refresh")!.textContent=tr("Apps suchen / reparieren","Find apps / repair");
async function refresh(){
  const state=await api.runtime.sendMessage({type:"status"});
  status.textContent=state.connected?tr("Verbunden · ","Connected · ")+state.grants.length+tr(" freigegebene Tabs"," shared tabs"):state.error?tr("Lokaler Host nicht erreichbar. Cowork öffnen und Browser-Verbindung einrichten oder reparieren.","Local host unavailable. Open Cowork and set up or repair the browser connection."):tr("Nicht verbunden. Cowork-Browserbereich öffnen und App suchen.","Disconnected. Open the Cowork browser workspace and find the app.");
  const selected=instances.value;instances.replaceChildren();
  for(const instance of state.instances){const option=document.createElement("option");option.value=instance.id;option.textContent=instance.label+" · "+instance.id.slice(0,8);instances.append(option);}if([...instances.options].some(o=>o.value===selected))instances.value=selected;
  (document.querySelector("#connect") as HTMLButtonElement).disabled=state.connected||!state.instances.length;
  if(!initialized){initialized=true;for(const tab of await api.tabs.query({currentWindow:true})){
    if(tab.incognito||!allowedUrl(tab.url))continue;
    const label=document.createElement("label"),input=document.createElement("input"),text=document.createElement("span");input.type="checkbox";input.value=String(tab.id);input.checked=state.grants.includes(tab.id)||(!state.connected&&tab.active);text.textContent=(tab.title||tab.url).slice(0,100);label.append(input,text);list.append(label);
  }}
  for(const input of list.querySelectorAll<HTMLInputElement>("input"))input.disabled=state.connected;
}
document.querySelector("#connect")!.addEventListener("click",()=>{void(async()=>{
  const tabs=[...list.querySelectorAll<HTMLInputElement>("input:checked")].map(i=>Number(i.value));if(!tabs.length)throw Error(tr("Mindestens einen Tab auswählen.","Select at least one tab."));
  // This call remains directly in the user's click handler before any await.
  const accepted=await api.permissions.request({origins:["<all_urls>"]});if(!accepted)throw Error(tr("Webseitenzugriff wurde nicht erteilt.","Website permission was not granted."));
  await api.runtime.sendMessage({type:"connect",instance_id:instances.value,tabs});await refresh();
})().catch(error=>{status.textContent=error instanceof Error?error.message:String(error);});});
document.querySelector("#disconnect")!.addEventListener("click",()=>{void api.runtime.sendMessage({type:"disconnect"}).then(refresh);});
document.querySelector("#refresh")!.addEventListener("click",()=>{void api.runtime.sendMessage({type:"discover"}).then(refresh).catch(()=>{status.textContent=tr("Cowork öffnen und den lokalen Browser-Host einrichten.","Open Cowork and set up the local browser host.");});});
void refresh();setInterval(()=>{void refresh().catch(()=>undefined);},1000);
