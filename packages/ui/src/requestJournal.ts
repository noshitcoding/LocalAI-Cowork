export type RequestJournalStorage=Pick<Storage,"getItem"|"setItem"|"removeItem">;
/** Auth and transient failures on a retry cannot settle an earlier lost acknowledgement. */
export function definitiveRequestRejection(status:unknown):boolean {
  return typeof status==="number"&&Number.isInteger(status)&&status>=400&&status<500&&![401,403,408,425,429].includes(status);
}
/** Serializes only caller-validated request content; credentials belong in transport. */
export class PersistentRequestLatch<T>{
  private original:T|null=null;
  private malformed=false;
  private storage?:RequestJournalStorage;
  constructor(private readonly key?:string,private readonly validate:(value:unknown)=>T=value=>value as T,storage?:RequestJournalStorage){
    if(!key)return;
    try{
      this.storage=storage??(typeof localStorage!=="undefined"?localStorage:undefined);
      if(!this.storage){if(typeof window!=="undefined")this.malformed=true;return;}
      const raw=this.storage.getItem(key);if(raw===null)return;
      if(raw.length>2_097_152)throw new Error();
      const envelope=JSON.parse(raw) as {format:number;scope:string;request:unknown};
      if(envelope?.format!==1||envelope.scope!==key||Object.keys(envelope).some(k=>!["format","scope","request"].includes(k)))throw new Error();
      this.original=this.validate(envelope.request);
    }catch{this.malformed=true;}
  }
  get pending(){return this.original!==null||this.malformed;}
  get invalid(){return this.malformed;}
  prepare(build:()=>T):T{
    if(this.malformed)throw new Error("request_journal_invalid");
    if(this.original===null){const value=this.validate(structuredClone(build()));
      if(this.key){if(!this.storage)throw new Error("request_journal_unavailable");if(this.storage.getItem(this.key)!==null){this.malformed=true;throw new Error("request_journal_changed");}this.storage.setItem(this.key,JSON.stringify({format:1,scope:this.key,request:value}));}
      this.original=value;
    }
    return structuredClone(this.original);
  }
  confirm(){if(this.key&&this.storage){const current=this.storage.getItem(this.key),expected=this.original===null?null:JSON.stringify({format:1,scope:this.key,request:this.original});if(current!==null&&current!==expected){this.malformed=true;throw new Error("request_journal_changed");}if(current!==null)this.storage.removeItem(this.key);}this.original=null;this.malformed=false;}
  discard(){if(this.key)this.storage?.removeItem(this.key);this.original=null;this.malformed=false;}
}
export function requestJournalEndpoint(base:string):string{try{const endpoint=new URL(base||"/", "http://native.local");return endpoint.origin+endpoint.pathname.replace(/\/$/,"");}catch{return "invalid-endpoint";}}
