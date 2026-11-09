import {beforeEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,RequestError} from "@cowork/client";
import {en} from "./i18n";
import {WorkingFiles,refreshWorkingFiles} from "./WorkingFiles";

const queries=vi.hoisted(()=>({workspace:"pending" as "pending"|"error"|"success"|"revalidating",workspaceError:undefined as unknown,listingError:undefined as unknown,listing:"pending" as "pending"|"success",options:[] as Array<{queryKey:readonly unknown[];enabled?:boolean}>,entries:[] as unknown[]}));
vi.mock("./FileUploads",()=>({FileUploads:()=>null}));
vi.mock("@tanstack/react-query",()=>({useQuery:(options:{queryKey:readonly unknown[];enabled?:boolean})=>{
  queries.options.push(options);
  const name=options.queryKey[0];
  if(name==="workspace")return {isPending:queries.workspace==="pending",isSuccess:queries.workspace==="success"||queries.workspace==="revalidating",isError:queries.workspace==="error",isFetching:queries.workspace==="pending"||queries.workspace==="revalidating",error:queries.workspaceError,data:undefined,refetch:vi.fn()};
  if(name==="workspace-files")return {isPending:queries.listing==="pending",isSuccess:queries.listing==="success",isError:!!queries.listingError,error:queries.listingError,data:queries.listing==="success"?{entries:queries.entries,truncated:false}:undefined,refetch:vi.fn()};
  return {data:undefined,isError:false,refetch:vi.fn()};
}}));
function render(){return renderToStaticMarkup(<WorkingFiles order="order" client={new Client("fixture")} t={key=>en[key]} prepareImport={()=>{}}/>);}
beforeEach(()=>{queries.workspace="pending";queries.listing="pending";queries.options=[];queries.entries=[];queries.workspaceError=undefined;queries.listingError=undefined;});
describe("working-directory initialization",()=>{
  it("holds listing and preview during cached workspace revalidation while keeping prior files visible",()=>{
    queries.workspace="revalidating";queries.listing="success";
    queries.entries=decode("DirectoryListing",{entries:[{name:"retained.txt",path:"retained.txt",kind:"file",size:12}],folder_id:"folder",path:".",truncated:false}).entries;
    const markup=render();
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-files")?.enabled).toBe(false);
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-text")?.enabled).toBe(false);
    expect(markup).toContain("retained.txt");expect(markup).toContain(en.loading);
    expect(markup).toMatch(/aria-label="Refresh files" disabled=""/);
  });
  it("shows broker backpressure as a neutral temporary status and preserves genuine error alerts",()=>{
    queries.workspace="success";queries.listingError=new RequestError(decode("ApiError",{code:"workspace_busy",message:"Broker lock held",retryable:false,correlation_id:"test"}),409);
    let markup=render();expect(markup).toContain(en.workingFilesBusy);expect(markup).not.toContain('role="alert"');expect(markup).toContain(en.retry);
    queries.workspace="error";queries.workspaceError=new Error("Unavailable database");
    markup=render();expect(markup).toContain(en.workingFilesBusy);expect(markup).toContain('role="alert"');expect(markup).toContain(en.workingFilesUnavailable);
  });
  it("holds directory listing and text preview until the workspace is confirmed",()=>{
    const markup=render();
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-files")?.enabled).toBe(false);
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-text")?.enabled).toBe(false);
    expect(markup).toContain(en.loading);
    expect(markup).not.toContain(en.workingFilesUnavailable);
    expect(markup).toMatch(/aria-label="Refresh files" disabled=""/);
  });
  it("offers a retry after initialization failure without showing endless listing loading",()=>{
    queries.workspace="error";
    const markup=render();
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-files")?.enabled).toBe(false);
    expect(markup).toContain(en.workingFilesUnavailable);
    expect(markup).toContain(en.retry);
    expect(markup).not.toContain(en.loading);
  });
  it("keeps confirmed historical working files accessible instead of hiding the file area for workflows",()=>{
    queries.workspace="success";queries.listing="success";
    queries.entries=decode("DirectoryListing",{entries:[{name:"existing.txt",path:"existing.txt",kind:"file",size:12}],folder_id:"folder",path:".",truncated:false}).entries;
    const markup=render();
    expect(queries.options.find(q=>q.queryKey[0]==="workspace-files")?.enabled).toBe(true);
    expect(markup).toContain("existing.txt");
    expect(markup).toContain("Download current file: existing.txt");
  });
  it("does not force a disabled listing refresh during initialization",async()=>{
    const workspace={isFetching:true,refetch:vi.fn()};const listing=vi.fn();const preview=vi.fn();
    await refreshWorkingFiles(workspace,listing,preview);
    expect(workspace.refetch).not.toHaveBeenCalled();expect(listing).not.toHaveBeenCalled();expect(preview).not.toHaveBeenCalled();
  });
  it("waits for a successful workspace retry before refreshing directory and preview",async()=>{
    let confirm!:(value:{isSuccess:boolean})=>void;
    const calls:string[]=[];
    const workspace={isFetching:false,refetch:vi.fn(()=>{calls.push("workspace");return new Promise<{isSuccess:boolean}>(resolve=>{confirm=resolve;});})};
    const listing=vi.fn(async()=>{calls.push("listing");});const preview=vi.fn(async()=>{calls.push("preview");});
    const pending=refreshWorkingFiles(workspace,listing,preview);
    expect(calls).toEqual(["workspace"]);confirm({isSuccess:true});await pending;
    expect(calls).toEqual(["workspace","listing","preview"]);
  });
  it("does not issue listing or preview requests after a failed workspace retry",async()=>{
    const workspace={isFetching:false,refetch:vi.fn(async()=>({isSuccess:false}))};const listing=vi.fn();const preview=vi.fn();
    await refreshWorkingFiles(workspace,listing,preview);
    expect(workspace.refetch).toHaveBeenCalledOnce();expect(listing).not.toHaveBeenCalled();expect(preview).not.toHaveBeenCalled();
  });
});