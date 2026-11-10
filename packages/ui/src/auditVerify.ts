import {decode,type SignedAuditArchive,type AuditArchiveBundle,type AuditRecord,type AuditPage,type AuditRecoveryPlan,type AuditRecoveryReceipt,type AuditV1Verification,type AuditWitnessBinding,type AuditCheckpointRequest,type SignedAuditCheckpoint,type AuditTrustTransitionProposal,type SignedAuditTrustTransition} from "@cowork/client";

const bytes=(value:string)=>Uint8Array.from(atob(value.replace(/-/g,"+").replace(/_/g,"/")),c=>c.charCodeAt(0));
const digest=async(value:Uint8Array)=>Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new Uint8Array(value)))).map(v=>v.toString(16).padStart(2,"0")).join("");
const canonicalEvent=(e:AuditRecord)=>({sequence:e.sequence,event_id:e.event_id,action:e.action,actor_id:e.actor_id,target_id:e.target_id,observed_at:e.observed_at,origin:e.origin,source_sha256:e.source_sha256,previous_sha256:e.previous_sha256,sha256:e.sha256});
const canonicalPage=(p:AuditPage)=>({format:p.format,scope:p.scope,chain_id:p.chain_id,after_sequence:p.after_sequence,previous_sha256:p.previous_sha256,through_sequence:p.through_sequence,through_sha256:p.through_sha256,events:p.events.map(canonicalEvent),next_sequence:p.next_sequence,complete:p.complete});
const json=(value:unknown)=>new TextEncoder().encode(JSON.stringify(value));
const zero="0".repeat(64),sha=(value:string)=>/^[0-9a-f]{64}$/.test(value);

/** The pin must come from a trusted connection or independently shared key. */
export async function verifyAuditArchive(input:unknown,pin:string):Promise<SignedAuditArchive> {
  const a=decode("SignedAuditArchive",input),p=a.page;
  if(!sha(pin)||a.format!=="cowork.audit.signed.v1"||a.public_key_sha256!==pin||a.captured_at<=0||p.format!=="cowork.audit.page.v1"||p.after_sequence<0||p.through_sequence<p.after_sequence||p.events.length>500||!sha(p.previous_sha256)||!sha(p.through_sha256)||p.after_sequence===0&&p.previous_sha256!==zero)throw new Error("audit_archive_invalid");
  const publicBytes=bytes(a.public_key),signature=bytes(a.signature);
  if(publicBytes.length!==32||signature.length!==64||await digest(publicBytes)!==pin)throw new Error("audit_key_invalid");
  let sequence=p.after_sequence,previous=p.previous_sha256;
  for(const e of p.events){sequence++;if(e.sequence!==sequence||e.previous_sha256!==previous||!sha(e.sha256)||e.observed_at<=0)throw new Error("audit_chain_invalid");const expected=await digest(json(["cowork:audit:event:v1",p.scope,p.chain_id,e.sequence,e.event_id,e.action,e.actor_id,e.target_id,e.observed_at,e.origin,e.source_sha256,e.previous_sha256]));if(expected!==e.sha256)throw new Error("audit_chain_invalid");previous=e.sha256;}
  if(p.next_sequence!==sequence||sequence>p.through_sequence||p.complete!==(sequence===p.through_sequence)||p.complete&&previous!==p.through_sha256||!p.complete&&!p.events.length)throw new Error("audit_chain_invalid");
  const key=await crypto.subtle.importKey("raw",new Uint8Array(publicBytes),{name:"Ed25519"},false,["verify"]);
  const signed=json(["cowork:audit:signature:v1",a.format,a.request_id,a.captured_at,a.public_key,a.public_key_sha256,canonicalPage(p)]);
  if(!await crypto.subtle.verify("Ed25519",key,new Uint8Array(signature),new Uint8Array(signed)))throw new Error("audit_signature_invalid");
  return a;
}
export async function verifyAuditBundle(input:unknown,pin:string):Promise<AuditArchiveBundle> {
  const b=decode("AuditArchiveBundle",input);if(b.format!=="cowork.audit.bundle.v1"||!b.pages.length||b.pages.length>200)throw new Error("audit_bundle_invalid");
  const first=b.pages[0].page;let after=first.after_sequence,previous=first.previous_sha256;
  for(const a of b.pages){await verifyAuditArchive(a,pin);const p=a.page;if(p.scope!==first.scope||p.chain_id!==first.chain_id||p.through_sequence!==first.through_sequence||p.through_sha256!==first.through_sha256||p.after_sequence!==after||p.previous_sha256!==previous)throw new Error("audit_bundle_continuity");after=p.next_sequence;if(p.events.length)previous=p.events[p.events.length-1].sha256;}
  return b;
}
export function encodeAuditBytes(value:Uint8Array):string {
  let text="";for(let i=0;i<value.length;i+=8192)text+=String.fromCharCode(...value.subarray(i,i+8192));
  return btoa(text).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/g,"");
}
export function decodeAuditBytes(value:string):Uint8Array {return bytes(value);}

