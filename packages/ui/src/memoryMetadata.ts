export type MemoryScope={kind:"user"}|{kind:"project"}|{kind:"chat";order_id:string}|{kind:"agent";agent_id:string};
export type MemoryMetadata={scope:MemoryScope;category:string;confidence:number|null;profile_key:string|null;enabled:boolean};
export const defaultMemoryMetadata=():MemoryMetadata=>({scope:{kind:"project"},category:"general",confidence:null,profile_key:null,enabled:true});
export function validateMemoryMetadata(raw:unknown):MemoryMetadata {
 if(!raw||typeof raw!=="object"||Array.isArray(raw))throw new Error("invalid_memory_metadata");const v=raw as Record<string,unknown>;
 if(Object.keys(v).some(k=>!["scope","category","confidence","profile_key","enabled"].includes(k))||typeof v.category!=="string"||!v.category.trim()||v.category.length>80||typeof v.enabled!=="boolean"||!(v.confidence===null||typeof v.confidence==="number"&&Number.isFinite(v.confidence)&&v.confidence>=0&&v.confidence<=1)||!(v.profile_key===null||typeof v.profile_key==="string"&&!!v.profile_key.trim()&&v.profile_key.length<=128)||!v.scope||typeof v.scope!=="object"||Array.isArray(v.scope))throw new Error("invalid_memory_metadata");
 const scope=v.scope as Record<string,unknown>,key=scope.kind==="chat"?"order_id":scope.kind==="agent"?"agent_id":null;if(!["project","user"].includes(String(scope.kind))&&!key||Object.keys(scope).some(k=>k!=="kind"&&k!==key)||key&&(typeof scope[key]!=="string"||!String(scope[key]).trim()))throw new Error("invalid_memory_metadata");
 return structuredClone(v) as MemoryMetadata;
}
export function memoryMetadataForm(form:FormData):MemoryMetadata|null{return form.get("kind")==="memory"?validateMemoryMetadata(JSON.parse(String(form.get("memory")||JSON.stringify(defaultMemoryMetadata())))):null;}
