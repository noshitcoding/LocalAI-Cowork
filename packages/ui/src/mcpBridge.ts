import {RequestError,decode,type Client,type NativeMcpFactory,type ConfigureOwnerMcpStdio,type OwnerMcpCapabilities,type OwnerMcpCatalog} from "@cowork/client";
import {hasNativeSecrets} from "./secretBridge";
import {productRequest} from "./productApi";
export type StdioFactory=NativeMcpFactory;
export type StdioConfiguration=ConfigureOwnerMcpStdio;
export type McpCapabilities=OwnerMcpCapabilities;
export type McpDiscovery=OwnerMcpCatalog;
export const nativeMcpAvailable=(client:Client)=>hasNativeSecrets()&&/^http:\/\/127\.0\.0\.1:43821\/?$/.test(client.base);
async function call<T>(command:string,args:Record<string,unknown>):Promise<T>{const {invoke}=await import("@tauri-apps/api/core");try{return await invoke<T>(command,args);}catch(e){if(e&&typeof e==="object"&&"code" in e)throw new RequestError(e as ConstructorParameters<typeof RequestError>[0],0);throw e;}}
export const stdioFactories=(project:string)=>call<StdioFactory[]>("mcp_stdio_factories",{project});
export const configureStdio=(input:StdioConfiguration,language:"de"|"en")=>call<McpDiscovery|null>("configure_mcp_stdio",{input,language});
export const mcpCredentialStatus=(project:string,server:string)=>call<{endpoint:string,credential_reference:string,credential_revision:number}>("mcp_credential_status",{project,server});
export const saveMcpCredential=(input:{project_id:string,server_id:string,endpoint:string,bearer:string,expected_revision:number})=>call<{credential_reference:string,credential_revision:number}>("save_mcp_credential",{input});
export const disconnectStdio=(project:string,server:string,expected_revision:number)=>call<{revision:number}>("disconnect_mcp_stdio",{project,server,expectedRevision:expected_revision});
export const nativeMcpOAuth=<T,>(operation:"profiles"|"discover"|"configure"|"start"|"flow"|"complete"|"interactions"|"answer_interaction",target:string,input?:unknown)=>call<T>("mcp_oauth_native",{operation,target,input:input??null});
export const serverMcpCapabilities=(client:Client)=>productRequest<unknown>(client,"/mcp/capabilities").then(value=>decode("OwnerMcpCapabilities",value));
export const serverStdioFactories=(client:Client,project:string)=>productRequest<unknown[]>(client,"/mcp/stdio/"+encodeURIComponent(project)).then(values=>values.map(value=>decode("NativeMcpFactory",value)));
export const configureServerStdio=(client:Client,input:StdioConfiguration)=>productRequest<unknown>(client,"/mcp/stdio","POST",input).then(value=>decode("OwnerMcpCatalog",value));
export const serverMcpCredentialStatus=(client:Client,project:string,server:string)=>productRequest<{endpoint:string,credential_reference:string,credential_revision:number}>(client,`/mcp/credential/${encodeURIComponent(project)}/${encodeURIComponent(server)}`);
export const saveServerMcpCredential=(client:Client,input:{project_id:string,server_id:string,endpoint:string,bearer:string,expected_revision:number})=>productRequest<{credential_reference:string,credential_revision:number}>(client,"/mcp/credential","POST",input);
export const disconnectServerStdio=(client:Client,project:string,server:string,expected_revision:number)=>productRequest<{revision:number}>(client,`/mcp/stdio/${encodeURIComponent(project)}/${encodeURIComponent(server)}/disconnect`,"POST",{expected_revision});
export function parseMcpEnvironment(source:string):Record<string,string>{
  const value:unknown=JSON.parse(source);
  if(!value||Array.isArray(value)||typeof value!=="object")throw new Error("mcp_environment_invalid");
  const entries=Object.entries(value);const names=new Set<string>();
  if(entries.length>32||entries.reduce((sum,[key,value])=>sum+new TextEncoder().encode(key).length+(typeof value==="string"?new TextEncoder().encode(value).length:0),0)>8192)throw new Error("mcp_environment_limit");
  for(const [name,text]of entries){if(!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)||typeof text!=="string"||new TextEncoder().encode(text).length>2000||text.includes("\0")||names.has(name.toUpperCase()))throw new Error("mcp_environment_invalid");names.add(name.toUpperCase());}
  return value as Record<string,string>;
}