const canonicalRecoveryPlan=(p:AuditRecoveryPlan)=>({scope:p.scope,snapshot_sha256:p.snapshot_sha256,prior_chain_id:p.prior_chain_id,prior_sequence:p.prior_sequence,records:p.records,bytes:p.bytes,corrupt:p.corrupt,verification_complete:p.verification_complete,public_key_sha256:p.public_key_sha256});
async function verifyMetadata(domain:string,publicKey:string,signature:string,pin:string,unsigned:unknown){
  const keyBytes=bytes(publicKey),signatureBytes=bytes(signature);
  if(!sha(pin)||keyBytes.length!==32||signatureBytes.length!==64||await digest(keyBytes)!==pin)throw new Error("audit_key_invalid");
  const key=await crypto.subtle.importKey("raw",new Uint8Array(keyBytes),{name:"Ed25519"},false,["verify"]);
  if(!await crypto.subtle.verify("Ed25519",key,new Uint8Array(signatureBytes),json([domain,publicKey,pin,unsigned])))throw new Error("audit_signature_invalid");
}
export async function verifyAuditV1(input:unknown,pin:string):Promise<AuditV1Verification>{
  const v=decode("AuditV1Verification",input),a=v.assessment;
  if(v.format!=="cowork.audit.v1-hmac.verification.v1"||v.scope!=="personal"||v.public_key_sha256!==pin||v.captured_at<=0||v.archives.length<1||v.archives.length>4||v.archives.some(r=>!sha(r.content_sha256))||v.key_source!=="windows-credential-manager-v1-read-only")throw new Error("audit_v1_verification_invalid");
  const unsigned={format:v.format,request_id:v.request_id,scope:v.scope,captured_at:v.captured_at,archives:v.archives.map(r=>({request_id:r.request_id,content_sha256:r.content_sha256})),assessment:{valid:a.valid,signed_events:a.signed_events,legacy_events:a.legacy_events,first_sequence:a.first_sequence,last_sequence:a.last_sequence,chain_complete:a.chain_complete,first_problem_line:a.first_problem_line,error_code:a.error_code},key_source:v.key_source,public_key:"",public_key_sha256:"",signature:""};
  await verifyMetadata("cowork:audit:v1-hmac-verification:v1",v.public_key,v.signature,pin,unsigned);return v;
}
export async function verifyAuditRetirement(input:unknown,pin:string):Promise<AuditRecoveryReceipt>{
  const r=decode("AuditRecoveryReceipt",input),p=r.original;
  if(r.format!=="cowork.audit.retirement.v1"||r.public_key_sha256!==pin||p.public_key_sha256!==pin||r.scope!==p.scope||r.captured_at<=0||!p.corrupt||!sha(p.snapshot_sha256)||p.records<0||p.records>100000||p.bytes<=0||p.bytes>64*1024*1024||!/^[-]?\d+$/.test(p.prior_sequence)||r.new_chain_id===p.prior_chain_id)throw new Error("audit_retirement_invalid");
  const unsigned={format:r.format,request_id:r.request_id,scope:r.scope,captured_at:r.captured_at,original:canonicalRecoveryPlan(p),new_chain_id:r.new_chain_id,actor_id:r.actor_id,public_key:"",public_key_sha256:"",signature:""};
  await verifyMetadata("cowork:audit:retirement:v1",r.public_key,r.signature,pin,unsigned);return r;
}
export async function auditBytesSha256(value:Uint8Array):Promise<string>{return digest(value);}

