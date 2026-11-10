import {requestJournalEndpoint} from "./requestJournal";
import {decodeTerminalBytes,terminalBytes} from "./terminalApi";
export type TerminalPersistenceMode="runtime"|"scrollback"|"restore-tabs";
export const terminalPersistenceMode=(value:unknown):TerminalPersistenceMode=>value==="scrollback"||value==="restore-tabs"?value:"runtime";
export const terminalStateKey=(base:string,order:string,id="tabs")=>"cowork-terminal-state:v1:"+encodeURIComponent(JSON.stringify([requestJournalEndpoint(base),order,id]));
type Storage=Pick<globalThis.Storage,"getItem"|"setItem"|"removeItem">;
export type TerminalScrollback={cursor:number;bytes:Uint8Array};
export function readTerminalScrollback(storage:Storage,key:string):TerminalScrollback|null{try{const raw=storage.getItem(key);if(!raw)return null;if(raw.length>200000)throw new Error();const v=JSON.parse(raw);if(v.format!==1||!Number.isSafeInteger(v.cursor)||v.cursor<0||typeof v.bytes!=="string")throw new Error();const bytes=decodeTerminalBytes(v.bytes);if(bytes.length>65536||v.cursor<bytes.length)throw new Error();return {cursor:v.cursor,bytes};}catch{storage.removeItem(key);return null;}}
export function appendTerminalScrollback(previous:Uint8Array,next:Uint8Array){const bytes=new Uint8Array(Math.min(65536,previous.length+next.length));const kept=next.slice(Math.max(0,next.length-65536));const oldCount=bytes.length-kept.length;bytes.set(previous.slice(previous.length-oldCount),0);bytes.set(kept,oldCount);return bytes;}
export function saveTerminalScrollback(storage:Storage,key:string,value:TerminalScrollback){storage.setItem(key,JSON.stringify({format:1,cursor:value.cursor,bytes:terminalBytes(value.bytes)}));}
export function readTerminalTabs(storage:Storage,key:string):{active:string;closed:string[]}|null{try{const raw=storage.getItem(key);if(!raw||raw.length>16000)return null;const v=JSON.parse(raw);if(v.format!==1||typeof v.active!=="string"||!Array.isArray(v.closed)||v.closed.length>128||!v.closed.every((id:unknown)=>typeof id==="string"&&id.length<=128))return null;return {active:v.active,closed:v.closed};}catch{return null;}}
export function saveTerminalTabs(storage:Storage,key:string,value:{active:string;closed:string[]}){storage.setItem(key,JSON.stringify({format:1,active:value.active,closed:value.closed.slice(-128)}));}
