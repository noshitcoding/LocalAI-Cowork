import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {captureLatexProject,latexDiagnosticFile,latexProject,latexProjectPaths,latexProjectTool,latexProjectMatchesTool,retryProjectRead,type LatexProject} from "./latexProject";
import {RequestError} from "@cowork/client";
import {downloadLatexSource,LatexProjectEditor} from "./LatexProjectEditor";
import {DocumentDiagnostics} from "./DocumentDiagnostics";

const sha="a".repeat(64),project:LatexProject={version:1,folder:"workspace",main:"paper/main.tex",output:"compiled.pdf",files:[{path:"paper/main.tex",sha256:sha,size:40},{path:"paper/alternative.tex",sha256:sha,size:40},{path:"paper/chapters/intro.tex",sha256:sha,size:10},{path:"paper/references.bib",sha256:sha,size:15}]};
describe("versioned LaTeX projects",()=>{
  it("restores a manifest and binds every input hash with the chosen main file first",()=>{
    const restored=latexProject(JSON.parse(JSON.stringify(project)))!;
    const tool=latexProjectTool({...restored,main:"paper/alternative.tex"},"workspace");
    expect(tool.arguments).toMatchObject({operation:"compile_latex",inputs:[{path:"paper/alternative.tex",expected_sha256:sha},{path:"paper/main.tex",expected_sha256:sha},{path:"paper/chapters/intro.tex",expected_sha256:sha},{path:"paper/references.bib",expected_sha256:sha}],preview:true});
    expect(()=>latexProjectTool(project,"other")).toThrow("workspace_changed");
  });
  it("rejects unknown versions, malformed paths, duplicate files and excessive inputs",()=>{
    expect(latexProject(null)).toBeNull();
    for(const value of [{...project,version:2},{...project,unknown:true},{...project,main:"../outside.tex"},{...project,output:"C:/out.pdf"},{...project,files:[...project.files,project.files[0]]},{...project,files:[{...project.files[0],sha256:"stale"}]},{...project,files:[{...project.files[0],size:32*1024*1024+1}]}])expect(()=>latexProject(value)).toThrow();
    expect(()=>latexProjectPaths(Array.from({length:17},(_,index)=>`asset${index}.tex`).join("\n"))).toThrow();
    expect(()=>latexProjectPaths("main.tex\nMAIN.tex")).toThrow();
    expect(()=>latexProjectPaths("main.tex\nsub\\outside.tex")).toThrow();
  });
  it("refuses a main-file change which puts assets outside its directory",()=>{
    expect(()=>latexProject({...project,main:"paper/chapters/intro.tex"})).toThrow("file_invalid");
    expect(()=>latexProject({...project,files:project.files.slice(1)})).toThrow("main_invalid");
  });
  it("captures only actual files in the current workspace",async()=>{
    const result=await captureLatexProject("workspace",["main.tex"],"main.tex","result.pdf",async path=>({folder_id:"workspace",path,node:{kind:"file",sha256:sha,size:50,resource_id:"resource"}}));
    expect(result.files[0]).toEqual({path:"main.tex",sha256:sha,size:50});
    await expect(captureLatexProject("workspace",["main.tex"],"main.tex","result.pdf",async path=>({folder_id:"other",path,node:null}))).rejects.toThrow("file_missing");
  });
  it("serializes workspace lookups and retries only transient read contention",async()=>{
    let active=0,maximum=0,attempts=0;
    await captureLatexProject("workspace",["main.tex","chapter.tex"],"main.tex","result.pdf",async path=>{
      active++;maximum=Math.max(maximum,active);await Promise.resolve();active--;
      return {folder_id:"workspace",path,node:{kind:"file",sha256:sha,size:1,resource_id:"resource"}};
    });
    expect(maximum).toBe(1);
    const read=async()=>{if(attempts++===0)throw new RequestError({code:"workspace_busy",message:"busy",retryable:false,correlation_id:"test"},409);return "ready";};
    expect(await retryProjectRead(read)).toBe("ready");
    expect(attempts).toBe(2);
    attempts=0;
    await expect(retryProjectRead(async()=>{attempts++;throw new RequestError({code:"file_conflict",message:"changed",retryable:false,correlation_id:"test"},409);})).rejects.toThrow("changed");
    expect(attempts).toBe(1);
  });
  it("maps diagnostic locations only to exact project members",()=>{
    expect(latexDiagnosticFile(project,"./chapters/intro.tex")?.path).toBe("paper/chapters/intro.tex");
    expect(latexDiagnosticFile(project,"in/tex/main.tex")?.path).toBe("paper/main.tex");
    for(const path of ["../main.tex","C:/private/main.tex","/tmp/main.tex","intro.tex","unknown.sty"])expect(latexDiagnosticFile(project,path)).toBeNull();
    const html=renderToStaticMarkup(<DocumentDiagnostics engine={{diagnostics:[{file:"main.tex",line:2,message:"problem"},{file:"/private/secret",line:1,message:"missing"}]}} openSource={()=>{}} canOpenSource={file=>!!latexDiagnosticFile(project,file)}/>);
    expect(html).toContain('<button type="button">main.tex:2</button>');
    expect(html).not.toContain('<button type="button">/private/secret');
  });
  it("does not attach old diagnostics to another saved project or source version",()=>{
    const tool=latexProjectTool(project,"workspace");
    expect(latexProjectMatchesTool(project,tool)).toBe(true);
    expect(latexProjectMatchesTool({...project,main:"paper/alternative.tex"},tool)).toBe(false);
    expect(latexProjectMatchesTool({...project,folder:"other"},tool)).toBe(false);
    expect(latexProjectMatchesTool({...project,files:project.files.map((file,index)=>index===0?{...file,sha256:"b".repeat(64)}:file)},tool)).toBe(false);
    expect(latexProjectMatchesTool(project,{...tool,arguments:{...tool.arguments as object,inputs:Array(4).fill({folder_id:"workspace",path:project.main,expected_sha256:sha})}})).toBe(false);
  });
  it("downloads the captured version, decodes UTF-8 and bounds source navigation",async()=>{
    let request:unknown;
    const source=await downloadLatexSource(project,"paper/main.tex",async input=>{request=input;return {blob:new Blob(["one\ntwo\nthree"]),sha256:sha,file_name:"main.tex",size:13,media_type:"text/x-tex"};},999);
    expect(request).toEqual({path:"paper/main.tex",expected_sha256:sha});
    expect(source.line).toBe(3);
    await expect(downloadLatexSource(project,"outside.tex",async()=>{throw new Error("must not download");})).rejects.toThrow("source_unavailable");
    await expect(downloadLatexSource(project,"paper/main.tex",async()=>({blob:new Blob(["changed"]),sha256:"b".repeat(64),file_name:"main.tex",size:7,media_type:"text/x-tex"}))).rejects.toThrow("source_integrity");
  });
  it("locks project actions and source-version changes in read-only mode",()=>{
    const html=renderToStaticMarkup(<LatexProjectEditor client={{} as never} order="order" folder="workspace" value={project} onChange={()=>{}} seed="" locked ready={false} prepare={async()=>{throw new Error("must not dispatch");}} onOpen={async()=>{}} source={null}/>);
    expect(html).toContain('<fieldset disabled="">');
    expect(html).toContain("Dateiversionen erfasst");
    expect(html).toContain("Kompilierung ist hier noch nicht bereit");
  });
});
