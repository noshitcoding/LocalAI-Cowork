import {toolboxEndpoint} from "./toolboxPending";
export type BrowserAnnotation={id:string,selector:string,text:string,url:string,recorded_at:number};
export const browserAnnotationsKey=(base:string,order:string)=>"cowork-v2:browser-annotations:v1:"+encodeURIComponent(JSON.stringify([toolboxEndpoint(base),order]));
export function mergeBrowserAnnotations(previous:BrowserAnnotation[],incoming:unknown):BrowserAnnotation[]{
 if(!Array.isArray(incoming)||incoming.length>500)throw Error("invalid_browser_annotations");const result=new Map(previous.map(item=>[item.id,item]));
 for(const raw of incoming){const value=raw as BrowserAnnotation;if(!value||typeof value.id!=="string"||typeof value.selector!=="string"||value.selector.length>1000||typeof value.text!=="string"||value.text.length>2000||typeof value.url!=="string"||value.url.length>8192||!Number.isFinite(value.recorded_at))throw Error("invalid_browser_annotations");result.set(value.id,value);}
 return [...result.values()].sort((a,b)=>a.recorded_at-b.recorded_at).slice(-500);
}
export const annotationPrompt=(items:BrowserAnnotation[])=>items.map(item=>`${item.url}\n${item.selector}\n${item.text}`).join("\n\n");
