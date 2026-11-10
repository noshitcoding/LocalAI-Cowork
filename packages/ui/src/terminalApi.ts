import type {Client} from "@cowork/client";
import {productRequest} from "./productApi";
export type TerminalLaunch={idempotency_key:string;program:string;rows:number;cols:number;approval_sha256:string|null};
export type TerminalPreview={approval_sha256:string;executable:string;cwd:string;boundary:string};
export type TerminalSession={id:string;order_id:string;program:string;state:string;pid:number|null;rows:number;cols:number;created_at:number;exit_code:number|null;input_sequence:number};
export type TerminalOutput={session:TerminalSession;first_sequence:number;next_sequence:number;bytes_base64:string;truncated:boolean};
export const terminalPath=(order:string)=>`/orders/${encodeURIComponent(order)}/terminals`;
export const listTerminals=(client:Client,order:string)=>productRequest<TerminalSession[]>(client,terminalPath(order));
export function validateTerminalLaunch(value:unknown):TerminalLaunch{const v=value as TerminalLaunch;if(!v||Object.keys(v).some(k=>!["idempotency_key","program","rows","cols","approval_sha256"].includes(k))||typeof v.idempotency_key!=="string"||!v.idempotency_key||v.idempotency_key.length>128||!["powershell","bash","sh"].includes(v.program)||!Number.isInteger(v.rows)||v.rows<2||v.rows>200||!Number.isInteger(v.cols)||v.cols<10||v.cols>400||!(v.approval_sha256===null||typeof v.approval_sha256==="string"&&/^[a-f0-9]{64}$/.test(v.approval_sha256)))throw new Error("terminal_launch_invalid");return v;}
export function terminalBytes(bytes:Uint8Array){let raw="";for(const byte of bytes)raw+=String.fromCharCode(byte);return btoa(raw);}
export const decodeTerminalBytes=(raw:string)=>Uint8Array.from(atob(raw),c=>c.charCodeAt(0));
/** Cursor checks prevent duplicate output and silent gaps after reconnect. */
export function acceptTerminalOutput(after:number,value:TerminalOutput){if(!Number.isSafeInteger(value.first_sequence)||!Number.isSafeInteger(value.next_sequence)||value.first_sequence<after||value.next_sequence<value.first_sequence||value.first_sequence!==after&&!value.truncated)throw new Error("terminal_cursor_invalid");const bytes=decodeTerminalBytes(value.bytes_base64);if(bytes.length!==value.next_sequence-value.first_sequence)throw new Error("terminal_cursor_invalid");return bytes;}
