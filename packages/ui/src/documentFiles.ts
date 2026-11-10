import type {Artifact,DownloadedFile} from "@cowork/client";
export type DocumentKind="markdown"|"html"|"text"|"pdf"|"image"|"office"|"binary";
export function documentKind(name:string,mediaType=""):DocumentKind{
  const extension=name.toLowerCase().split(".").at(-1),media=mediaType.toLowerCase().split(";")[0];
  if(media==="application/pdf"||extension==="pdf")return "pdf";
  if(["image/png","image/jpeg","image/webp","image/gif","image/bmp","image/tiff"].includes(media)||["png","jpg","jpeg","webp","gif","bmp","tif","tiff"].includes(extension??""))return "image";
  if(media==="text/html"||["html","htm"].includes(extension??""))return "html";
  if(media==="text/markdown"||["md","markdown"].includes(extension??""))return "markdown";
  if(["docx","xlsx","pptx","doc","xls","ppt","odt","ods","odp"].includes(extension??""))return "office";
  if(media.startsWith("text/")||["txt","tex","bib","sty","cls","bst","csv","json","jsonl","xml","yaml","yml","log","js","ts","tsx","jsx","css","py","rs","sh","toml","svg"].includes(extension??""))return "text";
  return "binary";
}
export function previewMediaType(name:string,declared=""):string{
  const kind=documentKind(name,declared),extension=name.toLowerCase().split(".").at(-1);
  if(kind==="pdf")return "application/pdf";
  if(kind==="html")return "text/html";
  if(kind==="image")return ({png:"image/png",jpg:"image/jpeg",jpeg:"image/jpeg",webp:"image/webp",gif:"image/gif",bmp:"image/bmp",tif:"image/tiff",tiff:"image/tiff"} as Record<string,string>)[extension??""]??declared;
  return kind==="markdown"?"text/markdown":["text"].includes(kind)?"text/plain":"application/octet-stream";
}
export async function verifyDocument(file:DownloadedFile):Promise<DownloadedFile>{
  if(file.size>67_108_864||file.blob.size!==file.size||!/^[0-9a-f]{64}$/.test(file.sha256))throw new Error("download_integrity");
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",await file.blob.arrayBuffer()))).map(byte=>byte.toString(16).padStart(2,"0")).join("");
  if(digest!==file.sha256)throw new Error("download_integrity");return file;
}
export function artifactDocument(artifact:Artifact):Promise<DownloadedFile>{
  return verifyDocument({file_name:artifact.name,size:artifact.size,sha256:artifact.sha256,blob:new Blob([artifact.content],{type:previewMediaType(artifact.name,artifact.media_type)})});
}
/** Immutable siblings with the same name are real historical outputs, not invented revisions. */
export function artifactVersions(artifact:Artifact,artifacts:Artifact[]){
  return artifacts.filter(item=>item.order_id===artifact.order_id&&item.name===artifact.name).slice().sort((a,b)=>b.created_at-a.created_at||b.id.localeCompare(a.id));
}
export const previewContentPolicy="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
export function isolatedHtml(source:string):string{
  if(typeof document==="undefined")return "<!doctype html><html><body><pre>Preview requires a browser.</pre></body></html>";
  // Template contents are inert: unlike a parsed standalone document, source images
  // and frames cannot begin fetching before their attributes are removed.
  const template=document.createElement("template");template.innerHTML=source;
  template.content.querySelectorAll("script,iframe,frame,frameset,object,embed,link,meta,base,form,audio,video,source,track,template").forEach(node=>node.remove());
  for(const element of template.content.querySelectorAll("*"))for(const attribute of Array.from(element.attributes)){
    const name=attribute.name.toLowerCase(),value=attribute.value;
    if(name.startsWith("on")||["srcdoc","srcset","action","formaction","target","ping","poster","background","autofocus","contenteditable","download"].includes(name)||((name==="href"||name==="xlink:href")&&!value.startsWith("#"))||(name==="src"&&!/^data:image\/(png|jpeg|webp|gif);base64,[a-z0-9+/=\s]+$/i.test(value)))element.removeAttribute(attribute.name);
  }
  return '<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="'+previewContentPolicy+'"></head><body>'+template.innerHTML+'</body></html>';
}
