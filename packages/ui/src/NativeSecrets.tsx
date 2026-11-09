import {useRef,useState} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {RequestError,type NativeSecretsSetup,type NativeSecretsStatus,type NativeSecretsReceipt,type UiLanguage} from "@cowork/client";
import {hasNativeSecrets,nativeSecretStatus,nativeSecretRequest,setupNativeSecrets,resolveNativeSecrets} from "./secretBridge";
import {de,type Key} from "./i18n";
import {NativeKeyRecovery} from "./NativeKeyRecovery";

const queryKey=["native-secret-status"];
export function NativeSecrets({language,t}:{language:UiLanguage,t:(key:Key)=>string}){
  const native=hasNativeSecrets();const qc=useQueryClient();
  const state=useQuery({queryKey,queryFn:nativeSecretStatus,enabled:native,retry:false,refetchOnWindowFocus:false,refetchOnReconnect:false});
  const inFlight=useRef(false);const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();
  const [request,setRequest]=useState<NativeSecretsSetup|null>(null);const [notice,setNotice]=useState<Key|null>(null);
  function record(receipt:NativeSecretsReceipt){
    qc.setQueryData<NativeSecretsStatus>(queryKey,old=>old&&old.revision>receipt.store.revision?old:receipt.store);
    setNotice(receipt.operation_state==="pending"?"secretPending":receipt.operation_state==="activated"?"secretActivated":"secretAbandoned");
  }
  async function act(operation:()=>Promise<void>){
    if(inFlight.current)return;inFlight.current=true;setBusy(true);setError(undefined);setNotice(null);
    try{await operation();}catch(error){setError(error);}finally{inFlight.current=false;setBusy(false);}
  }
  async function start(){
    const current=state.data;if(!current||current.pending_operation_id)return;
    const input=request??{idempotency_key:crypto.randomUUID(),base_revision:current.revision};setRequest(input);
    const receipt=await setupNativeSecrets(input,language);
    if(receipt){record(receipt);setRequest(null);}else{setRequest(null);setNotice("secretCanceled");}
  }
  async function check(){
    if(!request)return;
    const receipt=await nativeSecretRequest(request.idempotency_key);
    if(receipt){record(receipt);setRequest(null);}
    else{
      // A late original request still uses its original revision. Any future
      // explicit setup competes through the daemon's CAS; neither can replay IO.
      const result=await state.refetch();if(result.error)throw result.error;
      setRequest(null);setNotice("secretNotRecorded");
    }
  }
  async function resolve(action:"reconcile"|"abandon"){
    const current=state.data;if(!current?.pending_operation_id)return;
    const receipt=await resolveNativeSecrets(action,{operation_id:current.pending_operation_id,base_revision:current.revision},language);
    if(receipt){record(receipt);setRequest(null);}else setNotice("secretCanceled");
  }
  const problem=error??state.error;const code=problem instanceof RequestError?problem.details.code:"error";
  const current=state.data;const disabled=busy||state.isFetching||state.isError;
  return <section className="native-secrets">
    <p>{t("secretStorageHint")}</p><p className="muted">{t("secretPreviewHint")}</p>
    {!native?<p className="banner">{t("secretDesktopHint")}</p>:<>
      {state.isFetching&&<p role="status">{t("loading")}</p>}
      {current&&<div className="secret-status"><strong>{t(!current.configured?"secretEmpty":current.key_available?"secretReady":"secretUnavailable")}</strong>
        {current.configured&&!current.key_available&&<p>{t("secretUnavailableHint")}</p>}
        {current.pending_operation_id&&<div className="banner"><strong>{t("secretPending")}</strong><p>{t("secretPendingHint")}</p></div>}
      </div>}
      {problem&&<div className="error" role="alert"><strong>{t(code in de?code as Key:"error")}</strong>
        {problem instanceof RequestError&&<details><summary>{t("problemDetails")}</summary><code>{problem.details.correlation_id}</code></details>}
      </div>}
      {busy&&<p role="status">{t("secretBusy")}</p>}{notice&&<p role="status">{t(notice)}</p>}
      {request&&<p>{t("secretRequestHint")}</p>}
      <div className="actions secret-actions">
        <button disabled={busy||state.isFetching} onClick={()=>void act(async()=>{const result=await state.refetch();if(result.error)throw result.error;})}>{t("secretRefresh")}</button>
        {request?<button disabled={disabled} onClick={()=>void act(check)}>{t("secretCheckRequest")}</button>:
          current?.pending_operation_id?<>
            <button disabled={disabled} className="primary" onClick={()=>void act(()=>resolve("reconcile"))}>{t("secretReconcile")}</button>
            <button disabled={disabled} onClick={()=>void act(()=>resolve("abandon"))}>{t("secretAbandon")}</button>
          </>:<button disabled={disabled||!current} className="primary" onClick={()=>void act(start)}>{t(current?.configured?"secretRotate":"secretSetup")}</button>}
      </div>
      {current&&<details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({revision:current.revision,pending_operation_id:current.pending_operation_id},null,2)}</pre></details>}
      {current&&<NativeKeyRecovery language={language} revision={current.revision} configured={current.configured} disabled={disabled||!!current.pending_operation_id} refresh={()=>state.refetch()}/>}
    </>}
  </section>;
}
