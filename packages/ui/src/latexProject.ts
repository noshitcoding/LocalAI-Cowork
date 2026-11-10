import {RequestError,type FileStat,type ToolCall} from "@cowork/client";

export type LatexProjectFile={path:string;sha256:string;size:number};
export type LatexProject={version:1;folder:string;main:string;files:LatexProjectFile[];output:string};
export const MAX_LATEX_PROJECT_FILES=16;
const object=(value:unknown):Record<string,unknown>=>value!==null&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};
const keys=(value:Record<string,unknown>,allowed:string[])=>Object.keys(value).every(key=>allowed.includes(key));
export function projectPath(value:unknown):value is string{
  return typeof value==="string"&&value.length>0&&value.length<=4096&&!/[\\:\x00-\x1f\x7f]/.test(value)&&value.split("/").every(part=>part!==""&&part!=="."&&part!=="..");
}
const base=(path:string)=>path.includes("/")?path.slice(0,path.lastIndexOf("/")+1):"";
export function latexProject(value:unknown):LatexProject|null{
  if(value===undefined||value===null)return null;
  const raw=object(value);
  if(!keys(raw,["version","folder","main","files","output"])||raw.version!==1||typeof raw.folder!=="string"||!raw.folder||raw.folder.length>128||!projectPath(raw.main)||!/\.tex$/i.test(raw.main)||!projectPath(raw.output)||!/\.pdf$/i.test(raw.output)||!Array.isArray(raw.files)||!raw.files.length||raw.files.length>MAX_LATEX_PROJECT_FILES)throw new Error("latex_project_invalid");
  const seen=new Set<string>(),directory=base(raw.main);
  const files=raw.files.map(value=>{
    const file=object(value);
    if(!keys(file,["path","sha256","size"])||!projectPath(file.path)||!file.path.startsWith(directory)||seen.has(file.path.toLowerCase())||typeof file.sha256!=="string"||!/^[a-f0-9]{64}$/.test(file.sha256)||!Number.isSafeInteger(file.size)||Number(file.size)<0||Number(file.size)>32*1024*1024)throw new Error("latex_project_file_invalid");
    seen.add(file.path.toLowerCase());
    return {path:file.path,sha256:file.sha256,size:Number(file.size)};
  });
  if(!files.some(file=>file.path===raw.main)||files.some(file=>file.path.toLowerCase()===String(raw.output).toLowerCase()))throw new Error("latex_project_main_invalid");
  return {version:1,folder:raw.folder,main:raw.main,files,output:raw.output};
}
export function latexProjectPaths(text:string):string[]{
  const paths=text.split("\n").map(path=>path.trim()).filter(Boolean);
  if(!paths.length||paths.length>MAX_LATEX_PROJECT_FILES||paths.some(path=>!projectPath(path))||new Set(paths.map(path=>path.toLowerCase())).size!==paths.length)throw new Error("latex_project_paths_invalid");
  return paths;
}
export async function retryProjectRead<T>(read:()=>Promise<T>):Promise<T>{
  for(let attempt=0;;attempt++){
    try{return await read();}catch(error){
      if(!(error instanceof RequestError)||error.status!==409||error.details.code!=="workspace_busy"||attempt>=3)throw error;
      await new Promise(resolve=>setTimeout(resolve,80*(attempt+1)));
    }
  }
}
/** Capturing does not update an existing manifest until every stat has succeeded. */
export async function captureLatexProject(folder:string,paths:string[],main:string,output:string,stat:(path:string)=>Promise<FileStat>):Promise<LatexProject>{
  latexProjectPaths(paths.join("\n"));
  const files:LatexProjectFile[]=[];
  // Workspace lookup briefly holds an exclusive broker lock even for stats.
  // Concurrent stats would contend with each other before reading any content.
  for(const path of paths){
    const observed=await retryProjectRead(()=>stat(path));
    if(observed.folder_id!==folder||observed.path!==path||observed.node?.kind!=="file")throw new Error("latex_project_file_missing");
    files.push({path,sha256:observed.node.sha256,size:observed.node.size});
  }
  return latexProject({version:1,folder,main,output,files})!;
}
export function latexProjectTool(value:LatexProject,folder:string):ToolCall{
  const project=latexProject(value)!;
  if(project.folder!==folder)throw new Error("latex_project_workspace_changed");
  const main=project.files.find(file=>file.path===project.main)!;
  return {id:"ui",name:"document_write",arguments:{operation:"compile_latex",inputs:[main,...project.files.filter(file=>file!==main)].map(file=>({folder_id:project.folder,path:file.path,expected_sha256:file.sha256})),output:{path:project.output},preview:true}};
}
export function latexProjectMatchesTool(project:LatexProject,tool:ToolCall):boolean{
  const args=object(tool.arguments);
  if(tool.name!=="document_write"||args.operation!=="compile_latex"||!Array.isArray(args.inputs)||args.inputs.length!==project.files.length||object(args.inputs[0]).path!==project.main)return false;
  const seen=new Set<string>();
  return args.inputs.every(value=>{
    const input=object(value),file=project.files.find(file=>file.path===input.path);
    if(!file||seen.has(file.path)||input.folder_id!==project.folder||input.expected_sha256!==file.sha256)return false;
    seen.add(file.path);return true;
  });
}
/** Diagnostics may select only an exact member of the captured project. */
export function latexDiagnosticFile(project:LatexProject,file:unknown):LatexProjectFile|null{
  if(typeof file!=="string")return null;
  const relative=file.replace(/^\.\//,"").replace(/^in\/tex\//,"");
  if(!projectPath(relative))return null;
  return project.files.find(item=>item.path===relative||item.path===base(project.main)+relative)??null;
}
