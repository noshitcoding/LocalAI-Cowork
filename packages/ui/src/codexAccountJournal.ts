import {decode,type Client,type CodexAccountAction,type CodexLoginRequest,type WriteCodexAccount,type DeleteLibraryEntry} from "@cowork/client";
import {requestJournalEndpoint} from "./requestJournal";
export type CodexAccountPending={path:string;method:"POST"|"PUT"|"DELETE";body:WriteCodexAccount|CodexLoginRequest|CodexAccountAction|DeleteLibraryEntry};
const identifier="[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}";
const account=new RegExp("^/accounts/("+identifier+")(?:/(login|logout))?$","i");
export function codexAccountScope(client:Client,session?:string,actorId?:string){
 if(session!==undefined&&(!session||!actorId)||actorId!==undefined&&!session||session===undefined&&!client.token)throw new Error("codex_account_session_required");
 return "cowork.codex-account.v1:"+JSON.stringify([requestJournalEndpoint(client.base),session!==undefined?actorId:"installation-owner"]);
}
export function validateCodexAccountPending(raw:unknown):CodexAccountPending{
 const value=raw as CodexAccountPending;if(!value||Object.keys(value).some(k=>!["path","method","body"].includes(k)))throw new Error("codex_account_reference_invalid");
 const match=typeof value.path==="string"?account.exec(value.path):null;
 let body:CodexAccountPending["body"];
 if(value.path==="/accounts"&&value.method==="POST")body=decode("WriteCodexAccount",value.body);
 else if(match&&!match[2]&&value.method==="PUT")body=decode("WriteCodexAccount",value.body);
 else if(match&&!match[2]&&value.method==="DELETE")body=decode("DeleteLibraryEntry",value.body);
 else if(match?.[2]==="login"&&value.method==="POST")body=decode("CodexLoginRequest",value.body);
 else if(match?.[2]==="logout"&&value.method==="POST")body=decode("CodexAccountAction",value.body);
 else throw new Error("codex_account_reference_invalid");
 if("value" in body&&(typeof body.value.name!=="string"||!body.value.name.trim()||body.value.name.length>200||/[\u0000-\u001f\u007f]/.test(body.value.name)||!Number.isSafeInteger(body.value.priority)||body.value.priority===undefined||body.value.priority<0||body.value.priority>10000))throw new Error("codex_account_reference_invalid");
 if(typeof body.idempotency_key!=="string"||!body.idempotency_key||body.idempotency_key.length>200||/[\u0000-\u001f\u007f]/.test(body.idempotency_key)||!Number.isSafeInteger(body.expected_revision)||body.expected_revision<(value.path==="/accounts"?0:1)||value.path==="/accounts"&&body.expected_revision!==0)throw new Error("codex_account_reference_invalid");
 return {path:value.path,method:value.method,body};
}
export function codexAccountReceipt(operation:CodexAccountPending,raw:unknown){
 const id=account.exec(operation.path)?.[1];
 if(operation.path.endsWith("/login")){const value=decode("CodexLoginView",raw);if(value.flow!==(operation.body as CodexLoginRequest).flow)throw new Error("codex_login_receipt_mismatch");return value;}
 if(operation.path.endsWith("/logout")){const value=raw as {signed_out:boolean};if(!value||Object.keys(value).length!==1||value.signed_out!==true)throw new Error("codex_logout_receipt_mismatch");return value;}
 if(operation.method==="DELETE"){const value=decode("LibraryDeleted",raw);if(value.id!==id||value.revision!==operation.body.expected_revision+1||!value.deleted)throw new Error("codex_account_receipt_mismatch");return value;}
 const value=decode("CodexAccount",raw),expected=(operation.body as WriteCodexAccount).value;
 if(id&&value.id!==id||value.revision!==operation.body.expected_revision+1||value.value.name!==expected.name||value.value.priority!==expected.priority)throw new Error("codex_account_receipt_mismatch");return value;
}
