import {useEffect,useMemo,useRef,useState} from "react";
import type {Client} from "@cowork/client";
import {HistoricalImportPanel} from "./HistoricalImportPanel";
import {nativeHistoricalImport,organizationHistoricalImport} from "./historicalImport";
import {useView} from "./view";

type Target={id:string;name:string;revision:number;profile_revision?:number};
export function NativeHistoricalImportWorkspace({client,onActivated}:{client:Client;onActivated?:()=>Promise<void>}){
  const language=useView(v=>v.language),api=useMemo(()=>nativeHistoricalImport(client,language),[client.base,client.token,language]);
  const [targets,setTargets]=useState<Target[]>([]),[error,setError]=useState(""),generation=useRef(0);
  async function reload(epoch=generation.current){const values=await client.projects();if(epoch===generation.current)setTargets(values.map(v=>({id:v.id,name:v.name,revision:v.revision})));}
  useEffect(()=>{const epoch=++generation.current;setTargets([]);setError("");void reload(epoch).catch(e=>{if(epoch===generation.current)setError(e instanceof Error?e.message:String(e));});return()=>{generation.current++;};},[client.base,client.token]);
  return <>{error&&<p className="error" role="alert">{language==="de"?"Die aktuellen persönlichen Ziele konnten nicht geladen werden.":"The current personal targets could not be loaded."} <code>{error}</code></p>}<HistoricalImportPanel key={api.scope} api={api} targets={targets} onActivated={async()=>{await reload();await onActivated?.();}}/>{!targets.length&&<p>{language==="de"?"Neue persönliche Projekte können im Importplan ausdrücklich angelegt werden.":"The import plan can explicitly create new personal projects."}</p>}</>;
}
export function OrganizationHistoricalImportWorkspace({client,session,actorId,team,project,onActivated}:{client:Client;session:string;actorId:string;team:string;project:string;onActivated?:()=>Promise<void>}){
  const api=useMemo(()=>organizationHistoricalImport(client,session,actorId,team,project),[client.base,session,actorId,team,project]);
  const language=useView(v=>v.language),[targets,setTargets]=useState<Target[]>([]),[error,setError]=useState(""),generation=useRef(0);
  async function reload(epoch=generation.current){const values=await api.json<Target[]>("/targets");if(!Array.isArray(values)||values.some(v=>typeof v.id!=="string"||typeof v.name!=="string"||!Number.isSafeInteger(v.revision)||v.revision<1||!Number.isSafeInteger(v.profile_revision)||Number(v.profile_revision)<0))throw new Error("historical_target_metadata_invalid");if(epoch===generation.current)setTargets(values);}
  useEffect(()=>{const epoch=++generation.current;setTargets([]);setError("");if(!session||!actorId||!team||!project)return;void reload(epoch).catch(e=>{if(epoch===generation.current)setError(e instanceof Error?e.message:String(e));});return()=>{generation.current++;};},[client.base,session,actorId,team,project]);
  if(!session||!actorId||!team||!project)return <p>{language==="de"?"Wähle das aktuelle Team und ein berechtigtes Projekt als Importzugang.":"Select the current team and an authorized project for import admission."}</p>;
  return <>{error&&<p className="error" role="alert">{language==="de"?"Die aktuell berechtigten Teamziele konnten nicht geladen werden.":"The currently authorized team targets could not be loaded."} <code>{error}</code></p>}<HistoricalImportPanel key={api.scope} api={api} targets={targets} onActivated={async()=>{await reload();await onActivated?.();}}/></>;
}
