import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {deliverDownload,deliveryStatusKey,safeFileName} from "./download";
const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
const bytes=new Uint8Array([0,255,128,42]);
beforeEach(()=>{invoke.mockReset();vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});});
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();vi.useRealTimers();});

for(const native of [false,true])it(`refuses a stale ${native?"native":"browser"} handoff after asynchronous byte preparation`,async()=>{
  vi.stubGlobal("window",native?{__TAURI_INTERNALS__:{}}:{});
  const create=vi.fn();vi.stubGlobal("document",{createElement:create});
  const abort=new AbortController(),blob=new Blob([bytes]);
  let finish!:(value:ArrayBuffer)=>void;
  vi.spyOn(blob,"arrayBuffer").mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
  const pending=deliverDownload(blob,"file.bin","en",undefined,abort.signal);
  abort.abort();finish(new Uint8Array(bytes).buffer);
  await expect(pending).rejects.toMatchObject({name:"AbortError"});
  expect(invoke).not.toHaveBeenCalled();expect(create).not.toHaveBeenCalled();
});

it("refuses a stale handoff after the final asynchronous integrity digest",async()=>{
  const abort=new AbortController();let finish!:(value:ArrayBuffer)=>void,entered!:()=>void;
  const reached=new Promise<void>(resolve=>{entered=resolve;});
  vi.spyOn(crypto.subtle,"digest").mockImplementation(()=>new Promise(resolve=>{finish=resolve;entered();}));
  const pending=deliverDownload(new Blob([bytes]),"file.bin","en",undefined,abort.signal);
  await reached;abort.abort();finish(new ArrayBuffer(32));
  await expect(pending).rejects.toMatchObject({name:"AbortError"});expect(invoke).not.toHaveBeenCalled();
});

it("preserves the original native outcome if the view leaves after save dispatch",async()=>{
  const abort=new AbortController();
  invoke.mockImplementation(async(_command,_body,options)=>{
    abort.abort();const request=JSON.parse(decodeURIComponent(options.headers["x-cowork-save"]));
    return {status:"saved",file:request.file};
  });
  expect(await deliverDownload(new Blob([bytes]),"file.bin","en",undefined,abort.signal)).toBe("saved");
  expect(invoke).toHaveBeenCalledOnce();
});

it("sends verified raw bytes and metadata without a host path, accepting only a matching save receipt",async()=>{
  invoke.mockImplementation(async(command,body,options)=>{
    expect(command).toBe("save_file");expect(body).toEqual(bytes);
    const request=JSON.parse(decodeURIComponent(options.headers["x-cowork-save"]));
    expect(Object.keys(request).sort()).toEqual(["file","language"]);
    expect(request.language).toBe("de");expect(request.file.file_name).toBe("Prüfung 📄.bin");
    return {status:"saved",file:{...request.file,file_name:"User choice.bin"}};
  });
  expect(await deliverDownload(new Blob([bytes]),"Prüfung 📄.bin","de")).toBe("saved");
  expect(deliveryStatusKey("saved")).toBe("fileSaved");
});

it("treats cancellation as cancellation and never falls back to a browser download",async()=>{
  invoke.mockResolvedValue({status:"canceled"});
  vi.stubGlobal("document",{createElement:vi.fn(()=>{throw new Error("unexpected browser fallback");})});
  expect(await deliverDownload(new Blob([]),"empty.bin","en")).toBe("canceled");
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(deliveryStatusKey("canceled")).toBe("saveCanceled");
});

it("preserves structured errors and reports unknown or inconsistent native completion as uncertain",async()=>{
  const error={code:"save_conflict",message:"Changed",retryable:false,correlation_id:"test"};
  invoke.mockRejectedValue(error);
  await expect(deliverDownload(new Blob([bytes]),"file.bin","en")).rejects.toMatchObject({details:error});
  invoke.mockRejectedValue(new Error("IPC disconnected after possible save"));
  await expect(deliverDownload(new Blob([bytes]),"file.bin","en")).rejects.toMatchObject({details:{code:"save_uncertain",retryable:false}});
  for(const result of [{},{status:"saved",file:{file_name:"file.bin",size:4,sha256:"0".repeat(64)}}]){
    invoke.mockResolvedValue(result);
    await expect(deliverDownload(new Blob([bytes]),"file.bin","en")).rejects.toMatchObject({details:{code:"save_uncertain",retryable:false}});
  }
  expect(invoke).toHaveBeenCalledTimes(4);
});

it("rejects bad integrity before requesting any native operation and sanitizes Unicode filenames",async()=>{
  await expect(deliverDownload(new Blob([bytes]),"file.bin","en",{size:4,sha256:"0".repeat(64)})).rejects.toThrow("download_integrity");
  await expect(deliverDownload(new Blob([bytes]),"file.bin","en",{size:3,sha256:"0".repeat(64)})).rejects.toThrow("download_integrity");
  expect(invoke).not.toHaveBeenCalled();
  expect(safeFileName("../CON:secret\u202e ")).toBe(".._CON_secret_");
  expect(safeFileName("NUL.txt")).toBe("download_NUL.txt");
  expect(safeFileName(".cowork-token")).toBe("download_.cowork-token");
  const name=safeFileName("📄".repeat(100));expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(240);
  expect(name).not.toContain("�");
});

it("reports web handoff separately and revokes its temporary URL",async()=>{
  vi.useFakeTimers();vi.stubGlobal("window",{});
  class FocusTarget {isConnected=true;focus=vi.fn();}
  vi.stubGlobal("HTMLElement",FocusTarget);const previous=new FocusTarget();
  const anchor={href:"",download:"",hidden:false,click:vi.fn(),remove:vi.fn()};
  const append=vi.fn();vi.stubGlobal("document",{activeElement:previous,body:{append},createElement:vi.fn(()=>anchor)});
  const create=vi.spyOn(URL,"createObjectURL").mockReturnValue("blob:fixture");
  const revoke=vi.spyOn(URL,"revokeObjectURL").mockImplementation(()=>{});
  const blob=new Blob([bytes]);
  expect(await deliverDownload(blob,"file.bin","de")).toBe("browser");
  expect(deliveryStatusKey("browser")).toBe("downloadPrepared");
  expect(create).toHaveBeenCalledWith(blob);expect(append).toHaveBeenCalledWith(anchor);
  expect(anchor.click).toHaveBeenCalledOnce();expect(anchor.remove).toHaveBeenCalledOnce();expect(invoke).not.toHaveBeenCalled();
  expect(previous.focus).toHaveBeenCalledWith({preventScroll:true});
  vi.advanceTimersByTime(60_000);expect(revoke).toHaveBeenCalledWith("blob:fixture");
});
