import {useCallback,useSyncExternalStore} from "react";
import type {Client,PrepareOwnerExecutionPlanV2} from "@cowork/client";
import {requestJournalEndpoint} from "./requestJournal";

export type DefaultExecutionMode="broker_controller"|"document_engine"|"ask";
export const defaultExecutionMode:DefaultExecutionMode="broker_controller";
type StorageLike=Pick<Storage,"getItem"|"setItem">;
const prefix="cowork-v2:execution-default:v1:";
export const executionDefaultsScope=(client:Pick<Client,"base">)=>requestJournalEndpoint(client.base);
export const executionDefaultsStorageKey=(scope:string)=>prefix+encodeURIComponent(scope);

export function readDefaultExecutionMode(storage:Pick<StorageLike,"getItem">,scope:string):DefaultExecutionMode{
  try{
    const value=JSON.parse(storage.getItem(executionDefaultsStorageKey(scope))??"null");
    return value==="document_engine"||value==="ask"?value:defaultExecutionMode;
  }catch{return defaultExecutionMode;}
}
export function saveDefaultExecutionMode(storage:StorageLike,scope:string,mode:DefaultExecutionMode){
  storage.setItem(executionDefaultsStorageKey(scope),JSON.stringify(mode));
}
export function defaultExecutionTarget(client:Pick<Client,"base">):PrepareOwnerExecutionPlanV2["target"]|null{
  const mode=typeof localStorage==="undefined"?defaultExecutionMode:readDefaultExecutionMode(localStorage,executionDefaultsScope(client));
  return mode==="ask"?null:{kind:mode};
}

const snapshots=new Map<string,DefaultExecutionMode>(),listeners=new Set<()=>void>();
function getSnapshot(scope:string){
  if(!snapshots.has(scope))snapshots.set(scope,typeof localStorage==="undefined"?defaultExecutionMode:readDefaultExecutionMode(localStorage,scope));
  return snapshots.get(scope)!;
}
function subscribe(listener:()=>void){
  listeners.add(listener);
  const changed=(event:StorageEvent)=>{if(event.key===null||event.key.startsWith(prefix)){snapshots.clear();for(const notify of listeners)notify();}};
  window.addEventListener("storage",changed);
  return()=>{listeners.delete(listener);window.removeEventListener("storage",changed);};
}
export function useDefaultExecutionMode(client:Pick<Client,"base">){
  const scope=executionDefaultsScope(client);
  const mode=useSyncExternalStore(subscribe,useCallback(()=>getSnapshot(scope),[scope]),()=>defaultExecutionMode);
  const setMode=useCallback((next:DefaultExecutionMode)=>{
    // A failed write must not appear to change the default.
    saveDefaultExecutionMode(localStorage,scope,next);
    snapshots.set(scope,next);for(const notify of listeners)notify();
  },[scope]);
  return {mode,setMode};
}
