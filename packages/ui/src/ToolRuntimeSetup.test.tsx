import {afterEach,beforeEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {documentToolsState,ensureDocumentTools,latexState,resetDocumentAutoSetup,type SetupStatus,type DocumentAvailability} from "./documentAutoSetup";
import {DocumentToolsStatus} from "./ToolRuntimeSetup";
import {workflowDataError} from "./workflowDataflow";

const de=(value:string)=>value;
const status=(patch:Partial<SetupStatus>={}):SetupStatus=>({running:false,stage:"",message:"",ready:false,programs:{},error:null,profile:null,automatic:false,documents_ready:false,browser_ready:false,auto:{attempts:0,last_attempt_at:null,next_attempt_at:null,last_error:null,paused_by_user:false},...patch});
const render=(value:SetupStatus|null,availability?:DocumentAvailability)=>renderToStaticMarkup(<DocumentToolsStatus status={value} availability={availability} tr={de} language="de" locked={false} onInstall={()=>{}}/>);

beforeEach(()=>resetDocumentAutoSetup());
afterEach(()=>{vi.unstubAllGlobals();});

describe("document tool setup state",()=>{
  it("keeps an unsupported execution path blocked despite stale installed or ready flags",()=>{
    const blocked={document_status:"data_path_too_long",latex_status:"data_path_too_long",documents_available:false};
    expect(documentToolsState(null,blocked)).toBe("path_too_long");
    expect(documentToolsState(status({documents_ready:true}),blocked)).toBe("path_too_long");
    expect(documentToolsState(status({running:true}),blocked)).toBe("path_too_long");
    expect(latexState(status({documents_ready:true}),{...blocked,latex_available:true})).toBe("data_path_too_long");
  });
  it("derives the document runtime state from the receipt, the backoff file and the daemon",()=>{
    expect(documentToolsState(null)).toBe("unavailable");
    expect(documentToolsState(status())).toBe("missing");
    expect(documentToolsState(status({running:true,profile:"full"}))).toBe("installing");
    expect(documentToolsState(status({documents_ready:true}))).toBe("ready");
    expect(documentToolsState(status(),{documents_available:true})).toBe("ready");
    expect(documentToolsState(status({auto:{attempts:1,last_attempt_at:1,next_attempt_at:null,last_error:null,paused_by_user:true}}))).toBe("paused");
    expect(documentToolsState(status({auto:{attempts:2,last_attempt_at:1,next_attempt_at:5_000,last_error:"offline",paused_by_user:false}}),undefined,1_000)).toBe("failed");
    expect(documentToolsState(status({auto:{attempts:1,last_attempt_at:1,next_attempt_at:5_000,last_error:null,paused_by_user:false}}),undefined,1_000)).toBe("waiting");
  });
  it("keeps LaTeX pending until the daemon reports confined LaTeX",()=>{
    const installed=status({documents_ready:true,programs:{tectonic:"C:/data/tool-runtime/tectonic-0.17.0-x/tectonic.exe"}});
    expect(latexState(installed,{documents_available:true,latex_available:false})).toBe("installed_inactive");
    expect(latexState(installed,{latex_available:false,latex_status:"cache_incomplete"})).toBe("cache_incomplete");
    expect(latexState(installed,{latex_status:"isolation_unavailable"})).toBe("isolation_unavailable");
    expect(latexState(installed,{latex_available:true})).toBe("active");
    expect(latexState(status({running:true}))).toBe("installing");
    expect(latexState(status({documents_ready:true}))).toBe("missing");
  });
});

describe("document tools panel",()=>{
  it("explains the Windows data path problem without suggesting a reinstall",()=>{
    const availability={documents_available:false,document_status:"data_path_too_long",latex_status:"data_path_too_long",ocr_available:false};
    const html=render(status({documents_ready:true}),availability);
    expect(html).toContain('role="alert"');expect(html).toContain("kürzeren privaten V2-Datenpfad");expect(html).toContain("Betreiber");expect(html).not.toContain("<button");
    const english=renderToStaticMarkup(<DocumentToolsStatus status={status({documents_ready:true})} availability={availability} tr={(_de,en)=>en} language="en" locked={false} onInstall={()=>{}}/>);
    expect(english).toContain("shorter private V2 data path");expect(english).not.toContain("Set up document tools now");
  });
  it("translates the pre-admission path error used by the public execution plan",()=>{
    const error=new Error("workflow_document_runtime_path_too_long");
    expect(workflowDataError(error,"de")).toContain("kürzeren privaten V2-Datenpfad");
    expect(workflowDataError(error,"en")).toContain("shorter private V2 data path");
  });
  it("requires actual OCR readiness and offers an update for an older document runtime",()=>{
    const installed=status({documents_ready:true});
    const pending=render(installed,{documents_available:true,ocr_available:false,ocr_status:"ocr_language_pack_incomplete"});
    expect(pending).toContain("Texterkennung (OCR)");expect(pending).toContain("Dokumentwerkzeuge jetzt einrichten");
    expect(pending).not.toContain("Bereit, Deutsch und Englisch lokal");
    const active=render(installed,{documents_available:true,ocr_available:true,ocr_status:"ready"});
    expect(active).toContain("Bereit, Deutsch und Englisch lokal");expect(active).not.toContain("<button");
  });
  it("does not misreport every inactive installation as missing future isolation",()=>{
    const html=render(status({documents_ready:true,programs:{tectonic:"t"}}),{documents_available:true,latex_available:false});
    expect(html).toContain("Installiert, vom Ausführungsziel noch nicht als bereit bestätigt");
    expect(html).toContain('data-state="ready"');
    expect(html).not.toContain("<button");
  });
  it("shows the reported cache failure separately from isolation failure",()=>{
    expect(render(status({documents_ready:true}),{latex_status:"cache_incomplete"})).toContain("LaTeX-Paketcache fehlt");
    expect(render(status({documents_ready:true}),{latex_status:"isolation_unavailable"})).toContain("Prozessisolation ist auf diesem Ausführungsziel nicht verfügbar");
  });
  it("offers a retry with the next automatic attempt after a failure",()=>{
    const html=render(status({error:"Tool download exceeds its bound",auto:{attempts:1,last_attempt_at:1,next_attempt_at:Date.UTC(2030,0,1),last_error:"x",paused_by_user:false}}));
    expect(html).toContain("Tool download exceeds its bound");
    expect(html).toContain("nächster automatischer Versuch");
    expect(html).toContain("Dokumentwerkzeuge erneut einrichten");
  });
  it("shows progress without an install button while the automatic setup runs",()=>{
    const html=render(status({running:true,automatic:true,profile:"documents",stage:"tectonic",message:"Downloading LaTeX packages and fonts for offline use (3/17)"}));
    expect(html).toContain("Automatische Einrichtung: tectonic: Downloading LaTeX packages");
    expect(html).toContain("<progress");
    expect(html).not.toContain("<button");
  });
});

describe("automatic document setup trigger",()=>{
  it("does nothing outside the desktop app",async()=>{
    const invoke=vi.fn();
    await expect(ensureDocumentTools(invoke)).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("asks the desktop once per session and retries after an IPC failure",async()=>{
    vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});
    const invoke=vi.fn().mockRejectedValueOnce(new Error("not ready")).mockResolvedValue(status({running:true,automatic:true,profile:"documents"}));
    await expect(ensureDocumentTools(invoke)).resolves.toBeNull();
    const first=await ensureDocumentTools(invoke);
    await ensureDocumentTools(invoke);
    expect(first?.automatic).toBe(true);
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenLastCalledWith("toolbox_setup_ensure_documents");
  });
});