export const canonicalAuditWitness=(b:AuditWitnessBinding)=>({format:b.format,binding_id:b.binding_id,origin_id:b.origin_id,scope:b.scope,origin_public_key_sha256:b.origin_public_key_sha256,witness_public_key_sha256:b.witness_public_key_sha256,configured_at:b.configured_at,actor_id:b.actor_id,public_key:b.public_key,signature:b.signature});
const canonicalArchive=(a:SignedAuditArchive)=>({format:a.format,request_id:a.request_id,captured_at:a.captured_at,public_key:a.public_key,public_key_sha256:a.public_key_sha256,page:canonicalPage(a.page),signature:a.signature});
export const canonicalAuditCheckpointRequest=(r:AuditCheckpointRequest)=>({format:r.format,request_id:r.request_id,binding:canonicalAuditWitness(r.binding),captured_at:r.captured_at,previous_checkpoint_sha256:r.previous_checkpoint_sha256,archive:{format:r.archive.format,pages:r.archive.pages.map(canonicalArchive)},public_key:r.public_key,public_key_sha256:r.public_key_sha256,signature:r.signature});
export const canonicalAuditCheckpoint=(r:SignedAuditCheckpoint)=>({format:r.format,request_id:r.request_id,binding_id:r.binding_id,origin_id:r.origin_id,scope:r.scope,candidate_sha256:r.candidate_sha256,origin_public_key_sha256:r.origin_public_key_sha256,chain_id:r.chain_id,head_sequence:r.head_sequence,head_sha256:r.head_sha256,witnessed_at:r.witnessed_at,witness_sequence:r.witness_sequence,previous_checkpoint_sha256:r.previous_checkpoint_sha256,operator_chain_transition:r.operator_chain_transition,public_key:r.public_key,public_key_sha256:r.public_key_sha256,signature:r.signature});
export async function verifyAuditWitness(input:unknown,originPin:string,witnessPin:string):Promise<AuditWitnessBinding>{
  const b=decode("AuditWitnessBinding",input);if(b.format!=="cowork.audit.witness-binding.v1"||b.configured_at<=0||!sha(originPin)||!sha(witnessPin)||originPin===witnessPin||b.origin_public_key_sha256!==originPin||b.witness_public_key_sha256!==witnessPin)throw new Error("audit_witness_binding_invalid");
  await verifyMetadata("cowork:audit:witness-binding:v1",b.public_key,b.signature,originPin,{...canonicalAuditWitness(b),public_key:"",signature:""});return b;
}
export async function verifyAuditCheckpointRequest(input:unknown,originPin:string,witnessPin:string):Promise<AuditCheckpointRequest>{
  const r=decode("AuditCheckpointRequest",input);await verifyAuditWitness(r.binding,originPin,witnessPin);await verifyAuditBundle(r.archive,originPin);const first=r.archive.pages[0].page,last=r.archive.pages.at(-1)!.page;
  if(r.format!=="cowork.audit.checkpoint-request.v1"||r.public_key_sha256!==originPin||r.captured_at<=0||r.previous_checkpoint_sha256!==null&&(typeof r.previous_checkpoint_sha256!=="string"||!sha(r.previous_checkpoint_sha256))||first.scope!==r.binding.scope||first.after_sequence!==0||!last.complete||first.through_sequence>100000)throw new Error("audit_checkpoint_request_invalid");
  await verifyMetadata("cowork:audit:checkpoint-request:v1",r.public_key,r.signature,originPin,{...canonicalAuditCheckpointRequest(r),public_key:"",public_key_sha256:"",signature:""});return r;
}
export async function verifyAuditCheckpoint(input:unknown,witnessPin:string):Promise<SignedAuditCheckpoint>{
  const r=decode("SignedAuditCheckpoint",input);if(r.format!=="cowork.audit.independent-checkpoint.v1"||r.public_key_sha256!==witnessPin||!sha(r.candidate_sha256)||!sha(r.origin_public_key_sha256)||!sha(r.head_sha256)||r.head_sequence<0||r.head_sequence>100000||r.witness_sequence<=0||r.witnessed_at<=0||r.previous_checkpoint_sha256!==null&&(typeof r.previous_checkpoint_sha256!=="string"||!sha(r.previous_checkpoint_sha256))||(r.witness_sequence===1)!==(r.previous_checkpoint_sha256===null)||r.head_sequence===0&&r.head_sha256!==zero)throw new Error("audit_checkpoint_invalid");
  await verifyMetadata("cowork:audit:independent-checkpoint:v1",r.public_key,r.signature,witnessPin,{...canonicalAuditCheckpoint(r),public_key:"",public_key_sha256:"",signature:""});return r;
}
export async function auditCheckpointSha256(value:SignedAuditCheckpoint):Promise<string>{return digest(json(canonicalAuditCheckpoint(value)));}
export async function verifyAuditCheckpointBinding(receipt:SignedAuditCheckpoint,request:AuditCheckpointRequest):Promise<void>{
  await verifyAuditCheckpoint(receipt,request.binding.witness_public_key_sha256);const head=request.archive.pages[0].page;
  if(receipt.request_id!==request.request_id||receipt.binding_id!==request.binding.binding_id||receipt.origin_id!==request.binding.origin_id||receipt.scope!==request.binding.scope||receipt.origin_public_key_sha256!==request.binding.origin_public_key_sha256||receipt.candidate_sha256!==await digest(json(canonicalAuditCheckpointRequest(request)))||receipt.chain_id!==head.chain_id||receipt.head_sequence!==head.through_sequence||receipt.head_sha256!==head.through_sha256||receipt.previous_checkpoint_sha256!==request.previous_checkpoint_sha256)throw new Error("audit_checkpoint_binding_invalid");
}
export const canonicalAuditTrustProposal=(p:AuditTrustTransitionProposal)=>({format:p.format,request_id:p.request_id,previous_binding:canonicalAuditWitness(p.previous_binding),next_binding:canonicalAuditWitness(p.next_binding),previous_checkpoint:canonicalAuditCheckpoint(p.previous_checkpoint),prepared_at:p.prepared_at,previous_origin_signature:p.previous_origin_signature,next_origin_signature:p.next_origin_signature});
export const canonicalAuditTrustTransition=(p:SignedAuditTrustTransition)=>({format:p.format,proposal:canonicalAuditTrustProposal(p.proposal),accepted_at:p.accepted_at,previous_witness_public_key:p.previous_witness_public_key,next_witness_public_key:p.next_witness_public_key,previous_witness_signature:p.previous_witness_signature,next_witness_signature:p.next_witness_signature});
export async function verifyAuditTrustProposal(input:unknown,oldOrigin:string,newOrigin:string,oldWitness:string,newWitness:string):Promise<AuditTrustTransitionProposal>{
  const p=decode("AuditTrustTransitionProposal",input),a=p.previous_binding,b=p.next_binding,h=p.previous_checkpoint;
  await verifyAuditWitness(a,oldOrigin,oldWitness);await verifyAuditWitness(b,newOrigin,newWitness);await verifyAuditCheckpoint(h,oldWitness);
  if(p.format!=="cowork.audit.trust-proposal.v1"||p.request_id!==b.binding_id||a.binding_id===b.binding_id||a.origin_id!==b.origin_id||a.scope!==b.scope||p.prepared_at<=0||p.prepared_at<a.configured_at||p.prepared_at<h.witnessed_at||b.configured_at!==p.prepared_at||h.binding_id!==a.binding_id||h.origin_id!==a.origin_id||h.scope!==a.scope||h.origin_public_key_sha256!==oldOrigin||oldOrigin===newOrigin&&oldWitness===newWitness)throw new Error("audit_trust_proposal_invalid");
  const unsigned={...canonicalAuditTrustProposal(p),previous_origin_signature:"",next_origin_signature:""};
  await verifyMetadata("cowork:audit:trust-origin-transition:v1",a.public_key,p.previous_origin_signature,oldOrigin,unsigned);await verifyMetadata("cowork:audit:trust-origin-transition:v1",b.public_key,p.next_origin_signature,newOrigin,unsigned);return p;
}
export async function verifyAuditTrustTransition(input:unknown,oldOrigin:string,newOrigin:string,oldWitness:string,newWitness:string):Promise<SignedAuditTrustTransition>{
  const p=decode("SignedAuditTrustTransition",input);await verifyAuditTrustProposal(p.proposal,oldOrigin,newOrigin,oldWitness,newWitness);
  if(p.format!=="cowork.audit.trust-transition.v1"||p.accepted_at<p.proposal.prepared_at)throw new Error("audit_trust_transition_invalid");
  const unsigned={...canonicalAuditTrustTransition(p),previous_witness_public_key:"",next_witness_public_key:"",previous_witness_signature:"",next_witness_signature:""};
  await verifyMetadata("cowork:audit:trust-witness-transition:v1",p.previous_witness_public_key,p.previous_witness_signature,oldWitness,unsigned);await verifyMetadata("cowork:audit:trust-witness-transition:v1",p.next_witness_public_key,p.next_witness_signature,newWitness,unsigned);return p;
}
